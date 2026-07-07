// Control-flow checking: `if` with guard narrowing (type guards and the `missing()`
// supplied-state guard), the guard predicate table, divergence detection, and the `for` /
// `while` / `repeat` loops driving the environment's fixed point. The typing-reference sections
// on control flow, guard narrowing, and loops are the contract.
use {
    super::{
        Binding, EnvironmentKey, InferenceError, InferenceState, ResolutionContext,
        contains_loop_exit,
        operand::{iterable_item_type, nullable_type, refine_guarded_type},
    },
    crate::{
        hir::{Expression, ExpressionId, ExpressionKind, HirArena},
        interner::Symbol,
        naming::find_binding,
        typecheck::TypeDefinitionEnvironment,
        types::{Atomic, CoreType, TypeScheme},
    },
    std::collections::BTreeMap,
    tree_sitter::Range,
};

// A type-guard predicate recognized in `if` conditions (`is.null(x)`, `is.character(x)`, ...).
// Guards narrow the guarded local variable's type along the branch edges; see the guard-narrowing
// section of the typing reference for the exact filtering rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GuardPredicate {
    Null,
    Character,
    Logical,
    Integer,
    Double,
    Numeric,
    Function,
    List,
}

// The names each guard predicate answers to. Seeded into `guard_predicates` by the builtin
// constructors so condition inspection is a symbol lookup, not a string compare.
pub(super) const GUARD_PREDICATES: &[(&str, GuardPredicate)] = &[
    ("is.null", GuardPredicate::Null),
    ("is.character", GuardPredicate::Character),
    ("is.logical", GuardPredicate::Logical),
    ("is.integer", GuardPredicate::Integer),
    ("is.double", GuardPredicate::Double),
    ("is.numeric", GuardPredicate::Numeric),
    ("is.function", GuardPredicate::Function),
    ("is.list", GuardPredicate::List),
];

// A guard's effect on the guarded slot: the entry to install on each edge (`None` = that edge
// leaves the entry untouched). `range` preserves the original binding's definition range.
pub(super) struct GuardRefinement {
    key: EnvironmentKey,
    range: Range,
    true_type: Option<CoreType>,
    false_type: Option<CoreType>,
    // Per-edge supplied state for a `missing(name)` guard on a defaultless parameter: reads on
    // the unsupplied edge error. Type guards leave both edges supplied.
    true_unsupplied: bool,
    false_unsupplied: bool,
}

impl InferenceState {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn infer_if_expression(
        &mut self,
        condition: &Expression,
        consequence: &Expression,
        alternative: Option<&Expression>,
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        self.expect_scalar_logical(condition, arena, resolution_context, type_definitions)?;

        // A type-guard condition refines the guarded slot along the branch edges. The refinement
        // is an ordinary undo-logged entry write inside the branch region, so a branch write
        // simply replaces it and the join below sees final values either way.
        let refinement = self.condition_refinement(condition, arena, resolution_context)?;

        let snapshot = self.environment_snapshot();
        if let Some(refinement) = &refinement
            && let Some(true_type) = &refinement.true_type
        {
            self.set_environment_entry(
                refinement.key,
                Some(Binding {
                    type_scheme: TypeScheme::monomorphic(true_type.clone()),
                    range: refinement.range,
                    unsupplied: refinement.true_unsupplied,
                }),
            );
        }
        let consequence_result = self.infer_expression_with_context(
            consequence,
            arena,
            resolution_context,
            type_definitions,
        );
        let consequence_bindings = self.environment_rollback(snapshot);
        let consequence_type = self.resolve(consequence_result?)?;
        let consequence_diverges = self.expression_diverges(arena, consequence.id);

        // The false edge: the `else` branch when present, otherwise a synthetic empty region that
        // carries just the false-edge refinement — that is what survives past the `if` when the
        // consequence diverges (the early-exit guard pattern).
        let mut alternative_outcome = None;
        let alternative_bindings = match alternative {
            Some(alternative) => {
                let snapshot = self.environment_snapshot();
                if let Some(refinement) = &refinement
                    && let Some(false_type) = &refinement.false_type
                {
                    self.set_environment_entry(
                        refinement.key,
                        Some(Binding {
                            type_scheme: TypeScheme::monomorphic(false_type.clone()),
                            range: refinement.range,
                            unsupplied: refinement.false_unsupplied,
                        }),
                    );
                }
                let alternative_result = self.infer_expression_with_context(
                    alternative,
                    arena,
                    resolution_context,
                    type_definitions,
                );
                let bindings = self.environment_rollback(snapshot);
                alternative_outcome = Some((
                    self.resolve(alternative_result?)?,
                    self.expression_diverges(arena, alternative.id),
                ));
                bindings
            }
            None => {
                let mut bindings = BTreeMap::new();
                if let Some(refinement) = &refinement
                    && let Some(false_type) = &refinement.false_type
                {
                    bindings.insert(
                        refinement.key,
                        Some(Binding {
                            type_scheme: TypeScheme::monomorphic(false_type.clone()),
                            range: refinement.range,
                            unsupplied: refinement.false_unsupplied,
                        }),
                    );
                }
                bindings
            }
        };

        // A diverging branch never falls through, so it contributes no state: the surviving edge's
        // final values (refinement included) apply directly instead of joining with a path that
        // cannot reach the code after the `if`. When neither (or both — dead code) diverge, every
        // touched slot joins as before: pre-state first, so a union reads in execution order.
        let alternative_diverges = alternative_outcome
            .as_ref()
            .is_some_and(|(_, diverges)| *diverges);
        match (consequence_diverges, alternative_diverges) {
            (true, false) => {
                for (key, value) in alternative_bindings {
                    self.set_environment_entry(key, value);
                }
            }
            (false, true) => {
                for (key, value) in consequence_bindings {
                    self.set_environment_entry(key, value);
                }
            }
            _ => {
                if alternative.is_some() {
                    self.join_branch_environments(
                        consequence_bindings,
                        alternative_bindings,
                        expression,
                    )?;
                } else {
                    // Fall-through side first, so a union reads in execution order (the pre-state
                    // before the branch's retype: `integer | character`, not the reverse).
                    self.join_branch_environments(
                        alternative_bindings,
                        consequence_bindings,
                        expression,
                    )?;
                }
            }
        }

        let Some((alternative_type, _)) = alternative_outcome else {
            // Without an `else` the construct may fall through untouched, contributing `NULL`; a
            // diverging branch never yields at all, so the whole expression is plain `NULL`.
            return Ok(if consequence_diverges {
                CoreType::Null
            } else {
                nullable_type(consequence_type)
            });
        };

        // A diverging branch contributes no value either: `x <- if (c) return(NULL) else 5`
        // gives `x` the surviving branch's type.
        if consequence_diverges != alternative_diverges {
            return Ok(if consequence_diverges {
                alternative_type
            } else {
                consequence_type
            });
        }

        // An unmodelled branch makes the result unmodelled rather than claiming the other branch's
        // type, matching how `Unknown` propagates (and absorbs unions) through the rest of the
        // checker.
        if consequence_type == CoreType::Unknown || alternative_type == CoreType::Unknown {
            return Ok(CoreType::Unknown);
        }

        self.join_types(consequence_type, alternative_type, expression)
    }

    // The guard refinement an `if` condition induces, when the condition is a recognized predicate
    // applied to a plain local variable read (negation swaps the edges). `None` when the condition
    // is no guard, the callee is locally shadowed, the argument is not a resolved local slot, or
    // the guard cannot change the entry's type (see the guard-narrowing section of the typing
    // reference for the filtering rules).
    pub(super) fn condition_refinement(
        &mut self,
        condition: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
    ) -> Result<Option<GuardRefinement>, InferenceError> {
        match &condition.kind {
            ExpressionKind::UnaryNot { value } => Ok(self
                .condition_refinement(arena.get(*value), arena, resolution_context)?
                .map(|refinement| GuardRefinement {
                    key: refinement.key,
                    range: refinement.range,
                    true_type: refinement.false_type,
                    false_type: refinement.true_type,
                    true_unsupplied: refinement.false_unsupplied,
                    false_unsupplied: refinement.true_unsupplied,
                })),
            ExpressionKind::Call { callee, arguments } => {
                let callee_expression = arena.get(*callee);
                let ExpressionKind::Symbol(callee_symbol) = &callee_expression.kind else {
                    return Ok(None);
                };
                let is_missing_guard = Some(*callee_symbol) == self.missing_symbol;
                let predicate = self.guard_predicates.get(callee_symbol).copied();
                if !is_missing_guard && predicate.is_none() {
                    return Ok(None);
                }
                // A local binding shadowing the predicate name wins, exactly as in resolution.
                if let Some(context) = resolution_context
                    && context
                        .local_naming
                        .expression_resolutions
                        .contains_key(&callee_expression.id)
                {
                    return Ok(None);
                }
                let [argument] = arguments.as_slice() else {
                    return Ok(None);
                };
                if argument.name.is_some() {
                    return Ok(None);
                }
                let argument_expression = arena.get(argument.expression);
                let ExpressionKind::Symbol(argument_symbol) = argument_expression.kind else {
                    return Ok(None);
                };
                // The refined key is exactly the one a read of the argument resolves to: the local
                // slot under a naming context; the flat global entry in a context-less state (the
                // fixture drivers), where every binding lives in the global map. A name that is
                // non-local under a context is a package global — winner semantics, not
                // flow-refined — so no refinement.
                let key = match resolution_context {
                    Some(context) => match context
                        .local_naming
                        .expression_resolutions
                        .get(&argument_expression.id)
                    {
                        Some(binding_id) => EnvironmentKey::Local(*binding_id),
                        None => return Ok(None),
                    },
                    None => EnvironmentKey::Global(argument_symbol),
                };
                let Some(binding) = self.environment.get(&key).cloned() else {
                    return Ok(None);
                };
                let entry_type = self.instantiate_type_scheme(&binding.type_scheme)?;
                let entry_type = self.resolve(entry_type)?;
                // `missing(x)` on one of the current frame's defaultless parameters: the true
                // edge marks the slot unsupplied (reads there would fail at run time), the false
                // edge marks it supplied. The entry's type is unchanged on both edges.
                if is_missing_guard {
                    let EnvironmentKey::Local(binding_id) = key else {
                        return Ok(None);
                    };
                    if !self.missing_narrowable.contains(&binding_id) {
                        return Ok(None);
                    }
                    return Ok(Some(GuardRefinement {
                        key,
                        range: binding.range,
                        true_type: Some(entry_type.clone()),
                        false_type: Some(entry_type),
                        true_unsupplied: true,
                        false_unsupplied: false,
                    }));
                }
                let Some(predicate) = predicate else {
                    return Ok(None);
                };
                Ok(
                    refine_guarded_type(&entry_type, predicate).map(|(true_type, false_type)| {
                        GuardRefinement {
                            key,
                            range: binding.range,
                            true_type,
                            false_type,
                            true_unsupplied: false,
                            false_unsupplied: false,
                        }
                    }),
                )
            }
            _ => Ok(None),
        }
    }

    // Whether an expression never falls through to the code after it: `return`/`break`/`next`, a
    // call to `stop` (by bare name — the `local`/`return` rebinding caveat applies), a block whose
    // last expression diverges, or an `if`/`else` both of whose branches diverge.
    pub(super) fn expression_diverges(&self, arena: &HirArena, id: ExpressionId) -> bool {
        match &arena.get(id).kind {
            ExpressionKind::Return { .. } | ExpressionKind::Break | ExpressionKind::Next => true,
            ExpressionKind::Block { expressions, .. } => expressions
                .last()
                .is_some_and(|last| self.expression_diverges(arena, *last)),
            ExpressionKind::Call { callee, .. } => matches!(
                &arena.get(*callee).kind,
                ExpressionKind::Symbol(symbol) if Some(*symbol) == self.stop_symbol
            ),
            ExpressionKind::If {
                consequence,
                alternative: Some(alternative),
                ..
            } => {
                self.expression_diverges(arena, *consequence)
                    && self.expression_diverges(arena, *alternative)
            }
            _ => false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn infer_for_expression(
        &mut self,
        _expression_id: ExpressionId,
        variable: Symbol,
        sequence: &Expression,
        body: &Expression,
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        let range = expression.range;
        // The sequence is evaluated once, before any iteration, so it stays outside the loop
        // region.
        let inferred_sequence = self.infer_expression_with_context(
            sequence,
            arena,
            resolution_context,
            type_definitions,
        )?;
        let sequence_type =
            self.resolve_structural(inferred_sequence, type_definitions, Some(sequence))?;
        let item_type = match iterable_item_type(&sequence_type) {
            Some(item_type) => item_type,
            // An unresolved inference variable stays unconstrained: R iterates vectors *and*
            // lists, and no single unification can express "either", so the loop variable
            // degrades to `Unknown` rather than committing the sequence to one shape.
            None if matches!(sequence_type, CoreType::Variable(_)) => CoreType::Unknown,
            None => {
                return Err(InferenceError::NotIterable {
                    actual: Box::new(sequence_type),
                    range: sequence.range,
                    expression_id: sequence.id,
                });
            }
        };

        let variable_key = match resolution_context.and_then(|context| {
            find_binding(context.local_naming, context.document_id, variable, range)
        }) {
            Some(binding_id) => {
                if let Some(context) = resolution_context {
                    self.note_slot_write(context.local_naming, binding_id, &item_type, expression)?;
                }
                EnvironmentKey::Local(binding_id)
            }
            None => EnvironmentKey::Global(variable),
        };
        self.infer_loop_to_fixed_point(
            expression,
            Some((variable_key, item_type, range)),
            false,
            resolution_context,
            |state| {
                state.infer_expression_with_context(
                    body,
                    arena,
                    resolution_context,
                    type_definitions,
                )?;
                Ok(())
            },
        )?;
        Ok(CoreType::Null)
    }

    pub(super) fn infer_while_expression(
        &mut self,
        condition: &Expression,
        body: &Expression,
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        // The condition re-evaluates before every iteration, so it belongs to the iterated region:
        // a read in it sees the types flowing around the back edge.
        self.infer_loop_to_fixed_point(expression, None, false, resolution_context, |state| {
            state.expect_scalar_logical(condition, arena, resolution_context, type_definitions)?;
            state.infer_expression_with_context(
                body,
                arena,
                resolution_context,
                type_definitions,
            )?;
            Ok(())
        })?;
        Ok(CoreType::Null)
    }

    pub(super) fn infer_repeat_expression(
        &mut self,
        body: &Expression,
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        // `repeat` runs its body at least once, but a `break`/`next` may leave before the body's
        // end, so only an exit-free body definitely applies all its writes.
        let runs_to_completion = !contains_loop_exit(arena, body.id);
        self.infer_loop_to_fixed_point(
            expression,
            None,
            runs_to_completion,
            resolution_context,
            |state| {
                state.infer_expression_with_context(
                    body,
                    arena,
                    resolution_context,
                    type_definitions,
                )?;
                Ok(())
            },
        )?;
        Ok(CoreType::Null)
    }

    pub(super) fn expect_scalar_logical(
        &mut self,
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<(), InferenceError> {
        let inferred_type = self.infer_expression_with_context(
            expression,
            arena,
            resolution_context,
            type_definitions,
        )?;
        // Project a nominal operand to its representation first, so a nominal whose representation is
        // `logical` is accepted by `&&`/`||` and `if`/`while` conditions, exactly as `!`, arithmetic,
        // and comparison already project nominals.
        let resolved_type =
            self.resolve_structural(inferred_type, type_definitions, Some(expression))?;
        self.unify_with_context(CoreType::Scalar(Atomic::Logical), resolved_type, expression)?;
        Ok(())
    }
}
