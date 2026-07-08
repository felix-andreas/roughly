// The irreducible builtin kernel: the operators and core constructors no stub declaration can
// express — arithmetic with R's shape/atomic promotion, comparison with the flexible-operand
// scalar claim, `:`, unary minus/not, `&&`/`||`, `c()` combination, `switch`, `list()` (tuples
// and records), and the `[` / `[[` / `$` indexing forms. The typing-reference sections on
// operators, indexing, and coercions are the contract.
use {
    super::{
        InferenceError, InferenceState, ResolutionContext, StrictOriginKind,
        operand::{
            ComparisonFamily, NumericOperand, NumericResultAtomic, OperandShape,
            classify_numeric_operand, combine_operand_atomic, comparison_operand_parts_list,
            core_type_for_shape, flexible_comparison_operand, integer_literal_position,
            is_whole_number_double_literal, literal_name_symbol, member_wise_numeric_results,
            nullable_type, promote_combine_atomic, shapes_for_operand,
            widen_error_container_to_union,
        },
    },
    crate::{
        hir::{Argument, Expression, HirArena},
        interner::Symbol,
        typecheck::{BuiltinKind, OperandExpectation, TypeDefinitionEnvironment},
        types::{Atomic, Constraint, CoreType, InferenceVariableId, RecordField},
    },
};

impl InferenceState {
    pub(super) fn infer_builtin_call(
        &mut self,
        symbol: Symbol,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<Option<CoreType>, InferenceError> {
        let Some(builtin_kind) = self.builtins.get(&symbol).copied() else {
            return Ok(None);
        };

        match builtin_kind {
            BuiltinKind::Modulo | BuiltinKind::IntegerDivide => self
                .infer_binary_numeric(
                    arguments,
                    expression,
                    NumericResultAtomic::Promote,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
            BuiltinKind::Colon => self
                .infer_builtin_colon(
                    arguments,
                    expression,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
            BuiltinKind::Compare => self
                .infer_builtin_compare(
                    arguments,
                    expression,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
            BuiltinKind::Plus => self
                .infer_binary_numeric(
                    arguments,
                    expression,
                    NumericResultAtomic::Promote,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
            BuiltinKind::Minus => self
                .infer_binary_numeric(
                    arguments,
                    expression,
                    NumericResultAtomic::Promote,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
            BuiltinKind::Multiply => self
                .infer_binary_numeric(
                    arguments,
                    expression,
                    NumericResultAtomic::Promote,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
            BuiltinKind::Divide => self
                .infer_binary_numeric(
                    arguments,
                    expression,
                    NumericResultAtomic::AlwaysDouble,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
            BuiltinKind::Power => self
                .infer_binary_numeric(
                    arguments,
                    expression,
                    NumericResultAtomic::AlwaysDouble,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
            BuiltinKind::And => self
                .infer_builtin_boolean_binary(
                    arguments,
                    expression,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
            BuiltinKind::Or => self
                .infer_builtin_boolean_binary(
                    arguments,
                    expression,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
            BuiltinKind::Combine => self
                .infer_builtin_combine(
                    arguments,
                    expression,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
            BuiltinKind::List => self
                .infer_builtin_list(
                    arguments,
                    expression,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
            BuiltinKind::Switch => self
                .infer_builtin_switch(
                    arguments,
                    expression,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
        }
    }

    pub(super) fn infer_binary_numeric(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        numeric_result_atomic: NumericResultAtomic,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
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

        let left_type =
            self.infer_expression_with_context(arg0, arena, resolution_context, type_definitions)?;
        let right_type =
            self.infer_expression_with_context(arg1, arena, resolution_context, type_definitions)?;

        let resolved_left = self.resolve_structural(left_type, type_definitions, Some(arg0))?;
        let resolved_right = self.resolve_structural(right_type, type_definitions, Some(arg1))?;

        let left = classify_numeric_operand(&resolved_left);
        let right = classify_numeric_operand(&resolved_right);

        if let NumericOperand::Invalid = left {
            return Err(InferenceError::InvalidOperand {
                expected: OperandExpectation::Numeric,
                actual: Box::new(resolved_left),
                range: arg0.range,
                expression_id: arg0.id,
            });
        }
        if let NumericOperand::Invalid = right {
            return Err(InferenceError::InvalidOperand {
                expected: OperandExpectation::Numeric,
                actual: Box::new(resolved_right),
                range: arg1.range,
                expression_id: arg1.id,
            });
        }
        if matches!(left, NumericOperand::AnyUnknown) || matches!(right, NumericOperand::AnyUnknown)
        {
            return Ok(CoreType::Unknown);
        }

        // Constrain every flexible operand to be numeric, collapsing them onto one representative
        // variable so `x + y` ties the two operands together.
        let mut flexible_variable: Option<InferenceVariableId> = None;
        for operand in [&left, &right] {
            if let NumericOperand::Variable(variable) = operand {
                flexible_variable = Some(match flexible_variable {
                    Some(existing) => match self
                        .unify(CoreType::Variable(existing), CoreType::Variable(*variable))?
                    {
                        CoreType::Variable(unified) => unified,
                        _ => existing,
                    },
                    None => *variable,
                });
            }
        }
        if let Some(variable) = flexible_variable {
            self.constrain_type(
                CoreType::Variable(variable),
                Constraint::Numeric,
                Some(expression),
            )?;
        }

        // A generic vector element (`T[]`) used arithmetically must be numeric; joined with the
        // atomic-element bound it already carries, the element becomes scalar-numeric.
        for operand in [&left, &right] {
            if let NumericOperand::FlexibleVector(Some(element_variable)) = operand {
                self.constrain_type(
                    CoreType::Variable(*element_variable),
                    Constraint::Numeric,
                    Some(expression),
                )?;
            }
        }

        // A flexible-element vector operand fixes the result shape (vector) without fixing the
        // atomic. Mirroring the scalar flexible-operand rules: an always-double operation or a
        // concrete `double` (or union) partner promotes to `double[]`; an integer partner promotes
        // *into* the element, so the result keeps the element variable; two generic elements are
        // unified; an untracked (`Any`/`Unknown`) element stays untracked.
        let flexible_vector_present = matches!(left, NumericOperand::FlexibleVector(_))
            || matches!(right, NumericOperand::FlexibleVector(_));
        if flexible_vector_present {
            if let NumericResultAtomic::AlwaysDouble = numeric_result_atomic {
                return Ok(CoreType::vector(Atomic::Double));
            }
            let concrete_parts = left.concrete_parts().or_else(|| right.concrete_parts());
            if let Some(parts) = &concrete_parts
                && (parts.len() > 1 || parts.iter().any(|(_, atomic)| *atomic == Atomic::Double))
            {
                return Ok(CoreType::vector(Atomic::Double));
            }
            let element = match (&left, &right) {
                (
                    NumericOperand::FlexibleVector(Some(left_element)),
                    NumericOperand::FlexibleVector(Some(right_element)),
                ) => Some(self.unify(
                    CoreType::Variable(*left_element),
                    CoreType::Variable(*right_element),
                )?),
                (NumericOperand::FlexibleVector(None), _)
                | (_, NumericOperand::FlexibleVector(None)) => None,
                (NumericOperand::FlexibleVector(Some(element)), _)
                | (_, NumericOperand::FlexibleVector(Some(element))) => {
                    Some(CoreType::Variable(*element))
                }
                _ => None,
            };
            return Ok(CoreType::Vector(Box::new(
                element.unwrap_or(CoreType::Unknown),
            )));
        }

        match (left.concrete_parts(), right.concrete_parts()) {
            // Member-wise: the operation applies to every pair of operand members, and the result
            // is the join of the per-pair results. A single concrete operand is the one-member
            // case, so this arm also carries the ordinary concrete/concrete path: both-`integer`
            // pairs stay `integer`, any `double` promotes the pair, and a vector member makes the
            // pair's result a vector.
            (Some(left_parts), Some(right_parts)) => Ok(CoreType::union_of(
                member_wise_numeric_results(&left_parts, &right_parts, numeric_result_atomic),
            )),
            (left_parts, right_parts) => {
                let variable = flexible_variable
                    .expect("a non-concrete numeric operand classifies as a variable");
                let concrete_parts = left_parts.or(right_parts);
                if let Some(parts) = &concrete_parts
                    && parts.len() > 1
                {
                    // A union operand cannot promote into a variable member-wise, so the flexible
                    // side is pinned to the default numeric scalar (`double`) — the same default a
                    // vector result applies below — and the operation continues member-wise.
                    self.bind_variable(
                        variable,
                        CoreType::Scalar(Atomic::Double),
                        Some(expression),
                    )?;
                    return Ok(CoreType::union_of(member_wise_numeric_results(
                        &[(OperandShape::Scalar, Atomic::Double)],
                        parts,
                        numeric_result_atomic,
                    )));
                }

                let concrete = concrete_parts.and_then(|parts| parts.first().copied());
                let result_shape = match concrete {
                    Some((OperandShape::Vector, _)) => OperandShape::Vector,
                    _ => OperandShape::Scalar,
                };
                if let NumericResultAtomic::AlwaysDouble = numeric_result_atomic {
                    return Ok(core_type_for_shape(result_shape, Atomic::Double));
                }
                // Promote: a concrete `double` anywhere forces `double`.
                if concrete.map(|(_, atomic)| atomic) == Some(Atomic::Double) {
                    return Ok(core_type_for_shape(result_shape, Atomic::Double));
                }
                match result_shape {
                    // `x + 1L` (and `x + y`) stay polymorphic over the numeric operand: integer
                    // promotes to whatever the variable resolves to, so the scalar result is the
                    // variable itself.
                    OperandShape::Scalar => Ok(CoreType::Variable(variable)),
                    // A vector result cannot carry an unresolved atomic, so a flexible operand
                    // defaults to `double` here.
                    OperandShape::Vector => {
                        self.bind_variable(
                            variable,
                            CoreType::Scalar(Atomic::Double),
                            Some(expression),
                        )?;
                        Ok(CoreType::vector(Atomic::Double))
                    }
                }
            }
        }
    }

    pub(super) fn infer_unary_minus(
        &mut self,
        value: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        let inferred_type =
            self.infer_expression_with_context(value, arena, resolution_context, type_definitions)?;
        let resolved_type =
            self.resolve_structural(inferred_type, type_definitions, Some(value))?;

        match classify_numeric_operand(&resolved_type) {
            NumericOperand::Concrete(shape, atomic) => Ok(core_type_for_shape(shape, atomic)),
            // Member-wise over a union operand: negation preserves each member's shape and atomic,
            // so the result is the same union.
            NumericOperand::ConcreteUnion(parts) => Ok(CoreType::union_of(
                parts
                    .into_iter()
                    .map(|(shape, atomic)| core_type_for_shape(shape, atomic))
                    .collect(),
            )),
            NumericOperand::Variable(variable) => {
                self.constrain_type(
                    CoreType::Variable(variable),
                    Constraint::Numeric,
                    Some(value),
                )?;
                Ok(CoreType::Variable(variable))
            }
            // Negation is elementwise and type-preserving, so a generic-element vector keeps its
            // element (constrained numeric) and an untracked element stays untracked.
            NumericOperand::FlexibleVector(element_variable) => {
                if let Some(element_variable) = element_variable {
                    self.constrain_type(
                        CoreType::Variable(element_variable),
                        Constraint::Numeric,
                        Some(value),
                    )?;
                }
                Ok(resolved_type)
            }
            NumericOperand::AnyUnknown => Ok(CoreType::Unknown),
            NumericOperand::Invalid => Err(InferenceError::InvalidOperand {
                expected: OperandExpectation::Numeric,
                actual: Box::new(resolved_type),
                range: value.range,
                expression_id: value.id,
            }),
        }
    }

    pub(super) fn infer_unary_not(
        &mut self,
        value: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        let inferred_type =
            self.infer_expression_with_context(value, arena, resolution_context, type_definitions)?;
        let resolved_type =
            self.resolve_structural(inferred_type, type_definitions, Some(value))?;

        match resolved_type {
            CoreType::Scalar(Atomic::Logical) => Ok(CoreType::Scalar(Atomic::Logical)),
            CoreType::Vector(ref element) | CoreType::NamedVector(ref element)
                if matches!(
                    element.as_ref(),
                    CoreType::Scalar(Atomic::Logical)
                        | CoreType::Variable(_)
                        | CoreType::Any
                        | CoreType::Unknown
                ) =>
            {
                Ok(CoreType::vector(Atomic::Logical))
            }
            CoreType::Any | CoreType::Unknown => Ok(CoreType::Unknown),
            CoreType::Variable(_) => {
                self.unify_with_context(CoreType::Scalar(Atomic::Logical), resolved_type, value)?;
                Ok(CoreType::Scalar(Atomic::Logical))
            }
            other_type => Err(InferenceError::InvalidOperand {
                expected: OperandExpectation::Logical,
                actual: Box::new(other_type),
                range: value.range,
                expression_id: value.id,
            }),
        }
    }

    pub(super) fn infer_builtin_compare(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
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
        let left_type =
            self.infer_expression_with_context(arg0, arena, resolution_context, type_definitions)?;
        let right_type =
            self.infer_expression_with_context(arg1, arena, resolution_context, type_definitions)?;
        let resolved_left = self.resolve_structural(left_type, type_definitions, Some(arg0))?;
        let resolved_right = self.resolve_structural(right_type, type_definitions, Some(arg1))?;

        if matches!(resolved_left, CoreType::Any | CoreType::Unknown)
            || matches!(resolved_right, CoreType::Any | CoreType::Unknown)
        {
            return Ok(CoreType::Unknown);
        }

        let left_parts = comparison_operand_parts_list(&resolved_left);
        let right_parts = comparison_operand_parts_list(&resolved_right);
        let left_flexible = flexible_comparison_operand(&resolved_left);
        let right_flexible = flexible_comparison_operand(&resolved_right);
        let left_is_variable = left_flexible.is_some();
        let right_is_variable = right_flexible.is_some();

        if left_parts.is_none() && !left_is_variable {
            return Err(InferenceError::InvalidOperand {
                expected: OperandExpectation::Comparable,
                actual: Box::new(resolved_left),
                range: arg0.range,
                expression_id: arg0.id,
            });
        }
        if right_parts.is_none() && !right_is_variable {
            return Err(InferenceError::InvalidOperand {
                expected: OperandExpectation::Comparable,
                actual: Box::new(resolved_right),
                range: arg1.range,
                expression_id: arg1.id,
            });
        }

        // Two concrete operands must belong to the same comparison family, member-wise: every
        // shape the left union can take must be comparable with every shape of the right.
        if let (Some(left_parts), Some(right_parts)) = (&left_parts, &right_parts)
            && left_parts.iter().any(|(_, left_family)| {
                right_parts
                    .iter()
                    .any(|(_, right_family)| left_family != right_family)
            })
        {
            return Err(InferenceError::TypeMismatch {
                expected: Box::new(resolved_left),
                actual: Box::new(resolved_right),
                range: Some(expression.range),
                expression_id: Some(expression.id),
            });
        }

        // A flexible operand compared against a concrete numeric operand is constrained numeric;
        // comparison against a non-numeric family leaves it free, since the type system has no
        // character-or-logical constraint.
        let all_numeric = |parts: &Option<Vec<(OperandShape, ComparisonFamily)>>| {
            parts.as_ref().is_some_and(|parts| {
                parts
                    .iter()
                    .all(|(_, family)| *family == ComparisonFamily::Numeric)
            })
        };
        if let Some(flexible) = &left_flexible
            && all_numeric(&right_parts)
            && let Some(variable) = flexible.variable()
        {
            self.constrain_type(
                CoreType::Variable(variable),
                Constraint::Numeric,
                Some(arg0),
            )?;
        }
        if let Some(flexible) = &right_flexible
            && all_numeric(&left_parts)
            && let Some(variable) = flexible.variable()
        {
            self.constrain_type(
                CoreType::Variable(variable),
                Constraint::Numeric,
                Some(arg1),
            )?;
        }

        // Member-wise result: a pair with a vector member compares element-wise (`logical[]`), a
        // scalar-scalar pair compares to `logical`; a union operand mixing shapes therefore yields
        // the join of both. A flexible-element vector operand has no concrete parts but a known
        // vector shape.
        let left_shapes = shapes_for_operand(&left_parts, &left_flexible);
        let right_shapes = shapes_for_operand(&right_parts, &right_flexible);
        let mut results = Vec::new();
        for left_shape in &left_shapes {
            for right_shape in &right_shapes {
                let result_shape = if *left_shape == OperandShape::Vector
                    || *right_shape == OperandShape::Vector
                {
                    OperandShape::Vector
                } else {
                    OperandShape::Scalar
                };
                results.push(core_type_for_shape(result_shape, Atomic::Logical));
            }
        }
        Ok(CoreType::union_of(results))
    }

    pub(super) fn infer_builtin_colon(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        if arguments.len() != 2 {
            return Err(InferenceError::FunctionArityMismatch {
                expected: 2,
                actual: arguments.len(),
                range: Some(expression.range),
                expression_id: Some(expression.id),
            });
        }

        let mut result_atomic = Atomic::Integer;
        for argument in arguments {
            let argument_expression = arena.get(argument.expression);
            let inferred_argument = self.infer_expression_with_context(
                argument_expression,
                arena,
                resolution_context,
                type_definitions,
            )?;
            let resolved_argument = self.resolve_structural(
                inferred_argument,
                type_definitions,
                Some(argument_expression),
            )?;
            match resolved_argument {
                CoreType::Scalar(Atomic::Integer) => {}
                // R's `:` yields an integer sequence for whole-number endpoints, so
                // whole-number double literals like `1` in `1:10` count as integer here.
                CoreType::Scalar(Atomic::Double)
                    if is_whole_number_double_literal(argument_expression) => {}
                CoreType::Scalar(Atomic::Double) => result_atomic = Atomic::Double,
                CoreType::Any | CoreType::Unknown => return Ok(CoreType::Unknown),
                // A flexible endpoint such as `1:n` must be a scalar number — the plain numeric
                // bound admits numeric vectors, which R only warns about and truncates to the
                // first element. It is also not known to be `integer`; it may resolve to `double`,
                // so the result must be `double[]` (claiming `integer[]` would be unsound when the
                // endpoint instantiates at `double`).
                CoreType::Variable(variable) => {
                    self.constrain_type(
                        CoreType::Variable(variable),
                        Constraint::ScalarNumeric,
                        Some(argument_expression),
                    )?;
                    result_atomic = Atomic::Double;
                }
                other_type => {
                    return Err(InferenceError::InvalidOperand {
                        expected: OperandExpectation::ScalarNumeric,
                        actual: Box::new(other_type),
                        range: argument_expression.range,
                        expression_id: argument_expression.id,
                    });
                }
            }
        }

        Ok(CoreType::vector(result_atomic))
    }

    pub(super) fn infer_subset_expression(
        &mut self,
        value: &Expression,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        // The subject and every index are inferred first regardless of shape, so names inside an
        // unsupported form (`m[i, j]`) still resolve and get their own diagnostics.
        let inferred_value =
            self.infer_expression_with_context(value, arena, resolution_context, type_definitions)?;
        let value_type = self.resolve_structural(inferred_value, type_definitions, Some(value))?;
        for argument in arguments {
            let argument_expression = arena.get(argument.expression);
            self.infer_expression_with_context(
                argument_expression,
                arena,
                resolution_context,
                type_definitions,
            )?;
        }

        // An Unknown/Any subject stays Unknown/Any even under an unsupported index shape — the
        // subject's own gap was already diagnosed, so `m[i, j]` must not cascade an arity error.
        if matches!(value_type, CoreType::Unknown) {
            return Ok(CoreType::Unknown);
        }
        if matches!(value_type, CoreType::Any) {
            return Ok(CoreType::Any);
        }
        // A sealed nominal supports value-dependent indexing of any shape at runtime
        // (`df[rows, cols]`, `df[predicate, ]`), none of it modeled: `Unknown`, before the
        // index-arity check, so idiomatic two-index data.frame subsetting is not an error.
        if matches!(value_type, CoreType::Nominal(..)) {
            self.record_strict_origin(
                expression.id,
                expression.range,
                StrictOriginKind::UnsupportedConstruct,
            );
            return Ok(CoreType::Unknown);
        }
        if arguments.len() != 1 || arguments[0].name.is_some() {
            return Err(InferenceError::UnsupportedIndexShape {
                index_count: arguments.len(),
                range: expression.range,
                expression_id: expression.id,
            });
        }

        self.subset_result_type(value_type, value, expression, type_definitions)
    }

    pub(super) fn subset_result_type(
        &mut self,
        value_type: CoreType,
        value: &Expression,
        expression: &Expression,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        // Member-wise over a union subject: `[` must be valid on every shape the subject can take,
        // and the slice's type is the join of the per-member results. A failing member reports the
        // full union — the subject's actual type — not the single member that failed.
        if let CoreType::Union(members) = value_type {
            let union_type = CoreType::Union(members.clone());
            let mut results = Vec::with_capacity(members.len());
            for member in members {
                let member = self.resolve_structural(member, type_definitions, Some(value))?;
                let result = self
                    .subset_result_type(member, value, expression, type_definitions)
                    .map_err(|error| widen_error_container_to_union(error, &union_type))?;
                results.push(result);
            }
            return Ok(CoreType::union_of(results));
        }

        match value_type {
            CoreType::Unknown => Ok(CoreType::Unknown),
            CoreType::Any => Ok(CoreType::Any),
            CoreType::List(item_type) => Ok(CoreType::List(item_type)),
            CoreType::NamedList(item_type) => Ok(CoreType::NamedList(item_type)),
            // A `[` slice of a fixed-shape list is a sub-list that can contain any of the item
            // types, so the element type is their union (collapsing back to the single item type
            // for a homogeneous list; slicing the empty list yields `list[NULL]`).
            CoreType::Tuple(items) => Ok(CoreType::List(Box::new(CoreType::union_of(items)))),
            CoreType::Record(fields) => Ok(CoreType::NamedList(Box::new(CoreType::union_of(
                fields.iter().map(|field| field.value.clone()).collect(),
            )))),
            // A sealed nominal has no modeled structure, but the R object behind it commonly
            // supports `[` with a value-dependent result (`df[rows]`, `f[levels]`): the slice is
            // `Unknown` — sound-by-refusal, surfaced under strict mode — not a hard error.
            CoreType::Nominal(..) => {
                self.record_strict_origin(
                    expression.id,
                    expression.range,
                    StrictOriginKind::UnsupportedConstruct,
                );
                Ok(CoreType::Unknown)
            }
            // An unresolved inference variable — an unannotated parameter sliced with `[`
            // (`function(x) x[1L]`) — cannot be structurally resolved here, so the slice is
            // `Unknown` rather than a rejection: the same sound-by-refusal the `Nominal` arm above
            // takes, and the tolerance `[[`/`$` give the same subject. The variable is left
            // unconstrained; the `Unknown` is a strict-mode origin.
            CoreType::Variable(_) => {
                self.record_strict_origin(
                    expression.id,
                    expression.range,
                    StrictOriginKind::UnsupportedConstruct,
                );
                Ok(CoreType::Unknown)
            }
            other_type => Err(InferenceError::UnsupportedSubset {
                actual: Box::new(other_type),
                range: expression.range,
                expression_id: expression.id,
            }),
        }
    }

    pub(super) fn infer_subset2_expression(
        &mut self,
        value: &Expression,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        let inferred_value =
            self.infer_expression_with_context(value, arena, resolution_context, type_definitions)?;
        let value_type = self.resolve_structural(inferred_value, type_definitions, Some(value))?;
        for argument in arguments {
            let argument_expression = arena.get(argument.expression);
            self.infer_expression_with_context(
                argument_expression,
                arena,
                resolution_context,
                type_definitions,
            )?;
        }

        if matches!(value_type, CoreType::Unknown) {
            return Ok(CoreType::Unknown);
        }
        if matches!(value_type, CoreType::Any) {
            return Ok(CoreType::Any);
        }
        // A sealed nominal: value-dependent element access, unmodeled — `Unknown` before the
        // index-arity check, exactly as for `[`.
        if matches!(value_type, CoreType::Nominal(..)) {
            self.record_strict_origin(
                expression.id,
                expression.range,
                StrictOriginKind::UnsupportedConstruct,
            );
            return Ok(CoreType::Unknown);
        }
        if arguments.len() != 1 || arguments[0].name.is_some() {
            return Err(InferenceError::UnsupportedIndexShape {
                index_count: arguments.len(),
                range: expression.range,
                expression_id: expression.id,
            });
        }
        let index_expression = arena.get(arguments[0].expression);

        self.subset2_result_type(
            value_type,
            value,
            index_expression,
            expression,
            type_definitions,
        )
    }

    pub(super) fn subset2_result_type(
        &mut self,
        value_type: CoreType,
        value: &Expression,
        index_expression: &Expression,
        expression: &Expression,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        // Member-wise over a union subject: `[[` must be valid on every shape the subject can
        // take, and the element's type is the join of the per-member results. A failing member
        // reports the full union — the subject's actual type — not the single member that failed.
        if let CoreType::Union(members) = value_type {
            let union_type = CoreType::Union(members.clone());
            let mut results = Vec::with_capacity(members.len());
            for member in members {
                let member = self.resolve_structural(member, type_definitions, Some(value))?;
                let result = self
                    .subset2_result_type(
                        member,
                        value,
                        index_expression,
                        expression,
                        type_definitions,
                    )
                    .map_err(|error| widen_error_container_to_union(error, &union_type))?;
                results.push(result);
            }
            return Ok(CoreType::union_of(results));
        }

        match value_type {
            CoreType::Unknown => Ok(CoreType::Unknown),
            CoreType::Any => Ok(CoreType::Any),
            CoreType::Scalar(atomic) => Ok(CoreType::Scalar(atomic)),
            CoreType::Vector(element) => Ok(*element),
            CoreType::NamedVector(element) => {
                if literal_name_symbol(index_expression).is_some() {
                    Ok(nullable_type(*element))
                } else {
                    Ok(*element)
                }
            }
            CoreType::List(item_type) => Ok(*item_type),
            // Mirrors the map-like vector arm: a literal name may be absent at runtime (`T | NULL`),
            // while positional and computed access extract an item like R does on any list.
            CoreType::NamedList(item_type) => {
                if literal_name_symbol(index_expression).is_some() {
                    Ok(nullable_type(*item_type))
                } else {
                    Ok(*item_type)
                }
            }
            CoreType::Tuple(items) => {
                // A computed position could reach any item, so the extraction is the union of the
                // item types — the same rule `for` iteration over a fixed-shape list uses. Only a
                // *literal* position is precise (and still errors when out of range).
                let Some(index) = integer_literal_position(index_expression) else {
                    return Ok(CoreType::union_of(items));
                };
                match items.get(index).cloned() {
                    Some(item_type) => Ok(item_type),
                    None => Err(InferenceError::PositionDoesNotExist {
                        position: index + 1,
                        container: Box::new(CoreType::Tuple(items)),
                        range: expression.range,
                        expression_id: expression.id,
                    }),
                }
            }
            CoreType::Record(fields) => {
                // Record fields are declaration-ordered, so R's positional `[[` extracts the
                // field at that position exactly like a tuple item.
                if let Some(index) = integer_literal_position(index_expression) {
                    return match fields.get(index) {
                        Some(field) => Ok(field.value.clone()),
                        None => Err(InferenceError::PositionDoesNotExist {
                            position: index + 1,
                            container: Box::new(CoreType::Record(fields)),
                            range: expression.range,
                            expression_id: expression.id,
                        }),
                    };
                }
                // A computed name could reach any field (the dispatch-table idiom,
                // `handlers[[name]](...)`), so the extraction is the union of the field types —
                // mirroring `for` iteration over a record. Only a *literal* name is precise (and
                // still errors when the field does not exist).
                let Some(name) = literal_name_symbol(index_expression) else {
                    return Ok(CoreType::union_of(
                        fields.into_iter().map(|field| field.value).collect(),
                    ));
                };
                match fields.iter().find(|field| field.name == name) {
                    Some(field) => Ok(field.value.clone()),
                    None => Err(InferenceError::FieldDoesNotExist {
                        field: name,
                        container: Box::new(CoreType::Record(fields)),
                        range: expression.range,
                        expression_id: expression.id,
                    }),
                }
            }
            // A sealed nominal has no modeled structure, but the R object behind it commonly
            // supports `[[` with a value-dependent result (`df[["col"]]`): the element is
            // `Unknown` — sound-by-refusal, surfaced under strict mode — not a hard error.
            CoreType::Nominal(..) => {
                self.record_strict_origin(
                    expression.id,
                    expression.range,
                    StrictOriginKind::UnsupportedConstruct,
                );
                Ok(CoreType::Unknown)
            }
            // An unresolved inference variable — an unannotated parameter indexed with `[[`
            // (`function(x) x[[1L]]`) — cannot be structurally resolved here, so the element is
            // `Unknown` rather than a rejection: the same sound-by-refusal the `Nominal` arm above
            // takes, and the way arithmetic tolerates an unconstrained operand. The variable is left
            // unconstrained; the `Unknown` is a strict-mode origin.
            CoreType::Variable(_) => {
                self.record_strict_origin(
                    expression.id,
                    expression.range,
                    StrictOriginKind::UnsupportedConstruct,
                );
                Ok(CoreType::Unknown)
            }
            other_type => Err(InferenceError::NotAList {
                actual: Box::new(other_type),
                range: expression.range,
                expression_id: expression.id,
            }),
        }
    }

    pub(super) fn infer_dollar_expression(
        &mut self,
        value: &Expression,
        name: Symbol,
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        let inferred_value =
            self.infer_expression_with_context(value, arena, resolution_context, type_definitions)?;
        let value_type = self.resolve_structural(inferred_value, type_definitions, Some(value))?;

        self.dollar_result_type(value_type, value, name, expression, type_definitions)
    }

    pub(super) fn dollar_result_type(
        &mut self,
        value_type: CoreType,
        value: &Expression,
        name: Symbol,
        expression: &Expression,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        // Member-wise over a union subject: the field must exist on every shape the subject can
        // take, and its type is the join of the per-member results. A failing member reports the
        // full union — the subject's actual type — not the single member that failed.
        if let CoreType::Union(members) = value_type {
            let union_type = CoreType::Union(members.clone());
            let mut results = Vec::with_capacity(members.len());
            for member in members {
                let member = self.resolve_structural(member, type_definitions, Some(value))?;
                let result = self
                    .dollar_result_type(member, value, name, expression, type_definitions)
                    .map_err(|error| widen_error_container_to_union(error, &union_type))?;
                results.push(result);
            }
            return Ok(CoreType::union_of(results));
        }

        match value_type {
            CoreType::Unknown => Ok(CoreType::Unknown),
            CoreType::Any => Ok(CoreType::Any),
            // R rejects `$` on every atomic vector ("$ operator is invalid for atomic vectors"),
            // named ones included — element extraction is `[[`'s job.
            atomic @ (CoreType::Scalar(_) | CoreType::Vector(_) | CoreType::NamedVector(_)) => {
                Err(InferenceError::DollarOnAtomicVector {
                    actual: Box::new(atomic),
                    range: expression.range,
                    expression_id: expression.id,
                })
            }
            CoreType::NamedList(item_type) => Ok(nullable_type(*item_type)),
            CoreType::Record(fields) => match fields.iter().find(|field| field.name == name) {
                Some(field) => Ok(field.value.clone()),
                None => Err(InferenceError::FieldDoesNotExist {
                    field: name,
                    container: Box::new(CoreType::Record(fields)),
                    range: expression.range,
                    expression_id: expression.id,
                }),
            },
            container @ (CoreType::Tuple(_) | CoreType::List(_)) => {
                Err(InferenceError::FieldDoesNotExist {
                    field: name,
                    container: Box::new(container),
                    range: expression.range,
                    expression_id: expression.id,
                })
            }
            // A sealed nominal (`data.frame`, `factor`, ...) has no modeled structure, but the R
            // object behind it commonly supports `$` with a value-dependent result (`df$col`).
            // Refusing loudly here would error on the most idiomatic R there is, so the access is
            // `Unknown` — sound-by-refusal, surfaced under strict mode.
            CoreType::Nominal(..) => {
                self.record_strict_origin(
                    expression.id,
                    expression.range,
                    StrictOriginKind::UnsupportedConstruct,
                );
                Ok(CoreType::Unknown)
            }
            // An unresolved inference variable — an unannotated parameter used as a `$` subject
            // (`function(node) node$value`, idiomatic R) — cannot be structurally resolved here, so
            // the access is `Unknown` rather than a rejection, matching how arithmetic tolerates an
            // unconstrained operand. The variable is left unconstrained (a structural
            // record-with-field constraint that would recover the field type is future work); the
            // `Unknown` is a strict-mode origin, so strict mode still nudges an annotation.
            CoreType::Variable(_) => {
                self.record_strict_origin(
                    expression.id,
                    expression.range,
                    StrictOriginKind::UnsupportedConstruct,
                );
                Ok(CoreType::Unknown)
            }
            other_type => Err(InferenceError::NotAList {
                actual: Box::new(other_type),
                range: expression.range,
                expression_id: expression.id,
            }),
        }
    }

    pub(super) fn infer_builtin_boolean_binary(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        if arguments.len() != 2 {
            return Err(InferenceError::FunctionArityMismatch {
                expected: 2,
                actual: arguments.len(),
                range: Some(expression.range),
                expression_id: Some(expression.id),
            });
        }

        self.expect_scalar_logical(
            arena.get(arguments[0].expression),
            arena,
            resolution_context,
            type_definitions,
        )?;
        self.expect_scalar_logical(
            arena.get(arguments[1].expression),
            arena,
            resolution_context,
            type_definitions,
        )?;
        Ok(CoreType::Scalar(Atomic::Logical))
    }

    pub(super) fn infer_builtin_combine(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        if arguments.is_empty() {
            return Ok(CoreType::Null);
        }

        let mut item_atomic = None;
        let mut all_arguments_are_named = true;
        let mut saw_non_null_argument = false;
        let mut result_indeterminate = false;

        for argument in arguments {
            let arg_expr = arena.get(argument.expression);
            let inferred_argument = self.infer_expression_with_context(
                arg_expr,
                arena,
                resolution_context,
                type_definitions,
            )?;
            let resolved_argument =
                self.resolve_structural(inferred_argument, type_definitions, Some(arg_expr))?;

            // R drops `NULL` inside `c(...)`: `c(x, NULL)` is `c(x)` and `c(NULL)` is `NULL`.
            if resolved_argument == CoreType::Null {
                continue;
            }
            saw_non_null_argument = true;
            all_arguments_are_named &= argument.name.is_some();

            // A non-concrete argument whose element atomic is not statically known — `Any`
            // (compatible with everything), `Unknown` (which must never cascade), or an unresolved
            // inference variable (an unannotated parameter, `function(x) c(x, 1L)`) — cannot pin the
            // combined element type. R's `c` still returns a vector, but its element atomic is
            // indeterminate here, so the whole result is `Unknown` rather than a rejection or an
            // unsound concrete claim (a later argument could widen the atomic). A variable is left
            // unconstrained and marked a strict-mode origin, mirroring `$`/`[[`/`[` on the same
            // subject.
            match &resolved_argument {
                CoreType::Any | CoreType::Unknown => {
                    result_indeterminate = true;
                    continue;
                }
                CoreType::Variable(_) => {
                    self.record_strict_origin(
                        expression.id,
                        expression.range,
                        StrictOriginKind::UnsupportedConstruct,
                    );
                    result_indeterminate = true;
                    continue;
                }
                _ => {}
            }

            // A union argument combines member-wise. Its `NULL` members contribute nothing —
            // R drops `NULL` inside `c(...)`, so the idiomatic accumulator seeded with `c()`
            // (`acc <- c(); acc <- c(acc, x)` — type `T[] | NULL` at the loop join) is not an
            // error — and every remaining member must itself be combinable.
            let argument_atomics = match &resolved_argument {
                CoreType::Union(members) => members
                    .iter()
                    .filter(|member| !matches!(member, CoreType::Null))
                    .map(combine_operand_atomic)
                    .collect::<Option<Vec<Atomic>>>(),
                other => combine_operand_atomic(other).map(|atomic| vec![atomic]),
            };
            let Some(argument_atomics) = argument_atomics.filter(|atomics| !atomics.is_empty())
            else {
                return Err(InferenceError::TypeMismatch {
                    expected: Box::new(CoreType::Scalar(Atomic::Integer)),
                    actual: Box::new(resolved_argument.clone()),
                    range: Some(arg_expr.range),
                    expression_id: Some(arg_expr.id),
                });
            };

            for current_atomic in argument_atomics {
                item_atomic = Some(match item_atomic {
                    Some(previous_atomic) => {
                        promote_combine_atomic(previous_atomic, current_atomic).ok_or_else(
                            || InferenceError::TypeMismatch {
                                expected: Box::new(CoreType::Scalar(previous_atomic)),
                                actual: Box::new(resolved_argument.clone()),
                                range: Some(arg_expr.range),
                                expression_id: Some(arg_expr.id),
                            },
                        )?
                    }
                    None => current_atomic,
                });
            }
        }

        if !saw_non_null_argument {
            return Ok(CoreType::Null);
        }
        if result_indeterminate {
            return Ok(CoreType::Unknown);
        }
        let combined_atomic = item_atomic.unwrap_or(Atomic::Integer);
        if all_arguments_are_named {
            Ok(CoreType::named_vector(combined_atomic))
        } else {
            Ok(CoreType::vector(combined_atomic))
        }
    }

    // `switch(subject, a = ..., b = ..., default)` selects one branch by the subject's runtime
    // value. Selection cannot be modeled statically, but every branch IS checked — errors inside a
    // branch surface like anywhere else — and the call's type is the union of the branch values.
    // R returns invisible `NULL` when nothing matches, so `NULL` joins the union unless a default
    // (unnamed, non-first) branch exists. A named branch with no value falls through to the next
    // branch in R; it contributes no type of its own.
    pub(super) fn infer_builtin_switch(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        let Some((subject, branches)) = arguments.split_first() else {
            return Err(InferenceError::FunctionArityMismatch {
                expected: 1,
                actual: 0,
                range: Some(expression.range),
                expression_id: Some(expression.id),
            });
        };
        self.infer_expression_with_context(
            arena.get(subject.expression),
            arena,
            resolution_context,
            type_definitions,
        )?;

        let mut members = Vec::with_capacity(branches.len() + 1);
        let mut has_default = false;
        for branch in branches {
            if branch.name.is_none() {
                has_default = true;
            }
            let branch_type = self.infer_expression_with_context(
                arena.get(branch.expression),
                arena,
                resolution_context,
                type_definitions,
            )?;
            members.push(self.resolve(branch_type)?);
        }
        if !has_default {
            members.push(CoreType::Null);
        }
        Ok(CoreType::union_of(members))
    }

    pub(super) fn infer_builtin_list(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
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
                let inferred_type = self.infer_expression_with_context(
                    arena.get(argument.expression),
                    arena,
                    resolution_context,
                    type_definitions,
                )?;
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
                let inferred_type = self.infer_expression_with_context(
                    arena.get(argument.expression),
                    arena,
                    resolution_context,
                    type_definitions,
                )?;
                items.push(self.resolve(inferred_type)?);
            }
            Ok(CoreType::Tuple(items))
        }
    }
}
