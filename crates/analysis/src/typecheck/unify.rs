// The unification core: variable allocation (fresh and rigid), the undo-logged snapshot /
// rollback / commit machinery every probe rides on, constraint raising, resolution, `unify` and
// its structural cases, member-wise union logic, directional `check_compatibility`, scheme
// instantiation and generalization, and function-type unification. The typing-reference sections
// on unification, compatibility, and generics are the contract; `InferenceState` lives in the
// parent module.
use {
    super::{
        InferenceEntry, InferenceError, InferenceState, Level, RECURSION_LIMIT, Snapshot, UndoStep,
        Variance,
        operand::{
            atomic_widens_to, constraint_is_satisfied, constraint_violation_error,
            import_core_type, nullable_single_member,
        },
        parameter_variances,
    },
    crate::{
        hir::Expression,
        interner::Symbol,
        typecheck::TypeDefinitionEnvironment,
        types::{
            Atomic, Constraint, CoreType, FunctionType, InferenceVariableId, QuantifiedVariable,
            RecordField, RestParameter, TypeScheme,
        },
    },
    std::collections::{BTreeMap, BTreeSet},
};

impl InferenceState {
    pub fn fresh_variable(&mut self) -> InferenceVariableId {
        self.fresh_constrained_variable(Constraint::Unconstrained)
    }

    pub(super) fn fresh_rigid_variable(
        &mut self,
        name: Symbol,
        constraint: Constraint,
    ) -> InferenceVariableId {
        let variable = self.fresh_constrained_variable(constraint);
        self.set_rigid(variable, name);
        variable
    }

    // Renders a rigid variable as its declared type-parameter name (e.g. `T`) for diagnostics, so a
    // failed polymorphic-annotation check reads `expected T, found integer` instead of `type1`.
    pub(super) fn rigid_display(&self, variable: InferenceVariableId) -> CoreType {
        match self.rigid_variables.get(&variable) {
            Some(name) => CoreType::Nominal(*name, Vec::new()),
            None => CoreType::Variable(variable),
        }
    }

    // Resolves `core_type` and replaces every rigid skolem variable with its declared name, so a
    // diagnostic involving a `<T>` annotation shows `T` rather than an internal `type1`.
    pub(super) fn display_with_rigid_names(&mut self, core_type: &CoreType) -> CoreType {
        // Pure display path: a resolve failure degrades to `Unknown` rather than propagating, so
        // rendering a diagnostic or hover never aborts the check that produced it.
        let resolved = self.resolve(core_type.clone()).unwrap_or(CoreType::Unknown);
        self.substitute_rigid_names(&resolved)
    }

    pub(super) fn substitute_rigid_names(&self, core_type: &CoreType) -> CoreType {
        match core_type {
            CoreType::Variable(variable) => self.rigid_display(*variable),
            CoreType::Union(members) => CoreType::union_of(
                members
                    .iter()
                    .map(|member| self.substitute_rigid_names(member))
                    .collect(),
            ),
            CoreType::Vector(element) => {
                CoreType::Vector(Box::new(self.substitute_rigid_names(element)))
            }
            CoreType::NamedVector(element) => {
                CoreType::NamedVector(Box::new(self.substitute_rigid_names(element)))
            }
            CoreType::List(inner) => CoreType::List(Box::new(self.substitute_rigid_names(inner))),
            CoreType::NamedList(inner) => {
                CoreType::NamedList(Box::new(self.substitute_rigid_names(inner)))
            }
            CoreType::Tuple(items) => CoreType::Tuple(
                items
                    .iter()
                    .map(|item| self.substitute_rigid_names(item))
                    .collect(),
            ),
            CoreType::Record(fields) => CoreType::Record(
                fields
                    .iter()
                    .map(|field| {
                        RecordField::with_optional(
                            field.name,
                            self.substitute_rigid_names(&field.value),
                            field.optional,
                        )
                    })
                    .collect(),
            ),
            CoreType::Nominal(name, arguments) => CoreType::Nominal(
                *name,
                arguments
                    .iter()
                    .map(|argument| self.substitute_rigid_names(argument))
                    .collect(),
            ),
            CoreType::Function(function_type) => CoreType::Function(FunctionType::with_variadic(
                function_type
                    .parameters
                    .iter()
                    .map(|parameter| self.substitute_rigid_names(parameter))
                    .collect(),
                function_type
                    .named_parameters
                    .iter()
                    .map(|parameter| {
                        RecordField::with_optional(
                            parameter.name,
                            self.substitute_rigid_names(&parameter.value),
                            parameter.optional,
                        )
                    })
                    .collect(),
                function_type
                    .variadic
                    .as_ref()
                    .map(|variadic| RestParameter {
                        element: Box::new(self.substitute_rigid_names(&variadic.element)),
                        preceding_named: variadic.preceding_named,
                    }),
                self.substitute_rigid_names(&function_type.return_type),
            )),
            other => other.clone(),
        }
    }

    // Begins a speculative region for probing (e.g. a trial unification) that can be discarded.
    //
    // Probe contract — what a snapshot does and does NOT reverse:
    // - REVERSED: every union-find write (`entries`, via `set_entry`), every rigid-variable marker
    //   (`rigid_variables`, via `set_rigid`), and `next_variable_id` (so ids allocated inside the
    //   probe are reclaimed). These are exactly the fields the resolve / unify / check_compatibility
    //   / representation- and alias-lowering paths touch, which is all a `check_compatibility` probe
    //   can reach.
    // - NOT reversed: `environment`, `recorded_expression_types`, and `current_level`. This is safe
    //   for the intended probe use: `environment` and `recorded_expression_types` are mutated only by
    //   binding inference and expression-type recording, never by the compatibility/unification paths
    //   a probe runs; and `current_level` is balanced by paired `enter_level`/`exit_level`, so a probe
    //   that does not leak an unbalanced level change leaves it untouched. `recursion_depth` is
    //   likewise transient and deliberately excluded. A probe must keep its writes within this contract.
    //
    // Nested snapshots compose: an inner rollback truncates the log to the inner mark, leaving outer
    // writes intact.
    pub fn snapshot(&mut self) -> Snapshot {
        self.snapshot_depth += 1;
        Snapshot {
            log_len: self.undo_log.len(),
            next_variable_id: self.next_variable_id,
        }
    }

    // Reverses every recorded write made since `snapshot` (see the probe contract on `snapshot`),
    // restoring entries and rigid markers and reclaiming the variable ids allocated in between.
    // Leaves `recursion_depth` and the non-reversed fields untouched.
    pub fn rollback_to(&mut self, snapshot: Snapshot) {
        debug_assert!(
            self.snapshot_depth > 0,
            "rollback_to without an open snapshot"
        );
        while self.undo_log.len() > snapshot.log_len {
            match self.undo_log.pop() {
                Some(UndoStep::Entry { variable, previous }) => match previous {
                    Some(entry) => {
                        self.entries.insert(variable, entry);
                    }
                    None => {
                        self.entries.remove(&variable);
                    }
                },
                Some(UndoStep::Rigid { variable, previous }) => match previous {
                    Some(name) => {
                        self.rigid_variables.insert(variable, name);
                    }
                    None => {
                        self.rigid_variables.remove(&variable);
                    }
                },
                None => break,
            }
        }

        // Variable ids allocated after the snapshot are reclaimed. The log already removes their
        // `entries` and `rigid_variables` records; dropping any survivor with an id at or above the
        // restored counter is a safety net, since all ids are allocated monotonically and so cannot
        // predate the snapshot (and therefore cannot collide with a pre-existing variable or rigid).
        let stale_variables = self
            .entries
            .range(InferenceVariableId(snapshot.next_variable_id)..)
            .map(|(variable, _)| *variable)
            .collect::<Vec<_>>();
        for variable in stale_variables {
            self.entries.remove(&variable);
        }
        let stale_rigids = self
            .rigid_variables
            .range(InferenceVariableId(snapshot.next_variable_id)..)
            .map(|(variable, _)| *variable)
            .collect::<Vec<_>>();
        for variable in stale_rigids {
            self.rigid_variables.remove(&variable);
        }
        self.next_variable_id = snapshot.next_variable_id;

        self.snapshot_depth -= 1;
    }

    // Keeps every write made since `snapshot`. The log is retained while an outer snapshot is still
    // active (it may yet roll back over these writes) and cleared once the outermost region commits.
    pub fn commit(&mut self, snapshot: Snapshot) {
        debug_assert!(self.snapshot_depth > 0, "commit without an open snapshot");
        debug_assert!(self.undo_log.len() >= snapshot.log_len);
        self.snapshot_depth -= 1;
        if self.snapshot_depth == 0 {
            self.undo_log.clear();
        }
    }

    // The single chokepoint for union-find writes: records the prior entry (when a snapshot is
    // active) before overwriting, so the undo log stays complete and the committed path stays free.
    pub(super) fn set_entry(&mut self, variable: InferenceVariableId, entry: InferenceEntry) {
        if self.snapshot_depth > 0 {
            let previous = self.entries.get(&variable).cloned();
            self.undo_log.push(UndoStep::Entry { variable, previous });
        }
        self.entries.insert(variable, entry);
    }

    // The chokepoint for rigid-variable markers, mirroring `set_entry` so a probe can reverse a
    // skolem allocation that would otherwise leave a reclaimed id wrongly marked rigid.
    pub(super) fn set_rigid(&mut self, variable: InferenceVariableId, name: Symbol) {
        if self.snapshot_depth > 0 {
            let previous = self.rigid_variables.get(&variable).copied();
            self.undo_log.push(UndoStep::Rigid { variable, previous });
        }
        self.rigid_variables.insert(variable, name);
    }

    pub(super) fn fresh_constrained_variable(
        &mut self,
        constraint: Constraint,
    ) -> InferenceVariableId {
        let variable = InferenceVariableId(self.next_variable_id);
        self.next_variable_id += 1;
        self.set_entry(
            variable,
            InferenceEntry::Unbound {
                level: self.current_level,
                constraint,
            },
        );
        variable
    }

    // Raises the bound on whatever `core_type` resolves to. When it resolves to an unbound
    // variable, the variable records the stronger constraint; when it resolves to a concrete
    // type, the type itself must already satisfy the constraint.
    pub(super) fn constrain_type(
        &mut self,
        core_type: CoreType,
        constraint: Constraint,
        expression: Option<&Expression>,
    ) -> Result<(), InferenceError> {
        if constraint == Constraint::Unconstrained {
            return Ok(());
        }
        match self.resolve(core_type)? {
            CoreType::Variable(variable) => {
                // A rigid skolem stands for every possible T; it cannot carry a constraint (e.g. a
                // `<T>` body that does `value + 1L` would require T to be numeric, which the declared
                // unconstrained `<T>` does not promise).
                if self.rigid_variables.contains_key(&variable) {
                    return Err(InferenceError::ConstraintViolation {
                        constraint,
                        actual: Box::new(self.rigid_display(variable)),
                        range: expression.map(|current| current.range),
                        expression_id: expression.map(|current| current.id),
                    });
                }
                let raised_entry = match self.entries.get(&variable) {
                    Some(InferenceEntry::Unbound {
                        level,
                        constraint: existing,
                    }) => Some(InferenceEntry::Unbound {
                        level: *level,
                        constraint: (*existing).join(constraint),
                    }),
                    _ => None,
                };
                if let Some(entry) = raised_entry {
                    self.set_entry(variable, entry);
                }
                Ok(())
            }
            concrete_type if constraint_is_satisfied(constraint, &concrete_type) => Ok(()),
            concrete_type => Err(constraint_violation_error(
                constraint,
                concrete_type,
                expression,
            )),
        }
    }

    pub(super) fn enter_level(&mut self) {
        self.current_level += 1;
    }

    pub(super) fn exit_level(&mut self) {
        self.current_level -= 1;
    }

    pub fn entry(&self, variable: InferenceVariableId) -> Option<&InferenceEntry> {
        self.entries.get(&variable)
    }

    pub(super) fn check_compatibility(
        &mut self,
        actual_type: CoreType,
        expected_type: CoreType,
        type_definitions: &TypeDefinitionEnvironment,
        expression: Option<&Expression>,
    ) -> Result<bool, InferenceError> {
        if self.recursion_depth >= RECURSION_LIMIT {
            return Err(InferenceError::RecursionLimitExceeded);
        }
        self.recursion_depth += 1;
        // Probe the structural check speculatively. A successful check keeps the inference-variable
        // bindings it makes (the two `Variable` arms binding against the other side is how `@new` and
        // checked annotations infer their type arguments), but any false-or-erroring check reverses
        // every mutation. This makes the predicate pure on failure, so it leaks nothing and its result
        // is order-independent. The snapshot does not capture `recursion_depth`.
        let snapshot = self.snapshot();
        let result = self.check_compatibility_inner(
            actual_type,
            expected_type,
            type_definitions,
            expression,
        );
        match &result {
            Ok(true) => self.commit(snapshot),
            Ok(false) | Err(_) => self.rollback_to(snapshot),
        }
        self.recursion_depth -= 1;
        result
    }

    pub(super) fn check_compatibility_inner(
        &mut self,
        actual_type: CoreType,
        expected_type: CoreType,
        type_definitions: &TypeDefinitionEnvironment,
        expression: Option<&Expression>,
    ) -> Result<bool, InferenceError> {
        let actual_type = self.resolve(actual_type)?;
        let expected_type = self.resolve(expected_type)?;

        if expected_type == CoreType::Any || actual_type == CoreType::Any {
            return Ok(true);
        }

        if actual_type == expected_type {
            return Ok(true);
        }

        // A failed unification means "not compatible", but a blown recursion limit is a resource
        // guard, not a verdict — swallowing it into `false` would turn a pathological type into a
        // silently wrong answer instead of the loud limit error.
        if let CoreType::Variable(actual_var) = actual_type {
            return match self.unify_internal(CoreType::Variable(actual_var), expected_type, None) {
                Ok(_) => Ok(true),
                Err(InferenceError::RecursionLimitExceeded) => {
                    Err(InferenceError::RecursionLimitExceeded)
                }
                Err(_) => Ok(false),
            };
        }

        if let CoreType::Variable(expected_var) = expected_type {
            return match self.unify_internal(actual_type, CoreType::Variable(expected_var), None) {
                Ok(_) => Ok(true),
                Err(InferenceError::RecursionLimitExceeded) => {
                    Err(InferenceError::RecursionLimitExceeded)
                }
                Err(_) => Ok(false),
            };
        }

        match (actual_type, expected_type) {
            // A union value must be accepted in every shape it can take, so each actual member is
            // checked against the expected type. This arm comes first so union-vs-union reduces to
            // "every actual member fits somewhere in the expected union".
            (CoreType::Union(actual_members), expected_type) => {
                for member in actual_members {
                    if !self.check_compatibility(
                        member,
                        expected_type.clone(),
                        type_definitions,
                        expression,
                    )? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            // A value fits an expected union when it fits any member. Each attempt is its own
            // probe (`check_compatibility` rolls back failed attempts), so an earlier failing
            // member leaks no bindings into a later one.
            (actual_type, CoreType::Union(expected_members)) => {
                for member in expected_members {
                    if self.check_compatibility(
                        actual_type.clone(),
                        member,
                        type_definitions,
                        expression,
                    )? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            (
                CoreType::Nominal(actual_name, actual_arguments),
                CoreType::Nominal(expected_name, expected_arguments),
            ) if actual_name == expected_name
                && actual_arguments.len() == expected_arguments.len() =>
            {
                // Each type argument is checked in the direction dictated by where the parameter
                // occurs in the representation: covariant for return/container/direct positions,
                // contravariant (flipped) for function-parameter positions, and invariant (both
                // directions) when a parameter occurs in conflicting positions. Without a definition
                // the variance is unknown, so every argument is checked invariantly.
                // A missing definition leaves `variances` empty, so every argument defaults to
                // invariant below. This is conservative: it over-rejects (demands an exact match)
                // rather than over-accepting an unsound widening.
                let variances = type_definitions
                    .get(actual_name)
                    .map(parameter_variances)
                    .unwrap_or_default();

                for (index, (actual_argument, expected_argument)) in actual_arguments
                    .into_iter()
                    .zip(expected_arguments)
                    .enumerate()
                {
                    let variance = variances.get(index).copied().unwrap_or(Variance::Invariant);
                    let compatible = match variance {
                        // The parameter never occurs in the representation, so the argument is
                        // unconstrained and any argument is accepted.
                        Variance::Bivariant => true,
                        Variance::Covariant => self.check_compatibility(
                            actual_argument,
                            expected_argument,
                            type_definitions,
                            expression,
                        )?,
                        Variance::Contravariant => self.check_compatibility(
                            expected_argument,
                            actual_argument,
                            type_definitions,
                            expression,
                        )?,
                        Variance::Invariant => {
                            self.check_compatibility(
                                actual_argument.clone(),
                                expected_argument.clone(),
                                type_definitions,
                                expression,
                            )? && self.check_compatibility(
                                expected_argument,
                                actual_argument,
                                type_definitions,
                                expression,
                            )?
                        }
                    };
                    if !compatible {
                        return Ok(false);
                    }
                }

                Ok(true)
            }
            (CoreType::Nominal(actual_name, actual_arguments), other_type) => {
                let Some(representation_type) = self.nominal_representation_type(
                    actual_name,
                    &actual_arguments,
                    type_definitions,
                    expression,
                )?
                else {
                    return Ok(false);
                };

                self.check_compatibility(
                    representation_type,
                    other_type,
                    type_definitions,
                    expression,
                )
            }
            // A scalar coerces into a vector position, a named vector drops its names into a plain
            // vector position, and vectors check element-wise. Element recursion lands on the
            // scalar arms below for concrete elements (so `integer` widening applies inside
            // vectors too) and on the variable arms above for a generic element (`T[]`), which is
            // how a call like `sort(c(1L))` binds `T := integer`.
            (CoreType::Scalar(actual_atomic), CoreType::Vector(expected_element)) => self
                .check_compatibility(
                    CoreType::Scalar(actual_atomic),
                    *expected_element,
                    type_definitions,
                    expression,
                ),
            (CoreType::NamedVector(actual_element), CoreType::Vector(expected_element)) => self
                .check_compatibility(
                    *actual_element,
                    *expected_element,
                    type_definitions,
                    expression,
                ),
            // `integer` widens to `double` in compatibility (a directional check only — unification
            // never widens): R freely promotes integers in numeric contexts, and without this every
            // numeric parameter in the stub corpus had to be `Any` to avoid rejecting `mean(1L)`.
            (CoreType::Scalar(actual_atomic), CoreType::Scalar(expected_atomic))
                if atomic_widens_to(actual_atomic, expected_atomic) =>
            {
                Ok(true)
            }
            (CoreType::Vector(actual_element), CoreType::Vector(expected_element)) => self
                .check_compatibility(
                    *actual_element,
                    *expected_element,
                    type_definitions,
                    expression,
                ),
            (CoreType::NamedVector(actual_element), CoreType::NamedVector(expected_element)) => {
                self.check_compatibility(
                    *actual_element,
                    *expected_element,
                    type_definitions,
                    expression,
                )
            }
            // Fixed-shape structural compatibility, checked covariantly per element/field. This is
            // what lets `@new` and checked annotations on a `list(...)` accept (and unify) a value
            // whose fields are still inference variables, e.g. `@new Person` on
            // `list(name = name, age = age)` inside an unannotated function.
            (CoreType::Tuple(actual_items), CoreType::Tuple(expected_items))
                if actual_items.len() == expected_items.len() =>
            {
                for (actual_item, expected_item) in actual_items.into_iter().zip(expected_items) {
                    if !self.check_compatibility(
                        actual_item,
                        expected_item,
                        type_definitions,
                        expression,
                    )? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            (CoreType::Record(actual_fields), CoreType::Record(expected_fields))
                if actual_fields.len() == expected_fields.len() =>
            {
                for expected_field in expected_fields {
                    let Some(actual_field) = actual_fields
                        .iter()
                        .find(|field| field.name == expected_field.name)
                    else {
                        return Ok(false);
                    };
                    if !self.check_compatibility(
                        actual_field.value.clone(),
                        expected_field.value,
                        type_definitions,
                        expression,
                    )? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            (CoreType::Tuple(items), CoreType::List(item_type)) => {
                for item in items {
                    if !self.check_compatibility(
                        item,
                        *item_type.clone(),
                        type_definitions,
                        expression,
                    )? {
                        return Ok(false);
                    }
                }

                Ok(true)
            }
            (CoreType::Record(fields), CoreType::List(item_type))
            | (CoreType::Record(fields), CoreType::NamedList(item_type)) => {
                for field in fields {
                    if !self.check_compatibility(
                        field.value,
                        *item_type.clone(),
                        type_definitions,
                        expression,
                    )? {
                        return Ok(false);
                    }
                }

                Ok(true)
            }
            (CoreType::NamedList(actual_item_type), CoreType::List(expected_item_type))
            | (CoreType::NamedList(actual_item_type), CoreType::NamedList(expected_item_type))
            | (CoreType::List(actual_item_type), CoreType::List(expected_item_type)) => self
                .check_compatibility(
                    *actual_item_type,
                    *expected_item_type,
                    type_definitions,
                    expression,
                ),
            (CoreType::Function(actual_function), CoreType::Function(expected_function)) => {
                let actual_parameter_count =
                    actual_function.parameters.len() + actual_function.named_parameters.len();
                let expected_parameter_count =
                    expected_function.parameters.len() + expected_function.named_parameters.len();

                if actual_parameter_count != expected_parameter_count {
                    return Ok(false);
                }

                // Variadic compatibility is conservative: a variadic function is compatible only with
                // another variadic (their rest elements are contravariant, like ordinary parameters),
                // and a variadic/fixed pair is always incompatible. The rest parameters must also sit
                // at the same formal position — the position decides which parameters callers may
                // fill positionally. This over-rejects some safe pairings but never admits an
                // unsound one.
                match (&actual_function.variadic, &expected_function.variadic) {
                    (Some(actual_variadic), Some(expected_variadic)) => {
                        if actual_variadic.preceding_named != expected_variadic.preceding_named {
                            return Ok(false);
                        }
                        if !self.check_compatibility(
                            (*expected_variadic.element).clone(),
                            (*actual_variadic.element).clone(),
                            type_definitions,
                            expression,
                        )? {
                            return Ok(false);
                        }
                    }
                    (None, None) => {}
                    _ => return Ok(false),
                }

                // Parameters pair by NAME where both sides name them (R matches call arguments
                // against formal names, so `fn(a: integer, b: character)` and a function defined
                // `function(b, a)` pair a-with-a and b-with-b regardless of order); unnamed
                // (positional) parameters consume the remaining slots left to right. A named
                // expected parameter with no same-named actual falls back to positional pairing —
                // interface names that do not exist on the actual function are the annotation
                // path's hard error, while plain value flow stays permissive for unnamed shapes.
                let mut actual_parameters = actual_function
                    .parameters
                    .into_iter()
                    .map(|parameter| (None, parameter, false))
                    .collect::<Vec<_>>();
                actual_parameters.extend(
                    actual_function
                        .named_parameters
                        .into_iter()
                        .map(|parameter| {
                            (Some(parameter.name), parameter.value, parameter.optional)
                        }),
                );

                let mut paired: Vec<Option<(CoreType, bool)>> = vec![None; actual_parameters.len()];
                let mut positional_expected: Vec<(CoreType, bool)> = Vec::new();
                for parameter in expected_function.named_parameters {
                    match actual_parameters
                        .iter()
                        .position(|(name, ..)| *name == Some(parameter.name))
                    {
                        Some(index) if paired[index].is_none() => {
                            paired[index] = Some((parameter.value, parameter.optional));
                        }
                        _ => positional_expected.push((parameter.value, parameter.optional)),
                    }
                }
                let mut positional_expected = expected_function
                    .parameters
                    .into_iter()
                    .map(|parameter| (parameter, false))
                    .chain(positional_expected);
                for slot in paired.iter_mut() {
                    if slot.is_none() {
                        *slot = positional_expected.next();
                    }
                }

                for ((_, actual_param, actual_optional), (expected_param, expected_optional)) in
                    actual_parameters
                        .into_iter()
                        .zip(paired.into_iter().map(|slot| {
                            slot.expect("parameter counts were checked equal before pairing")
                        }))
                {
                    // An expected-optional parameter promises callers they may omit it, so
                    // the actual function must have a default for that parameter.
                    if expected_optional && !actual_optional {
                        return Ok(false);
                    }

                    // Parameters are contravariant: a function used where `expected` is wanted
                    // must accept every argument the expected interface may pass, so the expected
                    // parameter type must be compatible with the actual one.
                    if !self.check_compatibility(
                        expected_param,
                        actual_param,
                        type_definitions,
                        expression,
                    )? {
                        return Ok(false);
                    }
                }

                // Return types stay covariant.
                self.check_compatibility(
                    *actual_function.return_type,
                    *expected_function.return_type,
                    type_definitions,
                    expression,
                )
            }
            _ => Ok(false),
        }
    }

    // Operators and indexing need a structural shape, and nominal values are compatible
    // with their representation type, so they project through nominal identity here. The
    // seen-set guards against recursive nominal representations.
    pub(super) fn resolve_structural(
        &mut self,
        core_type: CoreType,
        type_definitions: &TypeDefinitionEnvironment,
        expression: Option<&Expression>,
    ) -> Result<CoreType, InferenceError> {
        let mut resolved_type = self.resolve(core_type)?;
        let mut seen_nominals = BTreeSet::new();

        while let CoreType::Nominal(name, type_arguments) = &resolved_type {
            if !seen_nominals.insert(*name) {
                break;
            }
            let Some(representation_type) = self.nominal_representation_type(
                *name,
                type_arguments,
                type_definitions,
                expression,
            )?
            else {
                break;
            };
            resolved_type = self.resolve(representation_type)?;
        }

        Ok(resolved_type)
    }

    pub fn resolve(&mut self, core_type: CoreType) -> Result<CoreType, InferenceError> {
        if self.recursion_depth >= RECURSION_LIMIT {
            return Err(InferenceError::RecursionLimitExceeded);
        }
        self.recursion_depth += 1;
        let result = self.resolve_inner(core_type);
        self.recursion_depth -= 1;
        result
    }

    pub(super) fn resolve_inner(
        &mut self,
        core_type: CoreType,
    ) -> Result<CoreType, InferenceError> {
        match core_type {
            CoreType::Variable(variable) => self.resolve_variable(variable),
            CoreType::Union(members) => {
                let mut resolved_members = Vec::with_capacity(members.len());
                for member in members {
                    resolved_members.push(self.resolve(member)?);
                }
                // Re-normalize: members that resolved to equal types collapse, and a member that
                // resolved to a union flattens.
                Ok(CoreType::union_of(resolved_members))
            }
            CoreType::Nominal(symbol, type_arguments) => {
                let mut resolved_type_arguments = Vec::with_capacity(type_arguments.len());
                for type_argument in type_arguments {
                    resolved_type_arguments.push(self.resolve(type_argument)?);
                }
                Ok(CoreType::Nominal(symbol, resolved_type_arguments))
            }
            CoreType::Vector(element) => {
                let resolved_element = self.resolve(*element)?;
                Ok(CoreType::Vector(Box::new(resolved_element)))
            }
            CoreType::NamedVector(element) => {
                let resolved_element = self.resolve(*element)?;
                Ok(CoreType::NamedVector(Box::new(resolved_element)))
            }
            CoreType::List(item_type) => {
                let resolved_item_type = self.resolve(*item_type)?;
                Ok(CoreType::List(Box::new(resolved_item_type)))
            }
            CoreType::NamedList(item_type) => {
                let resolved_item_type = self.resolve(*item_type)?;
                Ok(CoreType::NamedList(Box::new(resolved_item_type)))
            }
            CoreType::Record(fields) => {
                let mut resolved_fields = Vec::with_capacity(fields.len());
                for field in fields {
                    resolved_fields.push(RecordField::with_optional(
                        field.name,
                        self.resolve(field.value)?,
                        field.optional,
                    ));
                }
                Ok(CoreType::Record(resolved_fields))
            }
            CoreType::Tuple(items) => {
                let mut resolved_items = Vec::with_capacity(items.len());
                for item in items {
                    resolved_items.push(self.resolve(item)?);
                }
                Ok(CoreType::Tuple(resolved_items))
            }
            CoreType::Function(function_type) => {
                let resolved_function_type = self.resolve_function_type(function_type)?;
                Ok(CoreType::Function(resolved_function_type))
            }
            other_type => Ok(other_type),
        }
    }

    pub fn free_type_variables(
        &mut self,
        core_type: &CoreType,
    ) -> Result<BTreeSet<InferenceVariableId>, InferenceError> {
        self.free_type_variables_in_core_type(core_type)
    }

    pub fn unify(&mut self, left: CoreType, right: CoreType) -> Result<CoreType, InferenceError> {
        self.unify_internal(left, right, None)
    }

    pub fn unify_with_context(
        &mut self,
        left: CoreType,
        right: CoreType,
        expression: &Expression,
    ) -> Result<CoreType, InferenceError> {
        self.unify_internal(left, right, Some(expression))
    }

    // Joins two control-flow results into one type. Types that unify share a representative — the
    // probe commits, which is what keeps the chooser idiom `if (c) a else b` linking two inference
    // variables — and genuinely different types fall back to their union, with the failed probe's
    // bindings rolled back so neither side is left constrained by the attempt. A recursion-limit
    // error is resource exhaustion, not a mismatch, so it propagates instead of producing a union.
    pub(super) fn join_types(
        &mut self,
        left: CoreType,
        right: CoreType,
        expression: &Expression,
    ) -> Result<CoreType, InferenceError> {
        let left = self.resolve(left)?;
        let right = self.resolve(right)?;

        // A `NULL` side joins by pure union, exactly like `if` without `else`: probing unification
        // first would bind an unconstrained inference variable on the other side to `NULL`,
        // collapsing the `T | NULL` results the nullable idioms rely on.
        if left == CoreType::Null || right == CoreType::Null {
            return Ok(CoreType::union_of(vec![left, right]));
        }

        let snapshot = self.snapshot();
        match self.unify_internal(left.clone(), right.clone(), Some(expression)) {
            Ok(unified_type) => {
                self.commit(snapshot);
                self.resolve(unified_type)
            }
            Err(InferenceError::RecursionLimitExceeded) => {
                self.rollback_to(snapshot);
                Err(InferenceError::RecursionLimitExceeded)
            }
            Err(_) => {
                self.rollback_to(snapshot);
                Ok(CoreType::union_of(vec![left, right]))
            }
        }
    }

    pub(super) fn unify_internal(
        &mut self,
        left: CoreType,
        right: CoreType,
        expression: Option<&Expression>,
    ) -> Result<CoreType, InferenceError> {
        if self.recursion_depth >= RECURSION_LIMIT {
            return Err(InferenceError::RecursionLimitExceeded);
        }
        self.recursion_depth += 1;
        let result = self.unify_internal_inner(left, right, expression);
        self.recursion_depth -= 1;
        result
    }

    pub(super) fn unify_internal_inner(
        &mut self,
        left: CoreType,
        right: CoreType,
        expression: Option<&Expression>,
    ) -> Result<CoreType, InferenceError> {
        let resolved_left = self.resolve(left)?;
        let resolved_right = self.resolve(right)?;

        match (resolved_left, resolved_right) {
            (CoreType::Variable(left_variable), CoreType::Variable(right_variable)) => {
                self.unify_variables(left_variable, right_variable)
            }
            (CoreType::Variable(variable), other_type)
            | (other_type, CoreType::Variable(variable)) => {
                self.bind_variable(variable, other_type.clone(), expression)?;
                Ok(other_type)
            }
            (CoreType::Any, other_type) | (other_type, CoreType::Any) => Ok(other_type),
            (CoreType::Unknown, other_type) | (other_type, CoreType::Unknown) => Ok(other_type),
            (CoreType::Null, CoreType::Null) => Ok(CoreType::Null),
            // Unification is the invariant floor and stays syntactic for unions: no member-wise
            // subtyping search happens here (that is `check_compatibility`'s job). Two unions unify
            // when their member sets are equal (order is presentation, not identity). The one
            // member-wise case kept is the nullable shape `T | NULL` vs `U | NULL` with exactly one
            // non-`NULL` member each — the pairing is unambiguous, and inferring through it is what
            // lets a `<T> ... T | NULL` scheme instantiate against a concrete nullable.
            (CoreType::Union(left_members), CoreType::Union(right_members)) => {
                let left_nullable_inner = nullable_single_member(&left_members);
                let right_nullable_inner = nullable_single_member(&right_members);
                if let (Some(left_inner), Some(right_inner)) =
                    (left_nullable_inner, right_nullable_inner)
                {
                    let unified = self.unify_internal(left_inner, right_inner, expression)?;
                    return Ok(CoreType::union_of(vec![unified, CoreType::Null]));
                }
                let sets_equal = left_members.len() == right_members.len()
                    && left_members
                        .iter()
                        .all(|member| right_members.contains(member));
                if sets_equal {
                    Ok(CoreType::Union(left_members))
                } else {
                    Err(InferenceError::TypeMismatch {
                        expected: Box::new(CoreType::Union(left_members)),
                        actual: Box::new(CoreType::Union(right_members)),
                        range: expression.map(|current_expression| current_expression.range),
                        expression_id: expression.map(|current_expression| current_expression.id),
                    })
                }
            }
            (
                CoreType::Nominal(left_name, left_arguments),
                CoreType::Nominal(right_name, right_arguments),
            ) if left_name == right_name && left_arguments.len() == right_arguments.len() => {
                // Unification is the invariant floor: it must produce a single representative type,
                // so every nominal argument is unified by equality regardless of the parameter's
                // compatibility variance. This is consistent with `check_compatibility` (unified ⇒
                // compatible in both directions): unify is strictly stronger than compatibility.
                let mut unified_arguments = Vec::with_capacity(left_arguments.len());
                for (left_argument, right_argument) in
                    left_arguments.into_iter().zip(right_arguments)
                {
                    unified_arguments.push(self.unify_internal(
                        left_argument,
                        right_argument,
                        expression,
                    )?);
                }
                Ok(CoreType::Nominal(left_name, unified_arguments))
            }
            (CoreType::Scalar(left_atomic), CoreType::Scalar(right_atomic))
                if left_atomic == right_atomic =>
            {
                Ok(CoreType::Scalar(left_atomic))
            }
            (CoreType::Vector(left_element), CoreType::Vector(right_element)) => {
                let unified_element =
                    self.unify_internal(*left_element, *right_element, expression)?;
                Ok(CoreType::Vector(Box::new(unified_element)))
            }
            (CoreType::NamedVector(left_element), CoreType::NamedVector(right_element)) => {
                let unified_element =
                    self.unify_internal(*left_element, *right_element, expression)?;
                Ok(CoreType::NamedVector(Box::new(unified_element)))
            }
            (CoreType::List(left_item_type), CoreType::List(right_item_type)) => {
                let unified_item_type =
                    self.unify_internal(*left_item_type, *right_item_type, expression)?;
                Ok(CoreType::List(Box::new(unified_item_type)))
            }
            (CoreType::NamedList(left_item_type), CoreType::NamedList(right_item_type)) => {
                let unified_item_type =
                    self.unify_internal(*left_item_type, *right_item_type, expression)?;
                Ok(CoreType::NamedList(Box::new(unified_item_type)))
            }
            (CoreType::Tuple(left_items), CoreType::Tuple(right_items)) => {
                self.unify_tuples(left_items, right_items, expression)
            }
            (CoreType::Record(left_fields), CoreType::Record(right_fields)) => {
                self.unify_records(left_fields, right_fields, expression)
            }
            (CoreType::Function(left_function), CoreType::Function(right_function)) => {
                let unified_function =
                    self.unify_functions(left_function, right_function, expression)?;
                Ok(CoreType::Function(unified_function))
            }
            (left_type, right_type) => Err(InferenceError::TypeMismatch {
                expected: Box::new(left_type),
                actual: Box::new(right_type),
                range: expression.map(|current_expression| current_expression.range),
                expression_id: expression.map(|current_expression| current_expression.id),
            }),
        }
    }

    pub fn occurs_in(
        &mut self,
        variable: InferenceVariableId,
        core_type: &CoreType,
    ) -> Result<bool, InferenceError> {
        match self.resolve(core_type.clone())? {
            CoreType::Variable(other_variable) => Ok(variable == other_variable),
            CoreType::Union(members) => {
                for member in members {
                    if self.occurs_in(variable, &member)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            CoreType::Nominal(_, type_arguments) => {
                for type_argument in type_arguments {
                    if self.occurs_in(variable, &type_argument)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            CoreType::Vector(element) => self.occurs_in(variable, &element),
            CoreType::NamedVector(element) => self.occurs_in(variable, &element),
            CoreType::List(item_type) => self.occurs_in(variable, &item_type),
            CoreType::NamedList(item_type) => self.occurs_in(variable, &item_type),
            CoreType::Record(fields) => {
                for field in fields {
                    if self.occurs_in(variable, &field.value)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            CoreType::Tuple(items) => {
                for item in items {
                    if self.occurs_in(variable, &item)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            CoreType::Function(function_type) => {
                for parameter in function_type.parameters {
                    if self.occurs_in(variable, &parameter)? {
                        return Ok(true);
                    }
                }

                for named_parameter in function_type.named_parameters {
                    if self.occurs_in(variable, &named_parameter.value)? {
                        return Ok(true);
                    }
                }

                if let Some(variadic) = &function_type.variadic
                    && self.occurs_in(variable, &variadic.element)?
                {
                    return Ok(true);
                }

                self.occurs_in(variable, &function_type.return_type)
            }
            _ => Ok(false),
        }
    }

    pub(super) fn instantiate_type_scheme(
        &mut self,
        type_scheme: &TypeScheme,
    ) -> Result<CoreType, InferenceError> {
        let mut substitutions = BTreeMap::new();

        for quantified in &type_scheme.quantified_variables {
            substitutions.insert(
                quantified.variable,
                self.fresh_constrained_variable(quantified.constraint),
            );
        }

        self.instantiate_core_type(&type_scheme.body, &substitutions)
    }

    pub(super) fn instantiate_core_type(
        &mut self,
        core_type: &CoreType,
        substitutions: &BTreeMap<InferenceVariableId, InferenceVariableId>,
    ) -> Result<CoreType, InferenceError> {
        match core_type {
            CoreType::Any => Ok(CoreType::Any),
            CoreType::Unknown => Ok(CoreType::Unknown),
            CoreType::Null => Ok(CoreType::Null),
            CoreType::Union(members) => {
                let mut instantiated_members = Vec::with_capacity(members.len());
                for member in members {
                    instantiated_members.push(self.instantiate_core_type(member, substitutions)?);
                }
                Ok(CoreType::union_of(instantiated_members))
            }
            CoreType::Scalar(atomic) => Ok(CoreType::Scalar(*atomic)),
            CoreType::Nominal(symbol, type_arguments) => {
                let mut instantiated_type_arguments = Vec::with_capacity(type_arguments.len());
                for type_argument in type_arguments {
                    instantiated_type_arguments
                        .push(self.instantiate_core_type(type_argument, substitutions)?);
                }
                Ok(CoreType::Nominal(*symbol, instantiated_type_arguments))
            }
            CoreType::Vector(element) => Ok(CoreType::Vector(Box::new(
                self.instantiate_core_type(element, substitutions)?,
            ))),
            CoreType::NamedVector(element) => Ok(CoreType::NamedVector(Box::new(
                self.instantiate_core_type(element, substitutions)?,
            ))),
            CoreType::List(item_type) => Ok(CoreType::List(Box::new(
                self.instantiate_core_type(item_type, substitutions)?,
            ))),
            CoreType::NamedList(item_type) => Ok(CoreType::NamedList(Box::new(
                self.instantiate_core_type(item_type, substitutions)?,
            ))),
            CoreType::Record(fields) => {
                let mut instantiated_fields = Vec::with_capacity(fields.len());
                for field in fields {
                    instantiated_fields.push(RecordField::with_optional(
                        field.name,
                        self.instantiate_core_type(&field.value, substitutions)?,
                        field.optional,
                    ));
                }
                Ok(CoreType::Record(instantiated_fields))
            }
            CoreType::Tuple(items) => {
                let mut instantiated_items = Vec::with_capacity(items.len());
                for item in items {
                    instantiated_items.push(self.instantiate_core_type(item, substitutions)?);
                }
                Ok(CoreType::Tuple(instantiated_items))
            }
            CoreType::Function(function_type) => {
                let mut instantiated_parameters =
                    Vec::with_capacity(function_type.parameters.len());
                for parameter in &function_type.parameters {
                    instantiated_parameters
                        .push(self.instantiate_core_type(parameter, substitutions)?);
                }

                let mut instantiated_named_parameters =
                    Vec::with_capacity(function_type.named_parameters.len());
                for named_parameter in &function_type.named_parameters {
                    instantiated_named_parameters.push(RecordField::with_optional(
                        named_parameter.name,
                        self.instantiate_core_type(&named_parameter.value, substitutions)?,
                        named_parameter.optional,
                    ));
                }

                let instantiated_variadic = match &function_type.variadic {
                    Some(variadic) => Some(RestParameter {
                        element: Box::new(
                            self.instantiate_core_type(&variadic.element, substitutions)?,
                        ),
                        preceding_named: variadic.preceding_named,
                    }),
                    None => None,
                };

                let instantiated_return_type =
                    self.instantiate_core_type(&function_type.return_type, substitutions)?;

                Ok(CoreType::Function(FunctionType::with_variadic(
                    instantiated_parameters,
                    instantiated_named_parameters,
                    instantiated_variadic,
                    instantiated_return_type,
                )))
            }
            CoreType::Variable(variable) => Ok(substitutions
                .get(variable)
                .copied()
                .map(CoreType::Variable)
                .unwrap_or(CoreType::Variable(*variable))),
        }
    }

    // Binds numeric-constrained variables reachable outside a function type to `double`. A numeric
    // variable only stays polymorphic when a function parameter abstracts it; anywhere else there
    // is no caller to choose the concrete numeric type, so it defaults like R's bare numbers.
    pub(super) fn default_free_numeric(
        &mut self,
        core_type: CoreType,
    ) -> Result<CoreType, InferenceError> {
        let resolved_type = self.resolve(core_type)?;
        match resolved_type {
            CoreType::Variable(variable) => {
                // Only default variables owned by the binding being finalized (created at a deeper
                // level). A numeric variable that escaped from an enclosing scope, such as an outer
                // function parameter referenced by a local binding, stays polymorphic until its own
                // boundary, matching the generalization level rule.
                if matches!(
                    self.entries.get(&variable),
                    Some(InferenceEntry::Unbound {
                        level,
                        constraint: Constraint::Numeric | Constraint::ScalarNumeric,
                    }) if *level > self.current_level
                ) {
                    self.bind_variable(variable, CoreType::Scalar(Atomic::Double), None)?;
                    return self.resolve(CoreType::Variable(variable));
                }
                Ok(CoreType::Variable(variable))
            }
            CoreType::Union(members) => {
                let mut defaulted_members = Vec::with_capacity(members.len());
                for member in members {
                    defaulted_members.push(self.default_free_numeric(member)?);
                }
                Ok(CoreType::union_of(defaulted_members))
            }
            CoreType::Vector(element) => Ok(CoreType::Vector(Box::new(
                self.default_free_numeric(*element)?,
            ))),
            CoreType::NamedVector(element) => Ok(CoreType::NamedVector(Box::new(
                self.default_free_numeric(*element)?,
            ))),
            CoreType::List(item_type) => Ok(CoreType::List(Box::new(
                self.default_free_numeric(*item_type)?,
            ))),
            CoreType::NamedList(item_type) => Ok(CoreType::NamedList(Box::new(
                self.default_free_numeric(*item_type)?,
            ))),
            CoreType::Record(fields) => {
                let mut defaulted_fields = Vec::with_capacity(fields.len());
                for field in fields {
                    defaulted_fields.push(RecordField::with_optional(
                        field.name,
                        self.default_free_numeric(field.value)?,
                        field.optional,
                    ));
                }
                Ok(CoreType::Record(defaulted_fields))
            }
            CoreType::Tuple(items) => {
                let mut defaulted_items = Vec::with_capacity(items.len());
                for item in items {
                    defaulted_items.push(self.default_free_numeric(item)?);
                }
                Ok(CoreType::Tuple(defaulted_items))
            }
            CoreType::Nominal(symbol, type_arguments) => {
                let mut defaulted_arguments = Vec::with_capacity(type_arguments.len());
                for type_argument in type_arguments {
                    defaulted_arguments.push(self.default_free_numeric(type_argument)?);
                }
                Ok(CoreType::Nominal(symbol, defaulted_arguments))
            }
            // Function parameter and return positions keep their numeric variables for
            // generalization, so descending into them would wrongly monomorphize them.
            other_type => Ok(other_type),
        }
    }

    // Quantifies the variables whose level is deeper than the current one: those were
    // created while inferring the binding's value and cannot escape it. Variables shared
    // with the enclosing scope were lowered to its level when they were unified, so no
    // environment walk is needed.
    pub(super) fn generalize(&mut self, core_type: CoreType) -> Result<TypeScheme, InferenceError> {
        let resolved_type = self.resolve(core_type)?;
        let type_variables = self.free_type_variables_in_core_type(&resolved_type)?;

        let mut quantified_variables = Vec::new();
        for variable in type_variables {
            let Some(entry) = self.entries.get(&variable) else {
                return Err(InferenceError::UnknownInferenceVariable(variable));
            };
            if let InferenceEntry::Unbound { level, constraint } = entry
                && *level > self.current_level
            {
                quantified_variables.push(QuantifiedVariable::new(variable, *constraint));
            }
        }

        Ok(TypeScheme {
            quantified_variables,
            body: resolved_type,
        })
    }

    pub(super) fn free_type_variables_in_core_type(
        &mut self,
        core_type: &CoreType,
    ) -> Result<BTreeSet<InferenceVariableId>, InferenceError> {
        if self.recursion_depth >= RECURSION_LIMIT {
            return Err(InferenceError::RecursionLimitExceeded);
        }
        self.recursion_depth += 1;
        let result = self.free_type_variables_in_core_type_inner(core_type);
        self.recursion_depth -= 1;
        result
    }

    pub(super) fn free_type_variables_in_core_type_inner(
        &mut self,
        core_type: &CoreType,
    ) -> Result<BTreeSet<InferenceVariableId>, InferenceError> {
        match self.resolve(core_type.clone())? {
            CoreType::Any | CoreType::Unknown | CoreType::Null | CoreType::Scalar(_) => {
                Ok(BTreeSet::new())
            }
            CoreType::Vector(element) | CoreType::NamedVector(element) => {
                self.free_type_variables_in_core_type(&element)
            }
            CoreType::Union(members) => {
                let mut free_variables = BTreeSet::new();
                for member in members {
                    free_variables.extend(self.free_type_variables_in_core_type(&member)?);
                }
                Ok(free_variables)
            }
            CoreType::Variable(variable) => Ok(BTreeSet::from([variable])),
            CoreType::Nominal(_, type_arguments) => {
                let mut free_variables = BTreeSet::new();
                for type_argument in type_arguments {
                    free_variables.extend(self.free_type_variables_in_core_type(&type_argument)?);
                }
                Ok(free_variables)
            }
            CoreType::List(item_type) => self.free_type_variables_in_core_type(&item_type),
            CoreType::NamedList(item_type) => self.free_type_variables_in_core_type(&item_type),
            CoreType::Record(fields) => {
                let mut free_variables = BTreeSet::new();
                for field in fields {
                    free_variables.extend(self.free_type_variables_in_core_type(&field.value)?);
                }
                Ok(free_variables)
            }
            CoreType::Tuple(items) => {
                let mut free_variables = BTreeSet::new();
                for item in items {
                    free_variables.extend(self.free_type_variables_in_core_type(&item)?);
                }
                Ok(free_variables)
            }
            CoreType::Function(function_type) => {
                let mut free_variables = BTreeSet::new();

                for parameter in function_type.parameters {
                    free_variables.extend(self.free_type_variables_in_core_type(&parameter)?);
                }

                for named_parameter in function_type.named_parameters {
                    free_variables
                        .extend(self.free_type_variables_in_core_type(&named_parameter.value)?);
                }

                if let Some(variadic) = &function_type.variadic {
                    free_variables
                        .extend(self.free_type_variables_in_core_type(&variadic.element)?);
                }

                free_variables
                    .extend(self.free_type_variables_in_core_type(&function_type.return_type)?);

                Ok(free_variables)
            }
        }
    }

    pub(super) fn resolve_function_type(
        &mut self,
        function_type: FunctionType<CoreType>,
    ) -> Result<FunctionType<CoreType>, InferenceError> {
        let mut resolved_parameters = Vec::with_capacity(function_type.parameters.len());
        for parameter in function_type.parameters {
            resolved_parameters.push(self.resolve(parameter)?);
        }

        let mut resolved_named_parameters =
            Vec::with_capacity(function_type.named_parameters.len());
        for named_parameter in function_type.named_parameters {
            resolved_named_parameters.push(RecordField::with_optional(
                named_parameter.name,
                self.resolve(named_parameter.value)?,
                named_parameter.optional,
            ));
        }

        let resolved_variadic = match function_type.variadic {
            Some(variadic) => Some(RestParameter {
                element: Box::new(self.resolve(*variadic.element)?),
                preceding_named: variadic.preceding_named,
            }),
            None => None,
        };

        let resolved_return_type = self.resolve(*function_type.return_type)?;

        Ok(FunctionType::with_variadic(
            resolved_parameters,
            resolved_named_parameters,
            resolved_variadic,
            resolved_return_type,
        ))
    }

    pub(super) fn unify_tuples(
        &mut self,
        left_items: Vec<CoreType>,
        right_items: Vec<CoreType>,
        expression: Option<&Expression>,
    ) -> Result<CoreType, InferenceError> {
        if left_items.len() != right_items.len() {
            return Err(InferenceError::TupleLengthMismatch {
                expected: left_items.len(),
                actual: right_items.len(),
                range: expression.map(|current_expression| current_expression.range),
                expression_id: expression.map(|current_expression| current_expression.id),
            });
        }

        let mut unified_items = Vec::with_capacity(left_items.len());
        for (left_item, right_item) in left_items.into_iter().zip(right_items) {
            unified_items.push(self.unify_internal(left_item, right_item, expression)?);
        }

        Ok(CoreType::Tuple(unified_items))
    }

    pub(super) fn unify_records(
        &mut self,
        left_fields: Vec<RecordField<CoreType>>,
        right_fields: Vec<RecordField<CoreType>>,
        expression: Option<&Expression>,
    ) -> Result<CoreType, InferenceError> {
        let left_names: BTreeSet<_> = left_fields.iter().map(|field| field.name).collect();
        let right_names: BTreeSet<_> = right_fields.iter().map(|field| field.name).collect();

        if left_names != right_names {
            return Err(InferenceError::RecordFieldMismatch {
                expected_fields: left_names.into_iter().collect(),
                actual_fields: right_names.into_iter().collect(),
                range: expression.map(|current_expression| current_expression.range),
                expression_id: expression.map(|current_expression| current_expression.id),
            });
        }

        let right_by_name: BTreeMap<_, _> = right_fields
            .into_iter()
            .map(|field| (field.name, field.value))
            .collect();

        let mut unified_fields = Vec::with_capacity(left_fields.len());
        for left_field in left_fields {
            let Some(right_value) = right_by_name.get(&left_field.name).cloned() else {
                return Err(InferenceError::RecordFieldMismatch {
                    expected_fields: vec![left_field.name],
                    actual_fields: Vec::new(),
                    range: expression.map(|current_expression| current_expression.range),
                    expression_id: expression.map(|current_expression| current_expression.id),
                });
            };

            let unified_value = self.unify_internal(left_field.value, right_value, expression)?;
            // Keep the left interface's optionality, like `unify_functions` keeps the left
            // parameter names: record syntax cannot declare optional fields today, but the shared
            // `RecordField` carrier must not silently drop the flag if that changes.
            unified_fields.push(RecordField::with_optional(
                left_field.name,
                unified_value,
                left_field.optional,
            ));
        }

        Ok(CoreType::Record(unified_fields))
    }

    // Parameters unify positionally across the flattened positional-then-named parameter
    // list: parameter names describe the call interface, not the identity of the function
    // type, so `fn(integer) -> NULL` and `fn(count: integer) -> NULL` unify. The left
    // function's interface (names and positional split) is kept for the result.
    pub(super) fn unify_functions(
        &mut self,
        left_function: FunctionType<CoreType>,
        right_function: FunctionType<CoreType>,
        expression: Option<&Expression>,
    ) -> Result<FunctionType<CoreType>, InferenceError> {
        let left_total = left_function.parameters.len() + left_function.named_parameters.len();
        let right_total = right_function.parameters.len() + right_function.named_parameters.len();
        // A variadic function accepts a caller shape a fixed function does not, so the two are never the
        // same type. Treat a variadic/fixed mismatch as an arity mismatch (the rest parameter counts as
        // one interface slot the other side lacks). The rest parameter's formal position is part of
        // the interface too — it decides which parameters fill positionally — so differing positions
        // also fail here.
        let rest_positions_differ = match (&left_function.variadic, &right_function.variadic) {
            (Some(left_variadic), Some(right_variadic)) => {
                left_variadic.preceding_named != right_variadic.preceding_named
            }
            (None, None) => false,
            _ => true,
        };
        if left_total != right_total || rest_positions_differ {
            return Err(InferenceError::FunctionArityMismatch {
                expected: left_total + usize::from(left_function.variadic.is_some()),
                actual: right_total + usize::from(right_function.variadic.is_some()),
                range: expression.map(|current_expression| current_expression.range),
                expression_id: expression.map(|current_expression| current_expression.id),
            });
        }

        let mut right_parameter_types = right_function.parameters;
        right_parameter_types.extend(
            right_function
                .named_parameters
                .into_iter()
                .map(|parameter| parameter.value),
        );
        let mut right_parameter_iter = right_parameter_types.into_iter();

        let mut unified_parameters = Vec::with_capacity(left_function.parameters.len());
        for left_parameter in left_function.parameters {
            let right_parameter = right_parameter_iter
                .next()
                .expect("parameter totals were checked to match");
            unified_parameters.push(self.unify_internal(
                left_parameter,
                right_parameter,
                expression,
            )?);
        }

        let mut unified_named_parameters = Vec::with_capacity(left_function.named_parameters.len());
        for left_named_parameter in left_function.named_parameters {
            let right_parameter = right_parameter_iter
                .next()
                .expect("parameter totals were checked to match");
            let unified_value =
                self.unify_internal(left_named_parameter.value, right_parameter, expression)?;
            unified_named_parameters.push(RecordField::with_optional(
                left_named_parameter.name,
                unified_value,
                left_named_parameter.optional,
            ));
        }

        let unified_variadic = match (left_function.variadic, right_function.variadic) {
            (Some(left_variadic), Some(right_variadic)) => Some(RestParameter {
                element: Box::new(self.unify_internal(
                    *left_variadic.element,
                    *right_variadic.element,
                    expression,
                )?),
                // Positions were checked equal above; keep the left interface like the names.
                preceding_named: left_variadic.preceding_named,
            }),
            // Presence was checked to match above, so only the both-absent case remains here.
            _ => None,
        };

        let unified_return_type = self.unify_internal(
            *left_function.return_type,
            *right_function.return_type,
            expression,
        )?;

        Ok(FunctionType::with_variadic(
            unified_parameters,
            unified_named_parameters,
            unified_variadic,
            unified_return_type,
        ))
    }

    pub(super) fn resolve_variable(
        &mut self,
        variable: InferenceVariableId,
    ) -> Result<CoreType, InferenceError> {
        let Some(entry) = self.entries.get(&variable).cloned() else {
            return Err(InferenceError::UnknownInferenceVariable(variable));
        };

        match entry {
            InferenceEntry::Unbound { .. } => Ok(CoreType::Variable(variable)),
            InferenceEntry::Redirect(other_variable) => {
                let resolved_type = self.resolve_variable(other_variable)?;
                self.compress_variable(variable, &resolved_type)?;
                Ok(resolved_type)
            }
            InferenceEntry::Bound(bound_type) => {
                let resolved_type = self.resolve(bound_type)?;
                self.compress_variable(variable, &resolved_type)?;
                Ok(resolved_type)
            }
        }
    }

    pub(super) fn compress_variable(
        &mut self,
        variable: InferenceVariableId,
        resolved_type: &CoreType,
    ) -> Result<(), InferenceError> {
        if !self.entries.contains_key(&variable) {
            return Err(InferenceError::UnknownInferenceVariable(variable));
        }

        let compressed_entry = match resolved_type {
            CoreType::Variable(other_variable) if *other_variable != variable => {
                InferenceEntry::Redirect(*other_variable)
            }
            other_type => InferenceEntry::Bound(other_type.clone()),
        };
        self.set_entry(variable, compressed_entry);

        Ok(())
    }

    pub(super) fn bind_variable(
        &mut self,
        variable: InferenceVariableId,
        core_type: CoreType,
        expression: Option<&Expression>,
    ) -> Result<(), InferenceError> {
        if self.occurs_in(variable, &core_type)? {
            return Err(InferenceError::OccursCheckFailed {
                variable,
                in_type: Box::new(core_type),
                range: expression.map(|current_expression| current_expression.range),
                expression_id: expression.map(|current_expression| current_expression.id),
            });
        }

        // A rigid (skolem) variable models a universally quantified annotation parameter; binding it
        // to a concrete type would specialize a `<T>` the body promised to handle for every T.
        if self.rigid_variables.contains_key(&variable) {
            return Err(InferenceError::TypeMismatch {
                expected: Box::new(self.rigid_display(variable)),
                actual: Box::new(core_type),
                range: expression.map(|current| current.range),
                expression_id: expression.map(|current| current.id),
            });
        }

        let Some(entry) = self.entries.get(&variable).cloned() else {
            return Err(InferenceError::UnknownInferenceVariable(variable));
        };
        if let InferenceEntry::Unbound { level, constraint } = entry {
            // A constrained variable may only be bound to a type that satisfies its bound. When
            // the bound type is itself a variable the constraint propagates there instead, and
            // when it is concrete and unsatisfying `constrain_type` reports the violation.
            self.constrain_type(core_type.clone(), constraint, expression)?;
            // Anything reachable from the bound type escapes to this variable's scope, so
            // inner variables drop to its level and stay monomorphic there.
            self.lower_levels_to(&core_type, level)?;
        }

        if !self.entries.contains_key(&variable) {
            return Err(InferenceError::UnknownInferenceVariable(variable));
        }
        self.set_entry(variable, InferenceEntry::Bound(core_type));
        Ok(())
    }

    pub(super) fn lower_levels_to(
        &mut self,
        core_type: &CoreType,
        level: Level,
    ) -> Result<(), InferenceError> {
        for variable in self.free_type_variables_in_core_type(core_type)? {
            let lowered_entry = match self.entries.get(&variable) {
                None => return Err(InferenceError::UnknownInferenceVariable(variable)),
                Some(InferenceEntry::Unbound {
                    level: variable_level,
                    constraint,
                }) if *variable_level > level => Some(InferenceEntry::Unbound {
                    level,
                    constraint: *constraint,
                }),
                Some(_) => None,
            };
            if let Some(entry) = lowered_entry {
                self.set_entry(variable, entry);
            }
        }
        Ok(())
    }

    pub(super) fn unify_variables(
        &mut self,
        left: InferenceVariableId,
        right: InferenceVariableId,
    ) -> Result<CoreType, InferenceError> {
        if left == right {
            return Ok(CoreType::Variable(left));
        }

        let Some(left_entry) = self.entries.get(&left) else {
            return Err(InferenceError::UnknownInferenceVariable(left));
        };
        if !matches!(left_entry, InferenceEntry::Unbound { .. }) {
            let resolved_left = self.resolve_variable(left)?;
            let resolved_right = self.resolve_variable(right)?;
            return self.unify(resolved_left, resolved_right);
        }

        let Some(right_entry) = self.entries.get(&right) else {
            return Err(InferenceError::UnknownInferenceVariable(right));
        };
        if !matches!(right_entry, InferenceEntry::Unbound { .. }) {
            let resolved_left = self.resolve_variable(left)?;
            let resolved_right = self.resolve_variable(right)?;
            return self.unify(resolved_left, resolved_right);
        }

        // Two distinct skolems are different universals and cannot be unified. When exactly one side
        // is rigid it must survive the union so its identity (and rigidity) is preserved; the
        // flexible variable redirects to it.
        let left_rigid = self.rigid_variables.contains_key(&left);
        let right_rigid = self.rigid_variables.contains_key(&right);
        if left_rigid && right_rigid {
            return Err(InferenceError::TypeMismatch {
                expected: Box::new(self.rigid_display(left)),
                actual: Box::new(self.rigid_display(right)),
                range: None,
                expression_id: None,
            });
        }
        let (survivor, redirected) = if left_rigid {
            (left, right)
        } else {
            (right, left)
        };

        let (redirected_level, redirected_constraint) = match self.entries.get(&redirected) {
            Some(InferenceEntry::Unbound { level, constraint }) => (*level, *constraint),
            _ => return Err(InferenceError::UnknownInferenceVariable(redirected)),
        };
        let merged_survivor = match self.entries.get(&survivor) {
            Some(InferenceEntry::Unbound { level, constraint }) => Some(InferenceEntry::Unbound {
                level: (*level).min(redirected_level),
                constraint: (*constraint).join(redirected_constraint),
            }),
            _ => None,
        };
        if let Some(entry) = merged_survivor {
            self.set_entry(survivor, entry);
        }

        if !self.entries.contains_key(&redirected) {
            return Err(InferenceError::UnknownInferenceVariable(redirected));
        }
        self.set_entry(redirected, InferenceEntry::Redirect(survivor));

        Ok(CoreType::Variable(survivor))
    }

    // Interface schemes computed by another `InferenceState` carry variable ids that mean
    // nothing here, so importing re-binds quantified variables to fresh local ids and erases
    // any stray free variable to `Unknown`.
    pub fn import_scheme(&mut self, type_scheme: &TypeScheme) -> TypeScheme {
        let mut substitutions = BTreeMap::new();
        let mut quantified_variables = Vec::with_capacity(type_scheme.quantified_variables.len());
        for quantified in &type_scheme.quantified_variables {
            let fresh = self.fresh_constrained_variable(quantified.constraint);
            substitutions.insert(quantified.variable, fresh);
            quantified_variables.push(QuantifiedVariable::new(fresh, quantified.constraint));
        }

        TypeScheme {
            quantified_variables,
            body: import_core_type(&type_scheme.body, &substitutions),
        }
    }
}
