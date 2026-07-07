// The variable-slot environment behind inference: bind/lookup for global and local slots, the
// undo-logged entry writes that let branches, loop passes, and probes roll back without cloning
// the stub-laden environment, branch joins, reaching-write notes for captured slots, and the
// loop fixed point. The typing-reference sections on variables and control-flow joins are the
// contract; `InferenceState` itself lives in the parent module.
use {
    super::{
        Binding, BuiltinKind, EnvironmentKey, EnvironmentSnapshot, InferenceError, InferenceState,
        LoopAccessLog, LoopMemo, ResolutionContext, StrictOriginKind,
    },
    crate::{
        hir::Expression,
        interner::Symbol,
        naming::{BindingId, NamesLocal},
        types::{CoreType, TypeScheme},
    },
    std::collections::{BTreeMap, BTreeSet},
    tree_sitter::Range,
};

impl InferenceState {
    pub fn bind_global_name(&mut self, symbol: Symbol, core_type: CoreType, range: Range) {
        self.bind_global_scheme(symbol, TypeScheme::monomorphic(core_type), range);
    }

    pub fn bind_name(&mut self, symbol: Symbol, core_type: CoreType, range: Range) {
        self.bind_global_name(symbol, core_type, range);
    }

    pub fn bind_global_scheme(&mut self, symbol: Symbol, type_scheme: TypeScheme, range: Range) {
        self.set_environment_entry(
            EnvironmentKey::Global(symbol),
            Some(Binding {
                type_scheme,
                range,
                unsupplied: false,
            }),
        );
    }

    pub fn bind_scheme(&mut self, symbol: Symbol, type_scheme: TypeScheme, range: Range) {
        self.bind_global_scheme(symbol, type_scheme, range);
    }

    pub(super) fn bind_local_name(
        &mut self,
        binding_id: BindingId,
        core_type: CoreType,
        range: Range,
    ) {
        self.bind_local_scheme(binding_id, TypeScheme::monomorphic(core_type), range);
    }

    pub(super) fn bind_local_scheme(
        &mut self,
        binding_id: BindingId,
        type_scheme: TypeScheme,
        range: Range,
    ) {
        self.set_environment_entry(
            EnvironmentKey::Local(binding_id),
            Some(Binding {
                type_scheme,
                range,
                unsupplied: false,
            }),
        );
    }

    // The single chokepoint for environment writes: records the key's prior value while an
    // environment snapshot is active so a control-flow region can be reverted.
    pub(super) fn set_environment_entry(&mut self, key: EnvironmentKey, entry: Option<Binding>) {
        self.log_loop_access(key);
        let previous = match entry {
            Some(binding) => self.environment.insert(key, binding),
            None => self.environment.remove(&key),
        };
        if self.environment_snapshot_depth > 0 {
            self.environment_log.push((key, previous));
        }
    }

    // Records `key` (with its current value on first touch) in every active loop region's access
    // log; the logs become the regions' memo guards.
    pub(super) fn log_loop_access(&mut self, key: EnvironmentKey) {
        if self.loop_access_logs.is_empty() {
            return;
        }
        let value = self.environment.get(&key).cloned();
        for log in &mut self.loop_access_logs {
            if !log.accesses.contains_key(&key) {
                if !log.first_pass {
                    log.complete = false;
                }
                log.accesses.insert(key, value.clone());
            }
        }
    }

    // Begins an environment region (a branch, a loop pass, or a function body) whose writes will
    // be reverted by `environment_rollback`. Nested regions compose like unification snapshots.
    pub(super) fn environment_snapshot(&mut self) -> EnvironmentSnapshot {
        self.environment_snapshot_depth += 1;
        EnvironmentSnapshot {
            log_length: self.environment_log.len(),
        }
    }

    // Reverts every environment write recorded since `snapshot` and returns each touched key's
    // value at the moment of rollback — the region's final values (`None` = the region removed the
    // entry) — so a control-flow join can merge them with the restored pre-state.
    // Closes an environment region *keeping* its writes: the recorded entries stay in the log so
    // they revert with the enclosing region instead (at depth zero there is nothing to revert
    // into, so the log clears). Used when a discovery pass turns out to be the only pass needed.
    pub(super) fn environment_commit(&mut self, _snapshot: EnvironmentSnapshot) {
        debug_assert!(
            self.environment_snapshot_depth > 0,
            "environment commit without an open snapshot"
        );
        self.environment_snapshot_depth -= 1;
        if self.environment_snapshot_depth == 0 {
            self.environment_log.clear();
        }
    }

    pub(super) fn environment_rollback(
        &mut self,
        snapshot: EnvironmentSnapshot,
    ) -> BTreeMap<EnvironmentKey, Option<Binding>> {
        debug_assert!(
            self.environment_snapshot_depth > 0,
            "environment rollback without an open snapshot"
        );
        self.environment_snapshot_depth -= 1;
        let recorded = self.environment_log.split_off(snapshot.log_length);
        let mut region_values = BTreeMap::new();
        for (key, _) in &recorded {
            region_values
                .entry(*key)
                .or_insert_with(|| self.environment.get(key).cloned());
        }
        for (key, previous) in recorded.into_iter().rev() {
            match previous {
                Some(binding) => {
                    self.environment.insert(key, binding);
                }
                None => {
                    self.environment.remove(&key);
                }
            }
        }
        region_values
    }

    // Merges the environment effects of two alternative control-flow paths (both already rolled
    // back, so the environment currently holds the shared pre-state): every key either path
    // touched gets the join of its two path-final values, with an untouched path contributing the
    // pre-state value.
    pub(super) fn join_branch_environments(
        &mut self,
        mut left: BTreeMap<EnvironmentKey, Option<Binding>>,
        mut right: BTreeMap<EnvironmentKey, Option<Binding>>,
        expression: &Expression,
    ) -> Result<(), InferenceError> {
        let keys: BTreeSet<EnvironmentKey> = left.keys().chain(right.keys()).copied().collect();
        for key in keys {
            let pre_state = self.environment.get(&key).cloned();
            let left_value = left.remove(&key).unwrap_or_else(|| pre_state.clone());
            let right_value = right.remove(&key).unwrap_or(pre_state);
            let joined = self.join_environment_entries(left_value, right_value, expression)?;
            self.set_environment_entry(key, joined);
        }
        Ok(())
    }

    // The binding a variable slot holds after two control paths merge. Identical entries stay; a
    // slot written on only one path optimistically keeps the written binding (a read on the
    // unwritten path is covered by the naming-level maybe-undefined warning); genuinely different
    // entries join into a monotype via `join_types` — a polymorphic scheme survives only while a
    // single write reaches (the generalization rule in the control-flow-joins section of the
    // typing reference).
    pub(super) fn join_environment_entries(
        &mut self,
        left: Option<Binding>,
        right: Option<Binding>,
        expression: &Expression,
    ) -> Result<Option<Binding>, InferenceError> {
        match (left, right) {
            (None, None) => Ok(None),
            (Some(binding), None) | (None, Some(binding)) => Ok(Some(binding)),
            (Some(left), Some(right)) => {
                if left == right {
                    return Ok(Some(left));
                }
                let range = left.range;
                // Definitely-missing only when both merging paths agree; a supplied path clears it.
                let unsupplied = left.unsupplied && right.unsupplied;
                let left_type = self.instantiate_type_scheme(&left.type_scheme)?;
                let right_type = self.instantiate_type_scheme(&right.type_scheme)?;
                let left_type = self.resolve(left_type)?;
                let right_type = self.resolve(right_type)?;
                // An unmodelled path makes the merged slot unmodelled, matching how `Unknown`
                // absorbs unions everywhere else in the checker.
                let joined = if left_type == CoreType::Unknown || right_type == CoreType::Unknown {
                    CoreType::Unknown
                } else {
                    let joined = self.join_types(left_type, right_type, expression)?;
                    self.resolve(joined)?
                };
                Ok(Some(Binding {
                    type_scheme: TypeScheme::monomorphic(joined),
                    range,
                    unsupplied,
                }))
            }
        }
    }

    pub fn bind_builtin(&mut self, symbol: Symbol, builtin_kind: BuiltinKind) {
        self.builtins.insert(symbol, builtin_kind);
    }

    pub(super) fn lookup_local_name(&self, binding_id: BindingId) -> Option<&Binding> {
        self.environment.get(&EnvironmentKey::Local(binding_id))
    }

    pub fn bind_overload_set(&mut self, symbol: Symbol, schemes: Vec<TypeScheme>) {
        self.overload_sets.insert(symbol, schemes);
    }

    pub fn lookup_global_name(&self, symbol: Symbol) -> Option<&Binding> {
        self.environment.get(&EnvironmentKey::Global(symbol))
    }

    pub fn lookup_name(&self, symbol: Symbol) -> Option<&Binding> {
        self.lookup_global_name(symbol)
    }

    // Bookkeeping for a write to a captured slot that has post-capture writes: accumulates the
    // slot's write join (variables erased so the entry survives unification rollbacks) and flags
    // the discovery re-pass.
    pub(super) fn note_slot_write(
        &mut self,
        local_naming: &NamesLocal,
        binding_id: BindingId,
        written: &CoreType,
        expression: &Expression,
    ) -> Result<(), InferenceError> {
        if !local_naming.capture_repass_slots.contains(&binding_id) {
            return Ok(());
        }
        self.wrote_repass_slot = true;
        let sanitized = self.erase_inference_variables(written.clone())?;
        let joined = match self.captured_write_joins.get(&binding_id).cloned() {
            None => sanitized,
            Some(existing) => {
                if existing == CoreType::Unknown || sanitized == CoreType::Unknown {
                    CoreType::Unknown
                } else {
                    let joined = self.join_types(existing, sanitized, expression)?;
                    self.erase_inference_variables(joined)?
                }
            }
        };
        self.captured_write_joins.insert(binding_id, joined);
        Ok(())
    }

    // A loop body may run zero or more times, so the types flowing around the back edge join into
    // the body's entry environment: `iterate` is re-run until that entry stabilizes, starting from
    // the plain pre-state (real code stabilizes on the second pass). At the pass cap, any slot
    // whose type is still changing (for example one growing structurally each iteration) is
    // widened to `Unknown` as a termination safety net — a strict origin, since the widening
    // introduces a genuine `Unknown`. Afterwards the loop's exit state is applied: `join(pre, out)`
    // for `for`/`while` and for a `repeat` whose body contains `break`/`next` (zero or partial
    // iterations possible), the final out-state for an exit-free `repeat` (runs to completion at
    // least once). The loop variable is re-seeded from the iterable on every pass; after the loop
    // it keeps its final state joined with the pre-loop state (R keeps the last element).
    //
    // Nested loops would re-run an inner region's fixed point once per outer pass (passes^depth):
    // each converged region is memoized by its (read ∪ written keys → entry values) guard, so an
    // enclosing pass whose entry state is unchanged replays the exit effects in O(touched keys).
    pub(super) fn infer_loop_to_fixed_point(
        &mut self,
        expression: &Expression,
        loop_variable: Option<(EnvironmentKey, CoreType, Range)>,
        runs_at_least_once: bool,
        resolution_context: Option<&ResolutionContext<'_>>,
        mut iterate: impl FnMut(&mut Self) -> Result<(), InferenceError>,
    ) -> Result<(), InferenceError> {
        if let Some(memo) = self.loop_memos.get(&expression.id) {
            let guard_holds = memo
                .guard
                .iter()
                .all(|(key, value)| self.environment.get(key) == value.as_ref());
            if guard_holds {
                let memo = memo.clone();
                for (key, value) in memo.exit_effects {
                    self.set_environment_entry(key, value);
                }
                // Re-recording is deduplicated, so this only restores origins dropped by a
                // discovery pass's truncation.
                for origin in memo.origins {
                    self.record_strict_origin(origin.expression_id, origin.range, origin.kind);
                }
                return Ok(());
            }
        }

        let loop_variable_pre_state = loop_variable
            .as_ref()
            .map(|(key, _, _)| self.environment.get(key).cloned());
        let origins_mark = self.strict_origins.len();
        self.loop_access_logs.push(LoopAccessLog {
            accesses: BTreeMap::new(),
            first_pass: true,
            complete: true,
        });
        let outcome = self.run_loop_passes(
            expression,
            &loop_variable,
            runs_at_least_once,
            resolution_context,
            &mut iterate,
        );
        let access_log = self
            .loop_access_logs
            .pop()
            .unwrap_or_else(|| panic!("loop access log missing for {:?}", expression.id));
        let mut exit_effects = outcome?;

        // The loop variable stays visible after the loop with its final state joined against the
        // pre-loop state (zero iterations leave the previous value or, if there was none, the
        // naming layer's maybe-undefined warning applies).
        if let Some((key, _, _)) = &loop_variable {
            let final_state = exit_effects.get(key).cloned().flatten();
            let joined = self.join_environment_entries(
                loop_variable_pre_state.flatten(),
                final_state,
                expression,
            )?;
            exit_effects.insert(*key, joined);
        }

        for (key, value) in &exit_effects {
            self.set_environment_entry(*key, value.clone());
        }
        if access_log.complete {
            let origins = self.strict_origins[origins_mark..].to_vec();
            self.loop_memos.insert(
                expression.id,
                LoopMemo {
                    guard: access_log.accesses,
                    exit_effects,
                    origins,
                },
            );
        }
        Ok(())
    }

    // The fixed-point passes of `infer_loop_to_fixed_point`, returning the exit effects to apply
    // (the loop variable's post-state is handled by the caller). Split out so the caller can pop
    // the access log on both the success and the error path.
    pub(super) fn run_loop_passes(
        &mut self,
        expression: &Expression,
        loop_variable: &Option<(EnvironmentKey, CoreType, Range)>,
        runs_at_least_once: bool,
        resolution_context: Option<&ResolutionContext<'_>>,
        iterate: &mut impl FnMut(&mut Self) -> Result<(), InferenceError>,
    ) -> Result<BTreeMap<EnvironmentKey, Option<Binding>>, InferenceError> {
        const LOOP_JOIN_PASSES: usize = 3;

        let mut entry: BTreeMap<EnvironmentKey, Option<Binding>> = BTreeMap::new();
        let mut exit: BTreeMap<EnvironmentKey, Option<Binding>> = BTreeMap::new();
        let mut still_changing: BTreeSet<EnvironmentKey> = BTreeSet::new();
        let mut converged = false;
        for _pass in 0..LOOP_JOIN_PASSES {
            let snapshot = self.environment_snapshot();
            for (key, value) in &entry {
                self.set_environment_entry(*key, value.clone());
            }
            if let Some((key, item_type, range)) = loop_variable {
                self.set_environment_entry(
                    *key,
                    Some(Binding {
                        type_scheme: TypeScheme::monomorphic(item_type.clone()),
                        range: *range,
                        unsupplied: false,
                    }),
                );
            }
            let result = iterate(self);
            exit = self.environment_rollback(snapshot);
            if let Some(log) = self.loop_access_logs.last_mut() {
                log.first_pass = false;
            }
            result?;

            let mut next_entry = entry.clone();
            for (key, exit_value) in &exit {
                let pre_state = self.environment.get(key).cloned();
                let joined =
                    self.join_environment_entries(pre_state, exit_value.clone(), expression)?;
                next_entry.insert(*key, joined);
            }
            if let Some((key, _, _)) = loop_variable {
                next_entry.remove(key);
            }
            still_changing = next_entry
                .iter()
                .filter(|(key, value)| entry.get(*key) != Some(*value))
                .map(|(key, _)| *key)
                .collect();
            if still_changing.is_empty() {
                converged = true;
                break;
            }
            entry = next_entry;
        }
        if !converged {
            for key in &still_changing {
                let range = entry
                    .get(key)
                    .and_then(|value| value.as_ref())
                    .map(|binding| binding.range)
                    .unwrap_or(expression.range);
                entry.insert(
                    *key,
                    Some(Binding {
                        type_scheme: TypeScheme::monomorphic(CoreType::Unknown),
                        range,
                        unsupplied: false,
                    }),
                );
                let symbol = match key {
                    EnvironmentKey::Global(symbol) => Some(*symbol),
                    EnvironmentKey::Local(binding_id) => resolution_context.and_then(|context| {
                        context
                            .local_naming
                            .bindings
                            .get(binding_id)
                            .map(|binding| binding.symbol)
                    }),
                };
                if let Some(symbol) = symbol {
                    self.record_strict_origin(
                        expression.id,
                        range,
                        StrictOriginKind::LoopWidened(symbol),
                    );
                }
            }
        }

        let mut exit_effects = BTreeMap::new();
        if runs_at_least_once && converged {
            for (key, value) in exit {
                exit_effects.insert(key, value);
            }
        } else {
            for (key, value) in entry {
                exit_effects.insert(key, value);
            }
            if let Some((key, _, _)) = loop_variable
                && let Some(final_state) = exit.remove(key)
            {
                // `entry` excludes the re-seeded loop variable; its region-final state still
                // feeds the caller's post-loop join.
                exit_effects.insert(*key, final_state);
            }
        }
        Ok(exit_effects)
    }
}
