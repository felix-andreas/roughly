// Call checking: the call-expression entry points, R's argument matcher (name-aware, positional,
// and rest-parameter matching), ordered overload probing, the callback-forwarding probe behind
// `lapply(x, f, ...)`-style call sites, and the argument compatibility check. The typing-reference
// sections on function calls, overload sets, and callback forwarding are the contract.
use {
    super::{
        InferenceEntry, InferenceError, InferenceState, ResolutionContext,
        operand::{callee_overload_symbol, erase_variables, is_whole_number_double_literal},
    },
    crate::{
        hir::{Argument, Expression, HirArena},
        interner::Symbol,
        typecheck::TypeDefinitionEnvironment,
        types::{Atomic, Constraint, CoreType, FunctionType, RecordField, TypeScheme},
    },
};

impl InferenceState {
    pub(super) fn infer_function_call_expression(
        &mut self,
        callee: &Expression,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        // An overloaded stub callee resolves per call site: each scheme is probed in declaration
        // order and the first whose parameters accept the arguments wins (its return is the call's
        // type). Only a plain or namespace-qualified name can be overloaded, and a local binding
        // shadowing the name disables the set (the local wins, as everywhere).
        if let Some(overload_symbol) = callee_overload_symbol(callee, resolution_context)
            && let Some(schemes) = self.overload_sets.get(&overload_symbol).cloned()
            && schemes.len() > 1
        {
            return self.infer_overloaded_call(
                overload_symbol,
                &schemes,
                arguments,
                callee,
                expression,
                arena,
                resolution_context,
                type_definitions,
            );
        }

        let inferred_callee = self.infer_expression_with_context(
            callee,
            arena,
            resolution_context,
            type_definitions,
        )?;
        let resolved_callee = self.resolve(inferred_callee)?;

        match resolved_callee {
            CoreType::Unknown => Ok(CoreType::Unknown),
            CoreType::Any => Ok(CoreType::Any),
            CoreType::Variable(variable) => {
                let mut positional_arguments = Vec::new();
                let mut named_arguments = Vec::new();

                for argument in arguments {
                    let inferred_argument = self.infer_expression_with_context(
                        arena.get(argument.expression),
                        arena,
                        resolution_context,
                        type_definitions,
                    )?;
                    if let Some(name) = argument.name {
                        named_arguments.push(RecordField::new(name, inferred_argument));
                    } else {
                        positional_arguments.push(inferred_argument);
                    }
                }

                let return_variable = self.fresh_variable();
                self.unify_with_context(
                    CoreType::Variable(variable),
                    CoreType::Function(FunctionType::new(
                        positional_arguments,
                        named_arguments,
                        CoreType::Variable(return_variable),
                    )),
                    expression,
                )?;
                self.resolve(CoreType::Variable(return_variable))
            }
            CoreType::Function(function_type) => self.infer_function_call(
                function_type,
                arguments,
                callee,
                expression,
                arena,
                resolution_context,
                type_definitions,
            ),
            // A call through a union of functions — the dispatch-table idiom,
            // `handlers[[name]](...)` — must be valid for every member, since the value could be
            // any of them. Each member's signature is probed against the arguments in an isolated
            // snapshot and the call's type is the union of the member returns; returns are
            // variable-erased because the probe bindings that produced them roll back.
            CoreType::Union(members)
                if members
                    .iter()
                    .all(|member| matches!(member, CoreType::Function(_))) =>
            {
                let mut return_types = Vec::with_capacity(members.len());
                for member in members {
                    let CoreType::Function(function_type) = member else {
                        unreachable!("guarded by the all-functions match arm condition");
                    };
                    let snapshot = self.snapshot();
                    let member_return = self.infer_function_call(
                        function_type,
                        arguments,
                        callee,
                        expression,
                        arena,
                        resolution_context,
                        type_definitions,
                    );
                    match member_return {
                        Ok(return_type) => {
                            let resolved = self.resolve(return_type)?;
                            self.rollback_to(snapshot);
                            return_types.push(erase_variables(resolved));
                        }
                        Err(error) => {
                            self.rollback_to(snapshot);
                            return Err(error);
                        }
                    }
                }
                Ok(CoreType::union_of(return_types))
            }
            other_type => Err(InferenceError::ExpectedFunction {
                actual_type: Box::new(other_type),
                range: callee.range,
                expression_id: callee.id,
            }),
        }
    }

    // Probes each scheme of an overloaded name in declaration order and commits the first one whose
    // signature accepts the arguments; its return type is the call's type. Arguments are inferred
    // exactly once, before any probe: expression inference writes fields the probe snapshot does not
    // reverse (`environment`, `recorded_expression_types`), so running it inside a probe would leak
    // bindings that reference rolled-back variable ids. The probes themselves run only the
    // instantiation and argument-matching paths, which stay within the snapshot contract.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn infer_overloaded_call(
        &mut self,
        symbol: Symbol,
        schemes: &[TypeScheme],
        arguments: &[Argument],
        callee: &Expression,
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        let argument_types =
            self.infer_call_arguments(arguments, arena, resolution_context, type_definitions)?;

        // Selection needs concrete argument types. Probing against an argument whose type still
        // contains a free inference variable would let the first candidate bind it — committing a
        // wrapper function's parameter (`function(x) sum(x)`) to the first scheme's parameter type
        // and rejecting calls R accepts. Such a call skips selection and uses the final
        // declaration, by corpus convention the most general one.
        let mut has_unresolved_argument = false;
        for argument_type in &argument_types {
            if !self.free_type_variables(argument_type)?.is_empty() {
                has_unresolved_argument = true;
                break;
            }
        }
        let declared_count = schemes.len();
        let schemes = match (has_unresolved_argument, schemes.split_last()) {
            (true, Some((last, _))) => std::slice::from_ref(last),
            _ => schemes,
        };
        // Maps a probe index back into the declared set: the unresolved-argument fallback probes
        // only the final declaration, so its one candidate is the set's last index.
        let declared_index = |probe_index: usize| {
            if schemes.len() == declared_count {
                probe_index
            } else {
                declared_count - 1
            }
        };

        // Selection runs strict first, then (only if nothing matched and a whole-number double
        // literal is present) once more with the literal-as-integer courtesy. During the strict
        // round the courtesy is off (`overload_probe_depth`): `1` is genuinely a double at runtime,
        // so letting it match an integer candidate would pick a signature whose return type
        // misstates what R computes (`sum(1, 2)` is a double, not an integer). The courtesy round
        // keeps a name whose only fitting candidate wants `integer` callable as `foo(1)` — exact
        // matches outrank conversions.
        let literal_courtesy_rounds: &[bool] = if arguments
            .iter()
            .any(|argument| is_whole_number_double_literal(arena.get(argument.expression)))
        {
            &[false, true]
        } else {
            &[false]
        };

        let mut first_error = None;
        for &allow_literal_courtesy in literal_courtesy_rounds {
            for (probe_index, scheme) in schemes.iter().enumerate() {
                let snapshot = self.snapshot();
                let function_type = match self
                    .instantiate_type_scheme(scheme)
                    .and_then(|instantiated| self.resolve(instantiated))
                {
                    Ok(CoreType::Function(function_type)) => function_type,
                    Ok(_) => {
                        self.rollback_to(snapshot);
                        continue;
                    }
                    Err(error) => {
                        self.rollback_to(snapshot);
                        return Err(error);
                    }
                };
                if !allow_literal_courtesy {
                    self.overload_probe_depth += 1;
                }
                let outcome = self.match_call_arguments(
                    function_type,
                    arguments,
                    &argument_types,
                    callee,
                    expression,
                    arena,
                    type_definitions,
                );
                if !allow_literal_courtesy {
                    self.overload_probe_depth -= 1;
                }
                match outcome {
                    Ok(result) => {
                        self.commit(snapshot);
                        if self.record_expression_types {
                            self.selected_overloads
                                .insert(callee.id, declared_index(probe_index));
                        }
                        return Ok(result);
                    }
                    Err(InferenceError::RecursionLimitExceeded) => {
                        self.rollback_to(snapshot);
                        return Err(InferenceError::RecursionLimitExceeded);
                    }
                    Err(error) => {
                        self.rollback_to(snapshot);
                        if first_error.is_none() {
                            first_error = Some(error);
                        }
                    }
                }
            }
        }

        // The unresolved-argument fallback probes a single scheme; failing it is an ordinary call
        // mismatch, so the underlying error reads better than a one-candidate overload report.
        if schemes.len() == 1
            && let Some(error) = first_error
        {
            return Err(error);
        }

        Err(InferenceError::NoMatchingOverload {
            symbol,
            candidate_count: schemes.len(),
            range: expression.range,
            expression_id: expression.id,
            first_error: first_error.map(Box::new),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn infer_function_call(
        &mut self,
        function_type: FunctionType<CoreType>,
        arguments: &[Argument],
        callee: &Expression,
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        let argument_types =
            self.infer_call_arguments(arguments, arena, resolution_context, type_definitions)?;
        self.match_call_arguments(
            function_type,
            arguments,
            &argument_types,
            callee,
            expression,
            arena,
            type_definitions,
        )
    }

    pub(super) fn infer_call_arguments(
        &mut self,
        arguments: &[Argument],
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<Vec<CoreType>, InferenceError> {
        arguments
            .iter()
            .map(|argument| {
                self.infer_expression_with_context(
                    arena.get(argument.expression),
                    arena,
                    resolution_context,
                    type_definitions,
                )
            })
            .collect()
    }

    // Matches already-inferred argument types against a concrete signature: positionals in order,
    // named arguments by name, surplus positionals into optional named parameters or `...`. Kept
    // free of expression inference so an overload probe can run it inside a snapshot (see
    // `infer_overloaded_call`). `argument_types` is parallel to `arguments`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn match_call_arguments(
        &mut self,
        function_type: FunctionType<CoreType>,
        arguments: &[Argument],
        argument_types: &[CoreType],
        callee: &Expression,
        expression: &Expression,
        arena: &HirArena,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        let total_parameters =
            function_type.parameters.len() + function_type.named_parameters.len();
        let required_parameters = function_type.parameters.len()
            + function_type
                .named_parameters
                .iter()
                .filter(|parameter| !parameter.optional)
                .count();
        let expected_named_parameters = function_type
            .named_parameters
            .iter()
            .map(|parameter| parameter.name)
            .collect::<Vec<_>>();
        let actual_named_arguments = arguments
            .iter()
            .filter_map(|argument| argument.name)
            .collect::<Vec<_>>();

        let positional_parameters = function_type.parameters;
        let variadic_element = function_type
            .variadic
            .as_ref()
            .map(|variadic| (*variadic.element).clone());
        // Named parameters declared before the rest parameter fill positionally, exactly as R
        // fills formals before `...`. Removals keep declaration order, so the pre-rest parameters
        // are always the front segment of the remaining list and this count tracks them.
        let mut pre_rest_remaining = match &function_type.variadic {
            Some(variadic) => variadic.preceding_named,
            None => function_type.named_parameters.len(),
        };
        let return_type = *function_type.return_type;
        let mut next_positional_index = 0;
        let mut remaining_named_parameters = function_type.named_parameters;

        // Which arguments the rest parameter will absorb, decided up front with the same
        // accounting the loop below applies (no type checks). Needed before the loop because a
        // function-typed argument earlier in the call may be checked against the arguments
        // forwarded to it later in the call (`lapply(x, gsub, pattern = "a")`).
        let forwarded_argument_indexes = if variadic_element.is_some() {
            let mut consumed_named = Vec::new();
            let mut positional_seen = 0usize;
            let mut pre_rest_slots = pre_rest_remaining;
            let mut forwarded = Vec::new();
            for (index, argument) in arguments.iter().enumerate() {
                match argument.name {
                    Some(name) => {
                        let declared_index =
                            expected_named_parameters.iter().position(|expected| {
                                *expected == name && !consumed_named.contains(&name)
                            });
                        match declared_index {
                            Some(declared_index) => {
                                consumed_named.push(name);
                                if declared_index < pre_rest_slots {
                                    pre_rest_slots -= 1;
                                }
                            }
                            None => forwarded.push(index),
                        }
                    }
                    None => {
                        if positional_seen < positional_parameters.len() {
                            positional_seen += 1;
                        } else if pre_rest_slots > 0 {
                            pre_rest_slots -= 1;
                        } else {
                            forwarded.push(index);
                        }
                    }
                }
            }
            forwarded
        } else {
            Vec::new()
        };

        for (argument, inferred_argument) in arguments.iter().zip(argument_types) {
            let arg_expr = arena.get(argument.expression);
            let inferred_argument = inferred_argument.clone();
            if let Some(name) = argument.name {
                let Some(parameter_index) = remaining_named_parameters
                    .iter()
                    .position(|parameter| parameter.name == name)
                else {
                    // A named argument matching no declared parameter is absorbed by the rest
                    // parameter, checked against its element type (R collects unmatched keywords
                    // into `...` — the pass-through idiom `read.csv(f, colClasses = ...)`). A name
                    // that *duplicates* a declared parameter already given stays an error (R:
                    // "formal argument matched by multiple actual arguments"), and without a rest
                    // parameter an unmatched name is an error as before.
                    if let Some(element) = &variadic_element
                        && !expected_named_parameters.contains(&name)
                    {
                        self.check_argument(
                            element.clone(),
                            inferred_argument,
                            arg_expr,
                            type_definitions,
                        )?;
                        continue;
                    }
                    return Err(InferenceError::NamedParameterMismatch {
                        expected_parameters: expected_named_parameters,
                        actual_parameters: actual_named_arguments,
                        range: Some(expression.range),
                        expression_id: Some(expression.id),
                    });
                };

                let parameter = remaining_named_parameters.remove(parameter_index);
                if parameter_index < pre_rest_remaining {
                    pre_rest_remaining -= 1;
                }
                if let Err(error) = self.check_argument(
                    parameter.value.clone(),
                    inferred_argument.clone(),
                    arg_expr,
                    type_definitions,
                ) && !self.forwarding_callback_probe(
                    &parameter.value,
                    &inferred_argument,
                    &forwarded_argument_indexes,
                    arguments,
                    argument_types,
                    arg_expr,
                    arena,
                    type_definitions,
                )? {
                    return Err(error);
                }
                continue;
            }

            if let Some(parameter) = positional_parameters.get(next_positional_index) {
                next_positional_index += 1;
                self.check_argument(
                    parameter.clone(),
                    inferred_argument,
                    arg_expr,
                    type_definitions,
                )?;
                continue;
            }

            // A positional argument past the fixed positionals fills the next named parameter
            // declared before the rest parameter (all of them, when the function is not variadic) —
            // R's rule: formals before `...` fill positionally, formals after it by name only.
            if pre_rest_remaining > 0 {
                let parameter = remaining_named_parameters.remove(0);
                pre_rest_remaining -= 1;
                if let Err(error) = self.check_argument(
                    parameter.value.clone(),
                    inferred_argument.clone(),
                    arg_expr,
                    type_definitions,
                ) && !self.forwarding_callback_probe(
                    &parameter.value,
                    &inferred_argument,
                    &forwarded_argument_indexes,
                    arguments,
                    argument_types,
                    arg_expr,
                    arena,
                    type_definitions,
                )? {
                    return Err(error);
                }
                continue;
            }

            // A variadic function absorbs any number of surplus positional arguments, each checked
            // against the rest-parameter element type. Cloning the element per argument keeps the check
            // order-independent — no argument's check mutates state a later one reads.
            if let Some(element) = &variadic_element {
                self.check_argument(
                    element.clone(),
                    inferred_argument,
                    arg_expr,
                    type_definitions,
                )?;
                continue;
            }

            return Err(InferenceError::FunctionArityMismatch {
                expected: total_parameters,
                actual: arguments.len(),
                range: Some(callee.range),
                expression_id: Some(callee.id),
            });
        }

        if next_positional_index != positional_parameters.len()
            || remaining_named_parameters
                .iter()
                .any(|parameter| !parameter.optional)
        {
            return Err(InferenceError::FunctionArityMismatch {
                expected: required_parameters,
                actual: arguments.len(),
                range: Some(callee.range),
                expression_id: Some(callee.id),
            });
        }

        self.resolve(return_type)
    }

    // The forwarding retry for a callback argument of a variadic callee. R's apply family invokes
    // `FUN(element, ...)`, so a callback with more formals than the declared interface is still
    // correct when the caller forwards the difference — `lapply(x, gsub, pattern = "a",
    // replacement = "o")` calls `gsub(x[[i]], pattern = "a", replacement = "o")`, and formals the
    // forwarding leaves unfilled may default. When the plain interface check fails, this simulates
    // that invocation against the callback's real signature: forwarded named arguments consume
    // same-named formals, the interface's parameter types then fill the remaining formals in order
    // together with forwarded positionals, leftovers must be optional, and the callback's return
    // must satisfy the interface's. Runs as a probe: bindings commit only on success, and a failed
    // probe reports the original interface mismatch, not the simulation's.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn forwarding_callback_probe(
        &mut self,
        expected_parameter: &CoreType,
        actual_argument: &CoreType,
        forwarded_argument_indexes: &[usize],
        arguments: &[Argument],
        argument_types: &[CoreType],
        callback_expression: &Expression,
        arena: &HirArena,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<bool, InferenceError> {
        let CoreType::Function(expected_callback) = self.resolve(expected_parameter.clone())?
        else {
            return Ok(false);
        };
        let CoreType::Function(actual_callback) = self.resolve(actual_argument.clone())? else {
            return Ok(false);
        };

        let snapshot = self.snapshot();
        let result = self.forwarding_callback_probe_inner(
            &expected_callback,
            &actual_callback,
            forwarded_argument_indexes,
            arguments,
            argument_types,
            callback_expression,
            arena,
            type_definitions,
        );
        match result {
            Ok(true) => {
                self.commit(snapshot);
                Ok(true)
            }
            Ok(false) | Err(_) => {
                self.rollback_to(snapshot);
                Ok(false)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn forwarding_callback_probe_inner(
        &mut self,
        expected_callback: &FunctionType<CoreType>,
        actual_callback: &FunctionType<CoreType>,
        forwarded_argument_indexes: &[usize],
        arguments: &[Argument],
        argument_types: &[CoreType],
        callback_expression: &Expression,
        arena: &HirArena,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<bool, InferenceError> {
        let mut open_positionals = actual_callback.parameters.clone();
        let mut open_named = actual_callback.named_parameters.clone();
        let mut pre_rest_open = match &actual_callback.variadic {
            Some(variadic) => variadic.preceding_named,
            None => open_named.len(),
        };
        let actual_rest_element = actual_callback
            .variadic
            .as_ref()
            .map(|variadic| (*variadic.element).clone());

        // Forwarded named arguments consume the callback's same-named formals first, as R matches
        // names before positions.
        let mut forwarded_positionals = Vec::new();
        for &index in forwarded_argument_indexes {
            let argument = &arguments[index];
            let argument_type = argument_types[index].clone();
            let argument_expression = arena.get(argument.expression);
            match argument.name {
                Some(name) => {
                    match open_named
                        .iter()
                        .position(|parameter| parameter.name == name)
                    {
                        Some(position) => {
                            let parameter = open_named.remove(position);
                            if position < pre_rest_open {
                                pre_rest_open -= 1;
                            }
                            self.check_argument(
                                parameter.value,
                                argument_type,
                                argument_expression,
                                type_definitions,
                            )?;
                        }
                        None => match &actual_rest_element {
                            Some(element) => self.check_argument(
                                element.clone(),
                                argument_type,
                                argument_expression,
                                type_definitions,
                            )?,
                            None => return Ok(false),
                        },
                    }
                }
                None => forwarded_positionals.push(index),
            }
        }

        // The interface's parameter types are the elements the callee will pass; they fill the
        // callback's remaining formals in order, before the forwarded positionals.
        let mut element_types = expected_callback.parameters.clone();
        element_types.extend(
            expected_callback
                .named_parameters
                .iter()
                .map(|parameter| parameter.value.clone()),
        );

        enum Filled {
            Element(CoreType),
            Forwarded(usize),
        }
        let sequence = element_types
            .into_iter()
            .map(Filled::Element)
            .chain(forwarded_positionals.into_iter().map(Filled::Forwarded));
        for filled in sequence {
            let (argument_type, blame_expression) = match filled {
                Filled::Element(element) => (element, callback_expression),
                Filled::Forwarded(index) => (
                    argument_types[index].clone(),
                    arena.get(arguments[index].expression),
                ),
            };
            let formal = if !open_positionals.is_empty() {
                Some(open_positionals.remove(0))
            } else if pre_rest_open > 0 {
                pre_rest_open -= 1;
                Some(open_named.remove(0).value)
            } else {
                None
            };
            match formal {
                Some(formal) => {
                    self.check_argument(formal, argument_type, blame_expression, type_definitions)?
                }
                None => match &actual_rest_element {
                    Some(element) => self.check_argument(
                        element.clone(),
                        argument_type,
                        blame_expression,
                        type_definitions,
                    )?,
                    None => return Ok(false),
                },
            }
        }

        // Every formal the invocation leaves unfilled must have a default.
        if !open_positionals.is_empty() || open_named.iter().any(|parameter| !parameter.optional) {
            return Ok(false);
        }

        // The callback's result flows out through the interface's return type (covariant).
        self.check_compatibility(
            (*actual_callback.return_type).clone(),
            (*expected_callback.return_type).clone(),
            type_definitions,
            Some(callback_expression),
        )
    }

    // Arguments are checked with compatibility, not unification, so coercions like
    // scalar-to-vector and `T` into `T | NULL` work at parameter positions. An `Unknown`
    // argument is accepted to avoid cascading a second error after the cause was already
    // diagnosed where the value became `Unknown`.
    pub(super) fn check_argument(
        &mut self,
        parameter_type: CoreType,
        argument_type: CoreType,
        argument_expression: &Expression,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<(), InferenceError> {
        let resolved_argument = self.resolve(argument_type)?;
        if resolved_argument == CoreType::Unknown {
            return Ok(());
        }

        if self.check_compatibility(
            resolved_argument.clone(),
            parameter_type.clone(),
            type_definitions,
            Some(argument_expression),
        )? {
            return Ok(());
        }

        // R programmers write `seq_len(10)`, not `seq_len(10L)`: a whole-number double literal
        // counts as an integer at a parameter position, the same rule `:` applies to its
        // endpoints. The retry goes through full compatibility, so integer-expecting unions and
        // vector parameters admit the literal too. Off during a strict overload probe — the
        // courtesy must not decide which candidate wins (see `infer_overloaded_call`).
        if self.overload_probe_depth == 0
            && resolved_argument == CoreType::Scalar(Atomic::Double)
            && is_whole_number_double_literal(argument_expression)
            && self.check_compatibility(
                CoreType::Scalar(Atomic::Integer),
                parameter_type.clone(),
                type_definitions,
                Some(argument_expression),
            )?
        {
            return Ok(());
        }

        // A numeric-constrained parameter rejected the argument because it is not numeric; report
        // that directly rather than rendering the bare inference variable as the expected type.
        let resolved_parameter = self.resolve(parameter_type)?;
        if let CoreType::Variable(variable) = resolved_parameter
            && matches!(
                self.entries.get(&variable),
                Some(InferenceEntry::Unbound {
                    constraint: Constraint::Numeric,
                    ..
                })
            )
        {
            return Err(InferenceError::ConstraintViolation {
                constraint: Constraint::Numeric,
                actual: Box::new(resolved_argument),
                range: Some(argument_expression.range),
                expression_id: Some(argument_expression.id),
            });
        }

        Err(InferenceError::TypeMismatch {
            expected: Box::new(resolved_parameter),
            actual: Box::new(resolved_argument),
            range: Some(argument_expression.range),
            expression_id: Some(argument_expression.id),
        })
    }
}
