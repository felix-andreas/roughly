use {
    crate::{
        hir::{Argument, Expression, ExpressionId, ExpressionKind, HirArena, Module},
        interner::Symbol,
        types::{
            AnnotationKind, Atomic, AttachedAnnotation, CoreType, FunctionType,
            InferenceVariableId, RecordField, SurfaceType, TypeScheme,
        },
    },
    std::collections::{BTreeMap, BTreeSet},
    tree_sitter::Range,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferenceEntry {
    Unbound,
    Redirect(InferenceVariableId),
    Bound(CoreType),
}

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct InferenceState {
    next_variable_id: u32,
    entries: BTreeMap<InferenceVariableId, InferenceEntry>,
    environment: BTreeMap<Symbol, Binding>,
    builtins: BTreeMap<Symbol, BuiltinKind>,
}

impl InferenceState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fresh_variable(&mut self) -> InferenceVariableId {
        let variable = InferenceVariableId(self.next_variable_id);
        self.next_variable_id += 1;
        self.entries.insert(variable, InferenceEntry::Unbound);
        variable
    }

    pub fn entry(&self, variable: InferenceVariableId) -> Option<&InferenceEntry> {
        self.entries.get(&variable)
    }

    pub fn bind_name(&mut self, symbol: Symbol, core_type: CoreType, range: Range) {
        self.bind_scheme(symbol, TypeScheme::monomorphic(core_type), range);
    }

    pub fn bind_scheme(&mut self, symbol: Symbol, type_scheme: TypeScheme, range: Range) {
        self.environment
            .insert(symbol, Binding { type_scheme, range });
    }

    pub fn bind_builtin(&mut self, symbol: Symbol, builtin_kind: BuiltinKind) {
        self.builtins.insert(symbol, builtin_kind);
    }

    pub fn lookup_name(&self, symbol: Symbol) -> Option<&Binding> {
        self.environment.get(&symbol)
    }

    pub fn infer_module(&mut self, module: &Module) -> Result<Vec<CoreType>, InferenceError> {
        let mut inferred_types = Vec::with_capacity(module.root_expressions.len());

        for expression_id in &module.root_expressions {
            let expression = module.arena.get(*expression_id);
            inferred_types.push(self.infer_expression(expression, &module.arena)?);
        }

        Ok(inferred_types)
    }

    pub fn infer_expression(
        &mut self,
        expression: &Expression,
        arena: &HirArena,
    ) -> Result<CoreType, InferenceError> {
        match &expression.kind {
            ExpressionKind::Null => Ok(CoreType::Null),
            ExpressionKind::Logical(_) => Ok(CoreType::Scalar(Atomic::Logical)),
            ExpressionKind::Integer(_) => Ok(CoreType::Scalar(Atomic::Integer)),
            ExpressionKind::Double(_) => Ok(CoreType::Scalar(Atomic::Double)),
            ExpressionKind::Character(_) => Ok(CoreType::Scalar(Atomic::Character)),
            ExpressionKind::StringLiteralName(_) => Ok(CoreType::Scalar(Atomic::Character)),
            ExpressionKind::Symbol(symbol) => self
                .lookup_name(*symbol)
                .cloned()
                .map(|binding| self.instantiate_type_scheme(&binding.type_scheme))
                .transpose()?
                .ok_or(InferenceError::UnknownName {
                    symbol: *symbol,
                    range: expression.range,
                    expression_id: expression.id,
                }),
            ExpressionKind::Block {
                expressions,
                has_trailing_semicolon,
            } => self.infer_block(expressions, *has_trailing_semicolon, arena),
            ExpressionKind::Assign {
                target,
                annotation,
                value,
            } => {
                let inferred_value = self.infer_expression(arena.get(*value), arena)?;
                let binding_type = if let Some(annotation) = annotation {
                    if annotation.applies_to_binding {
                        self.apply_annotation(annotation, inferred_value, expression)?
                    } else {
                        inferred_value
                    }
                } else {
                    inferred_value
                };
                let generalized_scheme = self.generalize(binding_type.clone())?;
                self.bind_scheme(*target, generalized_scheme, expression.range);
                Ok(binding_type)
            }
            ExpressionKind::Function { parameters, body } => {
                let parent_environment = self.environment.clone();

                let mut parameter_types = Vec::with_capacity(parameters.len());
                for parameter in parameters {
                    let variable = self.fresh_variable();
                    let parameter_type = CoreType::Variable(variable);
                    self.bind_name(parameter.symbol, parameter_type.clone(), parameter.range);
                    parameter_types.push(parameter_type);
                }

                let return_type = self.infer_expression(arena.get(*body), arena)?;
                let function_type =
                    CoreType::Function(FunctionType::new(parameter_types, Vec::new(), return_type));

                self.environment = parent_environment;
                Ok(function_type)
            }
            ExpressionKind::If {
                condition,
                consequence,
                alternative,
            } => self.infer_if_expression(
                arena.get(*condition),
                arena.get(*consequence),
                alternative.as_ref().map(|id| arena.get(*id)),
                expression,
                arena,
            ),
            ExpressionKind::For {
                variable,
                sequence,
                body,
            } => self.infer_for_expression(
                *variable,
                arena.get(*sequence),
                arena.get(*body),
                expression.range,
                arena,
            ),
            ExpressionKind::While { condition, body } => {
                self.infer_while_expression(arena.get(*condition), arena.get(*body), arena)
            }
            ExpressionKind::Repeat { body } => {
                self.infer_repeat_expression(arena.get(*body), arena)
            }
            ExpressionKind::UnaryMinus { value } => {
                self.infer_unary_minus(arena.get(*value), arena)
            }
            ExpressionKind::Call { callee, arguments } => {
                let callee_expr = arena.get(*callee);
                if let ExpressionKind::Symbol(symbol) = &callee_expr.kind
                    && let Some(inferred_type) =
                        self.infer_builtin_call(*symbol, arguments, expression, arena)?
                {
                    return Ok(inferred_type);
                }

                self.infer_function_call_expression(callee_expr, arguments, expression, arena)
            }
            ExpressionKind::Subset { value, arguments } => {
                self.infer_subset_expression(arena.get(*value), arguments, expression, arena)
            }
            ExpressionKind::Subset2 { value, arguments } => {
                self.infer_subset2_expression(arena.get(*value), arguments, expression, arena)
            }
            ExpressionKind::Dollar { value, name } => {
                self.infer_dollar_expression(arena.get(*value), *name, expression, arena)
            }
            ExpressionKind::TypeAlias { .. } => Ok(CoreType::Null),
            ExpressionKind::Unsupported => Ok(CoreType::Unknown),
        }
    }

    fn infer_block(
        &mut self,
        expressions: &[ExpressionId],
        has_trailing_semicolon: bool,
        arena: &HirArena,
    ) -> Result<CoreType, InferenceError> {
        if expressions.is_empty() || has_trailing_semicolon {
            for expression_id in expressions {
                self.infer_expression(arena.get(*expression_id), arena)?;
            }
            return Ok(CoreType::Null);
        }

        let mut last_type = CoreType::Null;
        for expression_id in expressions {
            last_type = self.infer_expression(arena.get(*expression_id), arena)?;
        }

        Ok(last_type)
    }

    fn infer_if_expression(
        &mut self,
        condition: &Expression,
        consequence: &Expression,
        alternative: Option<&Expression>,
        expression: &Expression,
        arena: &HirArena,
    ) -> Result<CoreType, InferenceError> {
        self.expect_scalar_logical(condition, arena)?;

        let inferred_consequence = self.infer_expression(consequence, arena)?;
        let consequence_type = self.resolve(inferred_consequence)?;
        let Some(alternative) = alternative else {
            return Ok(nullable_type(consequence_type));
        };

        let inferred_alternative = self.infer_expression(alternative, arena)?;
        let alternative_type = self.resolve(inferred_alternative)?;
        if consequence_type == alternative_type {
            return Ok(consequence_type);
        }

        match (consequence_type, alternative_type) {
            (CoreType::Null, other_type) | (other_type, CoreType::Null) => {
                Ok(nullable_type(other_type))
            }
            (expected, actual) => Err(InferenceError::TypeMismatch {
                expected: Box::new(expected),
                actual: Box::new(actual),
                range: Some(expression.range),
                expression_id: Some(expression.id),
            }),
        }
    }

    fn infer_for_expression(
        &mut self,
        variable: Symbol,
        sequence: &Expression,
        body: &Expression,
        range: Range,
        arena: &HirArena,
    ) -> Result<CoreType, InferenceError> {
        let inferred_sequence = self.infer_expression(sequence, arena)?;
        let sequence_type = self.resolve(inferred_sequence)?;
        let Some(item_type) = iterable_item_type(&sequence_type) else {
            return Err(InferenceError::TypeMismatch {
                expected: Box::new(CoreType::Vector(Atomic::Integer)),
                actual: Box::new(sequence_type),
                range: Some(range),
                expression_id: Some(sequence.id),
            });
        };

        let previous_binding = self.environment.get(&variable).cloned();
        self.bind_name(variable, item_type, range);
        self.infer_expression(body, arena)?;

        if let Some(previous_binding) = previous_binding {
            self.environment.insert(variable, previous_binding);
        } else {
            self.environment.remove(&variable);
        }

        Ok(CoreType::Null)
    }

    fn infer_while_expression(
        &mut self,
        condition: &Expression,
        body: &Expression,
        arena: &HirArena,
    ) -> Result<CoreType, InferenceError> {
        self.expect_scalar_logical(condition, arena)?;
        self.infer_expression(body, arena)?;
        Ok(CoreType::Null)
    }

    fn infer_repeat_expression(
        &mut self,
        body: &Expression,
        arena: &HirArena,
    ) -> Result<CoreType, InferenceError> {
        self.infer_expression(body, arena)?;
        Ok(CoreType::Null)
    }

    fn expect_scalar_logical(
        &mut self,
        expression: &Expression,
        arena: &HirArena,
    ) -> Result<(), InferenceError> {
        let inferred_type = self.infer_expression(expression, arena)?;
        let actual = self.resolve(inferred_type)?;
        if actual == CoreType::Scalar(Atomic::Logical) {
            Ok(())
        } else {
            Err(InferenceError::TypeMismatch {
                expected: Box::new(CoreType::Scalar(Atomic::Logical)),
                actual: Box::new(actual),
                range: Some(expression.range),
                expression_id: Some(expression.id),
            })
        }
    }

    fn check_compatibility(&mut self, actual_type: CoreType, expected_type: CoreType) -> bool {
        let actual_type = self.resolve(actual_type).unwrap_or(CoreType::Unknown);
        let expected_type = self.resolve(expected_type).unwrap_or(CoreType::Unknown);

        if expected_type == CoreType::Any || actual_type == CoreType::Any {
            return true;
        }

        if actual_type == expected_type {
            return true;
        }

        if let CoreType::Variable(actual_var) = actual_type {
            return self
                .unify_internal(CoreType::Variable(actual_var), expected_type, None)
                .is_ok();
        }

        match (actual_type, expected_type) {
            (CoreType::Unknown, CoreType::Any) => true,
            (CoreType::Null, CoreType::Nullable(_)) => true,
            (other_type, CoreType::Nullable(inner_type)) => {
                self.check_compatibility(other_type, *inner_type)
            }
            (CoreType::Scalar(actual_atomic), CoreType::Vector(expected_atomic)) => {
                actual_atomic == expected_atomic
            }
            (CoreType::NamedVector(actual_atomic), CoreType::Vector(expected_atomic)) => {
                actual_atomic == expected_atomic
            }
            (CoreType::Tuple(items), CoreType::List(item_type)) => items
                .into_iter()
                .all(|item| self.check_compatibility(item, *item_type.clone())),
            (CoreType::Record(fields), CoreType::List(item_type))
            | (CoreType::Record(fields), CoreType::NamedList(item_type)) => fields
                .into_iter()
                .all(|field| self.check_compatibility(field.value, *item_type.clone())),
            (CoreType::NamedList(actual_item_type), CoreType::List(expected_item_type))
            | (CoreType::NamedList(actual_item_type), CoreType::NamedList(expected_item_type))
            | (CoreType::List(actual_item_type), CoreType::List(expected_item_type)) => {
                self.check_compatibility(*actual_item_type, *expected_item_type)
            }
            (CoreType::Function(actual_function), CoreType::Function(expected_function)) => {
                let actual_positional_parameters = actual_function.parameters;
                let actual_named_parameters = actual_function.named_parameters;
                let expected_positional_parameters = expected_function.parameters;
                let expected_named_parameters = expected_function.named_parameters;

                if actual_named_parameters.is_empty() && expected_named_parameters.is_empty() {
                    if actual_positional_parameters.len() != expected_positional_parameters.len() {
                        return false;
                    }

                    for (actual_param, expected_param) in actual_positional_parameters
                        .into_iter()
                        .zip(expected_positional_parameters)
                    {
                        if !self.check_compatibility(actual_param, expected_param) {
                            return false;
                        }
                    }

                    return self.check_compatibility(
                        *actual_function.return_type,
                        *expected_function.return_type,
                    );
                }

                let actual_parameter_count =
                    actual_positional_parameters.len() + actual_named_parameters.len();
                let expected_parameter_count =
                    expected_positional_parameters.len() + expected_named_parameters.len();

                if actual_parameter_count != expected_parameter_count {
                    return false;
                }

                let mut actual_parameters = Vec::with_capacity(actual_parameter_count);
                for actual_parameter in actual_positional_parameters {
                    actual_parameters.push(actual_parameter);
                }
                for actual_parameter in actual_named_parameters {
                    actual_parameters.push(actual_parameter.value);
                }

                let mut expected_parameters = Vec::with_capacity(expected_parameter_count);
                for expected_parameter in expected_positional_parameters {
                    expected_parameters.push(expected_parameter);
                }
                for expected_parameter in expected_named_parameters {
                    expected_parameters.push(expected_parameter.value);
                }

                for (actual_param, expected_param) in
                    actual_parameters.into_iter().zip(expected_parameters)
                {
                    if !self.check_compatibility(actual_param, expected_param) {
                        return false;
                    }
                }

                self.check_compatibility(
                    *actual_function.return_type,
                    *expected_function.return_type,
                )
            }
            _ => false,
        }
    }

    fn apply_annotation(
        &mut self,
        annotation: &AttachedAnnotation,
        inferred_type: CoreType,
        expression: &Expression,
    ) -> Result<CoreType, InferenceError> {
        let expected_type = core_type_from_surface_type(&annotation.annotation.surface_type);
        let actual_type = self.resolve(inferred_type)?;

        match annotation.annotation.kind {
            AnnotationKind::Checked => {
                if self.check_compatibility(actual_type.clone(), expected_type.clone()) {
                    Ok(expected_type)
                } else {
                    Err(InferenceError::TypeMismatch {
                        expected: Box::new(expected_type),
                        actual: Box::new(actual_type),
                        range: Some(expression.range),
                        expression_id: Some(expression.id),
                    })
                }
            }
            AnnotationKind::UnknownOnly => {
                if actual_type == CoreType::Unknown {
                    Ok(expected_type)
                } else {
                    Err(InferenceError::TypeMismatch {
                        expected: Box::new(CoreType::Unknown),
                        actual: Box::new(actual_type),
                        range: Some(expression.range),
                        expression_id: Some(expression.id),
                    })
                }
            }
            AnnotationKind::Trusted => Ok(expected_type),
        }
    }

    pub fn resolve(&mut self, core_type: CoreType) -> Result<CoreType, InferenceError> {
        match core_type {
            CoreType::Variable(variable) => self.resolve_variable(variable),
            CoreType::Nullable(inner_type) => {
                let resolved_inner_type = self.resolve(*inner_type)?;
                Ok(nullable_type(resolved_inner_type))
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
                    resolved_fields.push(RecordField::new(field.name, self.resolve(field.value)?));
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

    fn unify_internal(
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
            (CoreType::Nullable(left_type), CoreType::Nullable(right_type)) => {
                let unified_type = self.unify_internal(*left_type, *right_type, expression)?;
                Ok(nullable_type(unified_type))
            }
            (CoreType::Nullable(inner_type), CoreType::Null)
            | (CoreType::Null, CoreType::Nullable(inner_type)) => {
                Ok(CoreType::Nullable(inner_type))
            }
            (CoreType::Scalar(left_atomic), CoreType::Scalar(right_atomic))
                if left_atomic == right_atomic =>
            {
                Ok(CoreType::Scalar(left_atomic))
            }
            (CoreType::Vector(left_atomic), CoreType::Vector(right_atomic))
                if left_atomic == right_atomic =>
            {
                Ok(CoreType::Vector(left_atomic))
            }
            (CoreType::NamedVector(left_atomic), CoreType::NamedVector(right_atomic))
                if left_atomic == right_atomic =>
            {
                Ok(CoreType::NamedVector(left_atomic))
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
            CoreType::Nullable(inner_type) => self.occurs_in(variable, &inner_type),
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

                self.occurs_in(variable, &function_type.return_type)
            }
            _ => Ok(false),
        }
    }

    fn infer_builtin_call(
        &mut self,
        symbol: Symbol,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
    ) -> Result<Option<CoreType>, InferenceError> {
        let Some(builtin_kind) = self.builtins.get(&symbol).copied() else {
            return Ok(None);
        };

        match builtin_kind {
            BuiltinKind::Plus => self
                .infer_builtin_plus(arguments, expression, arena)
                .map(Some),
            BuiltinKind::Minus => self
                .infer_builtin_minus(arguments, expression, arena)
                .map(Some),
            BuiltinKind::Multiply => self
                .infer_builtin_multiply(arguments, expression, arena)
                .map(Some),
            BuiltinKind::Divide => self
                .infer_builtin_divide(arguments, expression, arena)
                .map(Some),
            BuiltinKind::Power => self
                .infer_builtin_power(arguments, expression, arena)
                .map(Some),
            BuiltinKind::And => self
                .infer_builtin_boolean_binary(arguments, expression, arena)
                .map(Some),
            BuiltinKind::Or => self
                .infer_builtin_boolean_binary(arguments, expression, arena)
                .map(Some),
            BuiltinKind::Combine => self
                .infer_builtin_combine(arguments, expression, arena)
                .map(Some),
            BuiltinKind::List => self
                .infer_builtin_list(arguments, expression, arena)
                .map(Some),
        }
    }

    fn infer_builtin_plus(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
    ) -> Result<CoreType, InferenceError> {
        self.infer_binary_numeric(arguments, expression, NumericResultAtomic::Promote, arena)
    }

    fn infer_builtin_minus(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
    ) -> Result<CoreType, InferenceError> {
        self.infer_binary_numeric(arguments, expression, NumericResultAtomic::Promote, arena)
    }

    fn infer_builtin_multiply(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
    ) -> Result<CoreType, InferenceError> {
        self.infer_binary_numeric(arguments, expression, NumericResultAtomic::Promote, arena)
    }

    fn infer_builtin_divide(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
    ) -> Result<CoreType, InferenceError> {
        self.infer_binary_numeric(
            arguments,
            expression,
            NumericResultAtomic::AlwaysDouble,
            arena,
        )
    }

    fn infer_builtin_power(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
    ) -> Result<CoreType, InferenceError> {
        self.infer_binary_numeric(
            arguments,
            expression,
            NumericResultAtomic::AlwaysDouble,
            arena,
        )
    }

    fn infer_binary_numeric(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        numeric_result_atomic: NumericResultAtomic,
        arena: &HirArena,
    ) -> Result<CoreType, InferenceError> {
        if arguments.len() != 2 {
            return Err(InferenceError::FunctionArityMismatch {
                expected: 2,
                actual: arguments.len(),
                range: Some(expression.range),
                expression_id: Some(expression.id),
            });
        }

        let arg0 = arena.get(arguments[0].expression);
        let arg1 = arena.get(arguments[1].expression);

        let left_type = self.infer_expression(arg0, arena)?;
        let right_type = self.infer_expression(arg1, arena)?;

        let resolved_left = self.resolve(left_type)?;
        let resolved_right = self.resolve(right_type)?;

        let left_shape_atomic = numeric_operand_parts(&resolved_left);
        let right_shape_atomic = numeric_operand_parts(&resolved_right);

        let (left_shape, left_atomic) = match left_shape_atomic {
            Some(parts) => parts,
            None if matches!(resolved_left, CoreType::Any | CoreType::Unknown) => {
                return Ok(CoreType::Unknown);
            }
            None => {
                return Err(InferenceError::InvalidPlusOperand {
                    actual: resolved_left.clone(),
                    range: arg0.range,
                    expression_id: arg0.id,
                });
            }
        };

        let (right_shape, right_atomic) = match right_shape_atomic {
            Some(parts) => parts,
            None if matches!(resolved_right, CoreType::Any | CoreType::Unknown) => {
                return Ok(CoreType::Unknown);
            }
            None => {
                return Err(InferenceError::InvalidPlusOperand {
                    actual: resolved_right.clone(),
                    range: arg1.range,
                    expression_id: arg1.id,
                });
            }
        };

        let result_atomic = match numeric_result_atomic {
            NumericResultAtomic::Promote => promote_numeric_atomic(left_atomic, right_atomic),
            NumericResultAtomic::AlwaysDouble => Atomic::Double,
        };
        let result_shape = if matches!(left_shape, OperandShape::Vector)
            || matches!(right_shape, OperandShape::Vector)
        {
            OperandShape::Vector
        } else {
            OperandShape::Scalar
        };

        Ok(core_type_for_shape(result_shape, result_atomic))
    }

    fn infer_unary_minus(
        &mut self,
        value: &Expression,
        arena: &HirArena,
    ) -> Result<CoreType, InferenceError> {
        let inferred_type = self.infer_expression(value, arena)?;
        let resolved_type = self.resolve(inferred_type)?;
        let (shape, atomic) = numeric_operand_parts(&resolved_type).ok_or_else(|| {
            InferenceError::InvalidPlusOperand {
                actual: resolved_type.clone(),
                range: value.range,
                expression_id: value.id,
            }
        })?;

        Ok(core_type_for_shape(shape, atomic))
    }

    fn infer_function_call_expression(
        &mut self,
        callee: &Expression,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
    ) -> Result<CoreType, InferenceError> {
        let inferred_callee = self.infer_expression(callee, arena)?;
        let resolved_callee = self.resolve(inferred_callee)?;

        match resolved_callee {
            CoreType::Unknown => Ok(CoreType::Unknown),
            CoreType::Any => Ok(CoreType::Any),
            CoreType::Variable(variable) => {
                let mut positional_arguments = Vec::new();
                let mut named_arguments = Vec::new();

                for argument in arguments {
                    let inferred_argument =
                        self.infer_expression(arena.get(argument.expression), arena)?;
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
            CoreType::Function(function_type) => {
                self.infer_function_call(function_type, arguments, callee, expression, arena)
            }
            other_type => Err(InferenceError::ExpectedFunction {
                actual_type: other_type,
                range: callee.range,
                expression_id: callee.id,
            }),
        }
    }

    fn infer_function_call(
        &mut self,
        function_type: FunctionType<CoreType>,
        arguments: &[Argument],
        callee: &Expression,
        expression: &Expression,
        arena: &HirArena,
    ) -> Result<CoreType, InferenceError> {
        let total_parameters =
            function_type.parameters.len() + function_type.named_parameters.len();
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
        let return_type = *function_type.return_type;
        let mut next_positional_index = 0;
        let mut remaining_named_parameters = function_type.named_parameters;

        for argument in arguments {
            let arg_expr = arena.get(argument.expression);
            let inferred_argument = self.infer_expression(arg_expr, arena)?;
            if let Some(name) = argument.name {
                let Some(parameter_index) = remaining_named_parameters
                    .iter()
                    .position(|parameter| parameter.name == name)
                else {
                    return Err(InferenceError::NamedParameterMismatch {
                        expected_parameters: expected_named_parameters,
                        actual_parameters: actual_named_arguments,
                        range: Some(expression.range),
                        expression_id: Some(expression.id),
                    });
                };

                let parameter = remaining_named_parameters.remove(parameter_index);
                self.unify_with_context(parameter.value, inferred_argument, arg_expr)?;
                continue;
            }

            if let Some(parameter) = positional_parameters.get(next_positional_index) {
                next_positional_index += 1;
                self.unify_with_context(parameter.clone(), inferred_argument, arg_expr)?;
                continue;
            }

            if !remaining_named_parameters.is_empty() {
                let parameter = remaining_named_parameters.remove(0);
                self.unify_with_context(parameter.value, inferred_argument, arg_expr)?;
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
            || !remaining_named_parameters.is_empty()
        {
            return Err(InferenceError::FunctionArityMismatch {
                expected: total_parameters,
                actual: arguments.len(),
                range: Some(callee.range),
                expression_id: Some(callee.id),
            });
        }

        self.resolve(return_type)
    }

    fn infer_subset_expression(
        &mut self,
        value: &Expression,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
    ) -> Result<CoreType, InferenceError> {
        if arguments.len() != 1 || arguments[0].name.is_some() {
            return Err(InferenceError::FunctionArityMismatch {
                expected: 1,
                actual: arguments.len(),
                range: Some(expression.range),
                expression_id: Some(expression.id),
            });
        }

        let inferred_value = self.infer_expression(value, arena)?;
        let value_type = self.resolve(inferred_value)?;
        let arg0_expr = arena.get(arguments[0].expression);
        let inferred_index = self.infer_expression(arg0_expr, arena)?;
        let index_type = self.resolve(inferred_index)?;

        match value_type {
            CoreType::Unknown => Ok(CoreType::Unknown),
            CoreType::Any => Ok(CoreType::Any),
            CoreType::List(item_type) => Ok(CoreType::List(item_type)),
            CoreType::NamedList(item_type) => Ok(CoreType::NamedList(item_type)),
            CoreType::Tuple(items) => {
                let Some(item_type) = homogeneous_structural_item_type(&items) else {
                    return Err(index_type_mismatch(
                        CoreType::Tuple(items),
                        index_type,
                        arg0_expr,
                    ));
                };
                Ok(CoreType::List(Box::new(item_type)))
            }
            CoreType::Record(fields) => {
                let field_types = fields
                    .iter()
                    .map(|field| field.value.clone())
                    .collect::<Vec<_>>();
                let Some(item_type) = homogeneous_structural_item_type(&field_types) else {
                    return Err(index_type_mismatch(
                        CoreType::Record(fields),
                        index_type,
                        arg0_expr,
                    ));
                };
                Ok(CoreType::NamedList(Box::new(item_type)))
            }
            other_type => Err(index_type_mismatch(other_type, index_type, arg0_expr)),
        }
    }

    fn infer_subset2_expression(
        &mut self,
        value: &Expression,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
    ) -> Result<CoreType, InferenceError> {
        if arguments.len() != 1 || arguments[0].name.is_some() {
            return Err(InferenceError::FunctionArityMismatch {
                expected: 1,
                actual: arguments.len(),
                range: Some(expression.range),
                expression_id: Some(expression.id),
            });
        }

        let inferred_value = self.infer_expression(value, arena)?;
        let value_type = self.resolve(inferred_value)?;
        let index_expression = arena.get(arguments[0].expression);
        let inferred_index = self.infer_expression(index_expression, arena)?;
        let index_type = self.resolve(inferred_index)?;

        match value_type {
            CoreType::Unknown => Ok(CoreType::Unknown),
            CoreType::Any => Ok(CoreType::Any),
            CoreType::Scalar(atomic) | CoreType::Vector(atomic) => Ok(CoreType::Scalar(atomic)),
            CoreType::NamedVector(atomic) => {
                if literal_name_symbol(index_expression).is_some() {
                    Ok(nullable_type(CoreType::Scalar(atomic)))
                } else {
                    Ok(CoreType::Scalar(atomic))
                }
            }
            CoreType::List(item_type) => Ok(*item_type),
            CoreType::NamedList(item_type) => {
                if literal_name_symbol(index_expression).is_some() {
                    Ok(nullable_type(*item_type))
                } else {
                    Err(index_type_mismatch(
                        CoreType::NamedList(item_type),
                        index_type,
                        index_expression,
                    ))
                }
            }
            CoreType::Tuple(items) => {
                let Some(index) = integer_literal_position(index_expression) else {
                    return Err(index_type_mismatch(
                        CoreType::Tuple(items),
                        index_type,
                        index_expression,
                    ));
                };
                let Some(item_type) = items.get(index).cloned() else {
                    return Err(index_type_mismatch(
                        CoreType::Tuple(items),
                        index_type,
                        index_expression,
                    ));
                };
                Ok(item_type)
            }
            CoreType::Record(fields) => {
                let Some(name) = literal_name_symbol(index_expression) else {
                    return Err(index_type_mismatch(
                        CoreType::Record(fields),
                        index_type,
                        index_expression,
                    ));
                };
                let Some(field) = fields.into_iter().find(|field| field.name == name) else {
                    return Err(index_type_mismatch(
                        CoreType::Record(Vec::new()),
                        index_type,
                        index_expression,
                    ));
                };
                Ok(field.value)
            }
            other_type => Err(index_type_mismatch(
                other_type,
                index_type,
                index_expression,
            )),
        }
    }

    fn infer_dollar_expression(
        &mut self,
        value: &Expression,
        name: Symbol,
        expression: &Expression,
        arena: &HirArena,
    ) -> Result<CoreType, InferenceError> {
        let inferred_value = self.infer_expression(value, arena)?;
        let value_type = self.resolve(inferred_value)?;

        match value_type {
            CoreType::Unknown => Ok(CoreType::Unknown),
            CoreType::Any => Ok(CoreType::Any),
            CoreType::NamedVector(atomic) => Ok(nullable_type(CoreType::Scalar(atomic))),
            CoreType::NamedList(item_type) => Ok(nullable_type(*item_type)),
            CoreType::Record(fields) => {
                let Some(field) = fields.into_iter().find(|field| field.name == name) else {
                    return Err(index_type_mismatch(
                        CoreType::Record(Vec::new()),
                        CoreType::Unknown,
                        expression,
                    ));
                };
                Ok(field.value)
            }
            other_type => Err(index_type_mismatch(
                other_type,
                CoreType::Unknown,
                expression,
            )),
        }
    }

    fn infer_builtin_boolean_binary(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
    ) -> Result<CoreType, InferenceError> {
        if arguments.len() != 2 {
            return Err(InferenceError::FunctionArityMismatch {
                expected: 2,
                actual: arguments.len(),
                range: Some(expression.range),
                expression_id: Some(expression.id),
            });
        }

        self.expect_scalar_logical(arena.get(arguments[0].expression), arena)?;
        self.expect_scalar_logical(arena.get(arguments[1].expression), arena)?;
        Ok(CoreType::Scalar(Atomic::Logical))
    }

    fn infer_builtin_combine(
        &mut self,
        arguments: &[Argument],
        _expression: &Expression,
        arena: &HirArena,
    ) -> Result<CoreType, InferenceError> {
        if arguments.is_empty() {
            return Ok(CoreType::Unknown);
        }

        let mut item_atomic = None;
        let mut all_arguments_are_named = true;

        for argument in arguments {
            all_arguments_are_named &= argument.name.is_some();

            let arg_expr = arena.get(argument.expression);
            let inferred_argument = self.infer_expression(arg_expr, arena)?;
            let resolved_argument = self.resolve(inferred_argument)?;

            let Some(current_atomic) = combine_operand_atomic(&resolved_argument) else {
                return Err(InferenceError::TypeMismatch {
                    expected: Box::new(CoreType::Scalar(Atomic::Integer)),
                    actual: Box::new(resolved_argument.clone()),
                    range: Some(arg_expr.range),
                    expression_id: Some(arg_expr.id),
                });
            };

            item_atomic = Some(match item_atomic {
                Some(previous_atomic) => promote_combine_atomic(previous_atomic, current_atomic)
                    .ok_or_else(|| InferenceError::TypeMismatch {
                        expected: Box::new(CoreType::Scalar(previous_atomic)),
                        actual: Box::new(resolved_argument.clone()),
                        range: Some(arg_expr.range),
                        expression_id: Some(arg_expr.id),
                    })?,
                None => current_atomic,
            });
        }

        let combined_atomic = item_atomic.unwrap_or(Atomic::Integer);
        if all_arguments_are_named {
            Ok(CoreType::NamedVector(combined_atomic))
        } else {
            Ok(CoreType::Vector(combined_atomic))
        }
    }

    fn infer_builtin_list(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
    ) -> Result<CoreType, InferenceError> {
        if arguments.is_empty() {
            return Ok(CoreType::Tuple(Vec::new()));
        }

        let all_arguments_are_named = arguments.iter().all(|argument| argument.name.is_some());
        let all_arguments_are_unnamed = arguments.iter().all(|argument| argument.name.is_none());

        if !(all_arguments_are_named || all_arguments_are_unnamed) {
            return Err(InferenceError::MixedListElements {
                range: Some(expression.range),
                expression_id: Some(expression.id),
            });
        }

        if all_arguments_are_named {
            let mut fields = Vec::with_capacity(arguments.len());
            for argument in arguments {
                let inferred_type = self.infer_expression(arena.get(argument.expression), arena)?;
                let inferred_type = self.resolve(inferred_type)?;
                fields.push(RecordField::new(
                    argument
                        .name
                        .expect("named list arguments should have names"),
                    inferred_type,
                ));
            }
            Ok(CoreType::Record(fields))
        } else {
            let mut items = Vec::with_capacity(arguments.len());
            for argument in arguments {
                let inferred_type = self.infer_expression(arena.get(argument.expression), arena)?;
                items.push(self.resolve(inferred_type)?);
            }
            Ok(CoreType::Tuple(items))
        }
    }

    fn resolve_variable(
        &mut self,
        variable: InferenceVariableId,
    ) -> Result<CoreType, InferenceError> {
        let Some(entry) = self.entries.get(&variable).cloned() else {
            return Err(InferenceError::UnknownInferenceVariable(variable));
        };

        match entry {
            InferenceEntry::Unbound => Ok(CoreType::Variable(variable)),
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

    fn compress_variable(
        &mut self,
        variable: InferenceVariableId,
        resolved_type: &CoreType,
    ) -> Result<(), InferenceError> {
        let Some(entry) = self.entries.get_mut(&variable) else {
            return Err(InferenceError::UnknownInferenceVariable(variable));
        };

        *entry = match resolved_type {
            CoreType::Variable(other_variable) if *other_variable != variable => {
                InferenceEntry::Redirect(*other_variable)
            }
            other_type => InferenceEntry::Bound(other_type.clone()),
        };

        Ok(())
    }

    fn bind_variable(
        &mut self,
        variable: InferenceVariableId,
        core_type: CoreType,
        expression: Option<&Expression>,
    ) -> Result<(), InferenceError> {
        if self.occurs_in(variable, &core_type)? {
            return Err(InferenceError::OccursCheckFailed {
                variable,
                in_type: core_type,
                range: expression.map(|current_expression| current_expression.range),
                expression_id: expression.map(|current_expression| current_expression.id),
            });
        }

        let Some(entry) = self.entries.get_mut(&variable) else {
            return Err(InferenceError::UnknownInferenceVariable(variable));
        };

        *entry = InferenceEntry::Bound(core_type);
        Ok(())
    }

    fn unify_variables(
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
        if !matches!(left_entry, InferenceEntry::Unbound) {
            let resolved_left = self.resolve_variable(left)?;
            let resolved_right = self.resolve_variable(right)?;
            return self.unify(resolved_left, resolved_right);
        }

        let Some(right_entry) = self.entries.get(&right) else {
            return Err(InferenceError::UnknownInferenceVariable(right));
        };
        if !matches!(right_entry, InferenceEntry::Unbound) {
            let resolved_left = self.resolve_variable(left)?;
            let resolved_right = self.resolve_variable(right)?;
            return self.unify(resolved_left, resolved_right);
        }

        let Some(entry) = self.entries.get_mut(&left) else {
            return Err(InferenceError::UnknownInferenceVariable(left));
        };
        *entry = InferenceEntry::Redirect(right);

        Ok(CoreType::Variable(right))
    }

    fn instantiate_type_scheme(
        &mut self,
        type_scheme: &TypeScheme,
    ) -> Result<CoreType, InferenceError> {
        let mut substitutions = BTreeMap::new();

        for variable in &type_scheme.quantified_variables {
            substitutions.insert(*variable, self.fresh_variable());
        }

        self.instantiate_core_type(&type_scheme.body, &substitutions)
    }

    fn instantiate_core_type(
        &mut self,
        core_type: &CoreType,
        substitutions: &BTreeMap<InferenceVariableId, InferenceVariableId>,
    ) -> Result<CoreType, InferenceError> {
        match core_type {
            CoreType::Any => Ok(CoreType::Any),
            CoreType::Unknown => Ok(CoreType::Unknown),
            CoreType::Null => Ok(CoreType::Null),
            CoreType::Nullable(inner_type) => Ok(nullable_type(
                self.instantiate_core_type(inner_type, substitutions)?,
            )),
            CoreType::Scalar(atomic) => Ok(CoreType::Scalar(*atomic)),
            CoreType::Vector(atomic) => Ok(CoreType::Vector(*atomic)),
            CoreType::NamedVector(atomic) => Ok(CoreType::NamedVector(*atomic)),
            CoreType::List(item_type) => Ok(CoreType::List(Box::new(
                self.instantiate_core_type(item_type, substitutions)?,
            ))),
            CoreType::NamedList(item_type) => Ok(CoreType::NamedList(Box::new(
                self.instantiate_core_type(item_type, substitutions)?,
            ))),
            CoreType::Record(fields) => {
                let mut instantiated_fields = Vec::with_capacity(fields.len());
                for field in fields {
                    instantiated_fields.push(RecordField::new(
                        field.name,
                        self.instantiate_core_type(&field.value, substitutions)?,
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
                    instantiated_named_parameters.push(RecordField::new(
                        named_parameter.name,
                        self.instantiate_core_type(&named_parameter.value, substitutions)?,
                    ));
                }

                let instantiated_return_type =
                    self.instantiate_core_type(&function_type.return_type, substitutions)?;

                Ok(CoreType::Function(FunctionType::new(
                    instantiated_parameters,
                    instantiated_named_parameters,
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

    fn generalize(&mut self, core_type: CoreType) -> Result<TypeScheme, InferenceError> {
        let resolved_type = self.resolve(core_type)?;
        let type_variables = self.free_type_variables_in_core_type(&resolved_type)?;
        let environment_variables = self.free_type_variables_in_environment()?;

        let quantified_variables = type_variables
            .difference(&environment_variables)
            .copied()
            .collect();

        Ok(TypeScheme {
            quantified_variables,
            body: resolved_type,
        })
    }

    fn free_type_variables_in_environment(
        &mut self,
    ) -> Result<BTreeSet<InferenceVariableId>, InferenceError> {
        let type_schemes = self
            .environment
            .values()
            .map(|binding| binding.type_scheme.clone())
            .collect::<Vec<_>>();
        let mut free_variables = BTreeSet::new();

        for type_scheme in type_schemes {
            let scheme_variables = self.free_type_variables_in_type_scheme(&type_scheme)?;
            free_variables.extend(scheme_variables);
        }

        Ok(free_variables)
    }

    fn free_type_variables_in_type_scheme(
        &mut self,
        type_scheme: &TypeScheme,
    ) -> Result<BTreeSet<InferenceVariableId>, InferenceError> {
        let mut free_variables = self.free_type_variables_in_core_type(&type_scheme.body)?;

        for quantified_variable in &type_scheme.quantified_variables {
            free_variables.remove(quantified_variable);
        }

        Ok(free_variables)
    }

    fn free_type_variables_in_core_type(
        &mut self,
        core_type: &CoreType,
    ) -> Result<BTreeSet<InferenceVariableId>, InferenceError> {
        match self.resolve(core_type.clone())? {
            CoreType::Any
            | CoreType::Unknown
            | CoreType::Null
            | CoreType::Scalar(_)
            | CoreType::Vector(_)
            | CoreType::NamedVector(_) => Ok(BTreeSet::new()),
            CoreType::Nullable(inner_type) => self.free_type_variables_in_core_type(&inner_type),
            CoreType::Variable(variable) => Ok(BTreeSet::from([variable])),
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

                free_variables
                    .extend(self.free_type_variables_in_core_type(&function_type.return_type)?);

                Ok(free_variables)
            }
        }
    }

    fn resolve_function_type(
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
            resolved_named_parameters.push(RecordField::new(
                named_parameter.name,
                self.resolve(named_parameter.value)?,
            ));
        }

        let resolved_return_type = self.resolve(*function_type.return_type)?;

        Ok(FunctionType::new(
            resolved_parameters,
            resolved_named_parameters,
            resolved_return_type,
        ))
    }

    fn unify_tuples(
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

    fn unify_records(
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
            unified_fields.push(RecordField::new(left_field.name, unified_value));
        }

        Ok(CoreType::Record(unified_fields))
    }

    fn unify_functions(
        &mut self,
        left_function: FunctionType<CoreType>,
        right_function: FunctionType<CoreType>,
        expression: Option<&Expression>,
    ) -> Result<FunctionType<CoreType>, InferenceError> {
        if left_function.parameters.len() != right_function.parameters.len() {
            return Err(InferenceError::FunctionArityMismatch {
                expected: left_function.parameters.len(),
                actual: right_function.parameters.len(),
                range: expression.map(|current_expression| current_expression.range),
                expression_id: expression.map(|current_expression| current_expression.id),
            });
        }

        let left_named_names: BTreeSet<_> = left_function
            .named_parameters
            .iter()
            .map(|parameter| parameter.name)
            .collect();
        let right_named_names: BTreeSet<_> = right_function
            .named_parameters
            .iter()
            .map(|parameter| parameter.name)
            .collect();

        if left_named_names != right_named_names {
            return Err(InferenceError::NamedParameterMismatch {
                expected_parameters: left_named_names.into_iter().collect(),
                actual_parameters: right_named_names.into_iter().collect(),
                range: expression.map(|current_expression| current_expression.range),
                expression_id: expression.map(|current_expression| current_expression.id),
            });
        }

        let mut unified_parameters = Vec::with_capacity(left_function.parameters.len());
        for (left_parameter, right_parameter) in left_function
            .parameters
            .into_iter()
            .zip(right_function.parameters)
        {
            unified_parameters.push(self.unify_internal(
                left_parameter,
                right_parameter,
                expression,
            )?);
        }

        let right_named_by_name: BTreeMap<_, _> = right_function
            .named_parameters
            .into_iter()
            .map(|parameter| (parameter.name, parameter.value))
            .collect();

        let mut unified_named_parameters = Vec::with_capacity(left_function.named_parameters.len());
        for left_named_parameter in left_function.named_parameters {
            let Some(right_value) = right_named_by_name.get(&left_named_parameter.name).cloned()
            else {
                return Err(InferenceError::NamedParameterMismatch {
                    expected_parameters: vec![left_named_parameter.name],
                    actual_parameters: Vec::new(),
                    range: expression.map(|current_expression| current_expression.range),
                    expression_id: expression.map(|current_expression| current_expression.id),
                });
            };

            let unified_value =
                self.unify_internal(left_named_parameter.value, right_value, expression)?;
            unified_named_parameters
                .push(RecordField::new(left_named_parameter.name, unified_value));
        }

        let unified_return_type = self.unify_internal(
            *left_function.return_type,
            *right_function.return_type,
            expression,
        )?;

        Ok(FunctionType::new(
            unified_parameters,
            unified_named_parameters,
            unified_return_type,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperandShape {
    Scalar,
    Vector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NumericResultAtomic {
    Promote,
    AlwaysDouble,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinKind {
    Plus,
    Minus,
    Multiply,
    Divide,
    Power,
    And,
    Or,
    Combine,
    List,
}

fn numeric_operand_parts(core_type: &CoreType) -> Option<(OperandShape, Atomic)> {
    match core_type {
        CoreType::Scalar(Atomic::Integer) => Some((OperandShape::Scalar, Atomic::Integer)),
        CoreType::Scalar(Atomic::Double) => Some((OperandShape::Scalar, Atomic::Double)),
        CoreType::Vector(Atomic::Integer) => Some((OperandShape::Vector, Atomic::Integer)),
        CoreType::Vector(Atomic::Double) => Some((OperandShape::Vector, Atomic::Double)),
        CoreType::NamedVector(Atomic::Integer) => Some((OperandShape::Vector, Atomic::Integer)),
        CoreType::NamedVector(Atomic::Double) => Some((OperandShape::Vector, Atomic::Double)),
        _ => None,
    }
}

fn combine_operand_atomic(core_type: &CoreType) -> Option<Atomic> {
    match core_type {
        CoreType::Scalar(atomic) | CoreType::Vector(atomic) | CoreType::NamedVector(atomic) => {
            Some(*atomic)
        }
        _ => None,
    }
}

fn promote_combine_atomic(left: Atomic, right: Atomic) -> Option<Atomic> {
    if left == right {
        return Some(left);
    }

    match (left, right) {
        (Atomic::Integer, Atomic::Double) | (Atomic::Double, Atomic::Integer) => {
            Some(Atomic::Double)
        }
        _ => None,
    }
}

fn promote_numeric_atomic(left: Atomic, right: Atomic) -> Atomic {
    if matches!(left, Atomic::Double) || matches!(right, Atomic::Double) {
        Atomic::Double
    } else {
        Atomic::Integer
    }
}

fn core_type_for_shape(shape: OperandShape, atomic: Atomic) -> CoreType {
    match shape {
        OperandShape::Scalar => CoreType::Scalar(atomic),
        OperandShape::Vector => CoreType::Vector(atomic),
    }
}

fn nullable_type(core_type: CoreType) -> CoreType {
    match core_type {
        CoreType::Null => CoreType::Null,
        CoreType::Nullable(inner_type) => CoreType::Nullable(inner_type),
        other_type => CoreType::Nullable(Box::new(other_type)),
    }
}

fn iterable_item_type(core_type: &CoreType) -> Option<CoreType> {
    match core_type {
        CoreType::Scalar(atomic) | CoreType::Vector(atomic) | CoreType::NamedVector(atomic) => {
            Some(CoreType::Scalar(*atomic))
        }
        CoreType::List(item_type) | CoreType::NamedList(item_type) => Some((**item_type).clone()),
        CoreType::Tuple(items) => homogeneous_structural_item_type(items),
        CoreType::Record(fields) => homogeneous_structural_item_type(
            &fields
                .iter()
                .map(|field| field.value.clone())
                .collect::<Vec<_>>(),
        ),
        _ => None,
    }
}

fn homogeneous_structural_item_type(items: &[CoreType]) -> Option<CoreType> {
    let first_item = items.first()?.clone();
    if items.iter().skip(1).all(|item| *item == first_item) {
        Some(first_item)
    } else {
        None
    }
}

fn integer_literal_position(expression: &Expression) -> Option<usize> {
    let ExpressionKind::Integer(text) = &expression.kind else {
        return None;
    };
    let integer_text = text.trim_end_matches('L');
    let one_based_index = integer_text.parse::<usize>().ok()?;
    one_based_index.checked_sub(1)
}

fn literal_name_symbol(expression: &Expression) -> Option<Symbol> {
    let ExpressionKind::StringLiteralName(symbol) = &expression.kind else {
        return None;
    };
    Some(*symbol)
}

fn index_type_mismatch(
    expected: CoreType,
    actual: CoreType,
    expression: &Expression,
) -> InferenceError {
    InferenceError::TypeMismatch {
        expected: Box::new(expected),
        actual: Box::new(actual),
        range: Some(expression.range),
        expression_id: Some(expression.id),
    }
}

fn core_type_from_surface_type(surface_type: &SurfaceType) -> CoreType {
    match surface_type {
        SurfaceType::Any => CoreType::Any,
        SurfaceType::Unknown => CoreType::Unknown,
        SurfaceType::Null => CoreType::Null,
        SurfaceType::Nullable(inner_type) => nullable_type(core_type_from_surface_type(inner_type)),
        SurfaceType::Scalar(atomic) => CoreType::Scalar(*atomic),
        SurfaceType::Named(_, _) => CoreType::Unknown,
        SurfaceType::Vector(inner_type) => match core_type_from_surface_type(inner_type) {
            CoreType::Scalar(atomic) => CoreType::Vector(atomic),
            other_type => CoreType::List(Box::new(other_type)),
        },
        SurfaceType::NamedVector(inner_type) => match core_type_from_surface_type(inner_type) {
            CoreType::Scalar(atomic) => CoreType::NamedVector(atomic),
            other_type => CoreType::NamedList(Box::new(other_type)),
        },
        SurfaceType::List(item_type) => {
            CoreType::List(Box::new(core_type_from_surface_type(item_type)))
        }
        SurfaceType::NamedList(item_type) => {
            CoreType::NamedList(Box::new(core_type_from_surface_type(item_type)))
        }
        SurfaceType::Record(fields) => CoreType::Record(
            fields
                .iter()
                .map(|field| {
                    RecordField::new(field.name, core_type_from_surface_type(&field.value))
                })
                .collect(),
        ),
        SurfaceType::Tuple(items) => {
            CoreType::Tuple(items.iter().map(core_type_from_surface_type).collect())
        }
        SurfaceType::Function(function_type) => CoreType::Function(FunctionType::new(
            function_type
                .parameters
                .iter()
                .map(core_type_from_surface_type)
                .collect(),
            function_type
                .named_parameters
                .iter()
                .map(|parameter| {
                    RecordField::new(
                        parameter.name,
                        core_type_from_surface_type(&parameter.value),
                    )
                })
                .collect(),
            core_type_from_surface_type(&function_type.return_type),
        )),
        SurfaceType::Binders(_, inner_type) => core_type_from_surface_type(inner_type),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferenceError {
    UnknownInferenceVariable(InferenceVariableId),
    UnknownName {
        symbol: Symbol,
        range: Range,
        expression_id: ExpressionId,
    },
    ExpectedFunction {
        actual_type: CoreType,
        range: Range,
        expression_id: ExpressionId,
    },
    OccursCheckFailed {
        variable: InferenceVariableId,
        in_type: CoreType,
        range: Option<Range>,
        expression_id: Option<ExpressionId>,
    },
    TypeMismatch {
        expected: Box<CoreType>,
        actual: Box<CoreType>,
        range: Option<Range>,
        expression_id: Option<ExpressionId>,
    },
    InvalidPlusOperand {
        actual: CoreType,
        range: Range,
        expression_id: ExpressionId,
    },
    TupleLengthMismatch {
        expected: usize,
        actual: usize,
        range: Option<Range>,
        expression_id: Option<ExpressionId>,
    },
    MixedListElements {
        range: Option<Range>,
        expression_id: Option<ExpressionId>,
    },
    RecordFieldMismatch {
        expected_fields: Vec<Symbol>,
        actual_fields: Vec<Symbol>,
        range: Option<Range>,
        expression_id: Option<ExpressionId>,
    },
    FunctionArityMismatch {
        expected: usize,
        actual: usize,
        range: Option<Range>,
        expression_id: Option<ExpressionId>,
    },
    NamedParameterMismatch {
        expected_parameters: Vec<Symbol>,
        actual_parameters: Vec<Symbol>,
        range: Option<Range>,
        expression_id: Option<ExpressionId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub type_scheme: TypeScheme,
    pub range: Range,
}
