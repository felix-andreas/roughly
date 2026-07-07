// Free-standing helpers behind the inference core: operand classification and numeric promotion
// for the builtin operators, comparison-family shapes, guard-refinement filtering, and the small
// pure `CoreType` transformations inference applies around unification and scheme import.
use {
    super::{GuardPredicate, InferenceError, ResolutionContext},
    crate::{
        hir::{Expression, ExpressionKind},
        interner::Symbol,
        types::{
            Atomic, Constraint, CoreType, FunctionType, InferenceVariableId, RecordField,
            RestParameter,
        },
    },
    std::collections::BTreeMap,
    tree_sitter::Range,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OperandShape {
    Scalar,
    Vector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NumericResultAtomic {
    Promote,
    AlwaysDouble,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ComparisonFamily {
    Numeric,
    Character,
    Logical,
}

// The per-pair result of an arithmetic operator over the member shapes of its two operands: a
// vector member makes the pair's result a vector, both-`integer` pairs stay `integer`, and any
// `double` (or an always-`double` operator like `/`) promotes the pair. The caller joins the pairs
// into the operation's result, so `(integer | double) + integer` is `integer | double`.
pub(super) fn member_wise_numeric_results(
    left_parts: &[(OperandShape, Atomic)],
    right_parts: &[(OperandShape, Atomic)],
    numeric_result_atomic: NumericResultAtomic,
) -> Vec<CoreType> {
    let mut results = Vec::with_capacity(left_parts.len() * right_parts.len());
    for (left_shape, left_atomic) in left_parts {
        for (right_shape, right_atomic) in right_parts {
            let shape =
                if *left_shape == OperandShape::Vector || *right_shape == OperandShape::Vector {
                    OperandShape::Vector
                } else {
                    OperandShape::Scalar
                };
            let atomic = match numeric_result_atomic {
                NumericResultAtomic::AlwaysDouble => Atomic::Double,
                NumericResultAtomic::Promote => {
                    if *left_atomic == Atomic::Integer && *right_atomic == Atomic::Integer {
                        Atomic::Integer
                    } else {
                        Atomic::Double
                    }
                }
            };
            results.push(core_type_for_shape(shape, atomic));
        }
    }
    results
}

// A comparison operand's member shapes: one for a concrete operand, all of them for a union
// (member-wise acceptance: every member must be comparable). `None` when any member is not.
pub(super) fn comparison_operand_parts_list(
    core_type: &CoreType,
) -> Option<Vec<(OperandShape, ComparisonFamily)>> {
    match core_type {
        CoreType::Union(members) => members.iter().map(comparison_operand_parts).collect(),
        other => comparison_operand_parts(other).map(|parts| vec![parts]),
    }
}

// The shapes a comparison operand can take; a still-flexible bare variable behaves as a scalar
// (matching the pre-union result rule), while a flexible-element vector is known to be a vector.
pub(super) fn shapes_for_operand(
    parts: &Option<Vec<(OperandShape, ComparisonFamily)>>,
    flexible: &Option<FlexibleComparisonOperand>,
) -> Vec<OperandShape> {
    match (parts, flexible) {
        (Some(parts), _) => parts.iter().map(|(shape, _)| *shape).collect(),
        (None, Some(FlexibleComparisonOperand::VectorElement(_))) => vec![OperandShape::Vector],
        _ => vec![OperandShape::Scalar],
    }
}

// A comparison operand whose family is not yet known: a bare inference variable, or a vector whose
// element is a still-generic variable (carried, so a numeric partner can constrain it) or is
// statically untracked (`Any`/`Unknown` element, nothing to constrain).
pub(super) enum FlexibleComparisonOperand {
    Bare(InferenceVariableId),
    VectorElement(Option<InferenceVariableId>),
}

impl FlexibleComparisonOperand {
    pub(super) fn variable(&self) -> Option<InferenceVariableId> {
        match self {
            FlexibleComparisonOperand::Bare(variable) => Some(*variable),
            FlexibleComparisonOperand::VectorElement(variable) => *variable,
        }
    }
}

pub(super) fn flexible_comparison_operand(
    core_type: &CoreType,
) -> Option<FlexibleComparisonOperand> {
    match core_type {
        CoreType::Variable(variable) => Some(FlexibleComparisonOperand::Bare(*variable)),
        CoreType::Vector(element) | CoreType::NamedVector(element) => match element.as_ref() {
            CoreType::Variable(variable) => {
                Some(FlexibleComparisonOperand::VectorElement(Some(*variable)))
            }
            CoreType::Any | CoreType::Unknown => {
                Some(FlexibleComparisonOperand::VectorElement(None))
            }
            _ => None,
        },
        _ => None,
    }
}

pub(super) fn comparison_operand_parts(
    core_type: &CoreType,
) -> Option<(OperandShape, ComparisonFamily)> {
    let (shape, atomic) = match core_type {
        CoreType::Scalar(atomic) => (OperandShape::Scalar, *atomic),
        CoreType::Vector(element) | CoreType::NamedVector(element) => {
            (OperandShape::Vector, element.element_atomic()?)
        }
        _ => return None,
    };

    let family = match atomic {
        Atomic::Integer | Atomic::Double => ComparisonFamily::Numeric,
        Atomic::Character => ComparisonFamily::Character,
        Atomic::Logical => ComparisonFamily::Logical,
        Atomic::Complex | Atomic::Raw => return None,
    };

    Some((shape, family))
}

// How an operand of an arithmetic operator classifies: a concrete numeric shape, a union whose
// members are all concrete numeric shapes (accepted member-wise), a still-flexible inference
// variable (which becomes numeric-constrained), an `Any`/`Unknown` short-circuit, or a hard error.
#[derive(Debug, Clone)]
pub(super) enum NumericOperand {
    Concrete(OperandShape, Atomic),
    // Every member of a union operand, in member order. The operation applies to each member and
    // the result is the join of the per-member results (see "Operators over union operands" in the
    // typing reference).
    ConcreteUnion(Vec<(OperandShape, Atomic)>),
    Variable(InferenceVariableId),
    // A vector whose element is not yet concrete: a generic element variable (`T[]`, carrying the
    // variable to constrain) or a statically untracked element (`Any`/`Unknown`, carrying `None`).
    // The shape is known — vector — even though the atomic is not.
    FlexibleVector(Option<InferenceVariableId>),
    AnyUnknown,
    Invalid,
}

impl NumericOperand {
    // The operand's member shapes: one for a concrete operand, all of them for a union.
    pub(super) fn concrete_parts(&self) -> Option<Vec<(OperandShape, Atomic)>> {
        match self {
            NumericOperand::Concrete(shape, atomic) => Some(vec![(*shape, *atomic)]),
            NumericOperand::ConcreteUnion(parts) => Some(parts.clone()),
            _ => None,
        }
    }
}

pub(super) fn classify_numeric_operand(core_type: &CoreType) -> NumericOperand {
    if let Some((shape, atomic)) = numeric_operand_parts(core_type) {
        return NumericOperand::Concrete(shape, atomic);
    }
    match core_type {
        // A union operand is numeric when every member is: `Any`/`Unknown`/nested unions cannot
        // appear as members (union normalization absorbs or flattens them) and inference variables
        // cannot either (a join binds a variable rather than uniting over it), so any non-numeric
        // member makes the whole operand invalid — the error then shows the full union type.
        CoreType::Union(members) => {
            let mut parts = Vec::with_capacity(members.len());
            for member in members {
                match numeric_operand_parts(member) {
                    Some(part) => parts.push(part),
                    None => return NumericOperand::Invalid,
                }
            }
            NumericOperand::ConcreteUnion(parts)
        }
        CoreType::Variable(variable) => NumericOperand::Variable(*variable),
        CoreType::Vector(element) | CoreType::NamedVector(element) => match element.as_ref() {
            CoreType::Variable(variable) => NumericOperand::FlexibleVector(Some(*variable)),
            CoreType::Any | CoreType::Unknown => NumericOperand::FlexibleVector(None),
            _ => NumericOperand::Invalid,
        },
        CoreType::Any | CoreType::Unknown => NumericOperand::AnyUnknown,
        _ => NumericOperand::Invalid,
    }
}

pub(super) fn constraint_is_satisfied(constraint: Constraint, core_type: &CoreType) -> bool {
    match constraint {
        Constraint::Unconstrained => true,
        Constraint::Numeric => is_numeric_core_type(core_type),
        Constraint::AtomicElement => matches!(core_type, CoreType::Scalar(_)),
        Constraint::ScalarNumeric => matches!(
            core_type,
            CoreType::Scalar(Atomic::Integer | Atomic::Double)
        ),
    }
}

pub(super) fn is_numeric_core_type(core_type: &CoreType) -> bool {
    match core_type {
        CoreType::Scalar(Atomic::Integer | Atomic::Double) => true,
        CoreType::Vector(element) | CoreType::NamedVector(element) => matches!(
            element.as_ref(),
            CoreType::Scalar(Atomic::Integer | Atomic::Double)
        ),
        _ => false,
    }
}

pub(super) fn constraint_violation_error(
    constraint: Constraint,
    actual: CoreType,
    expression: Option<&Expression>,
) -> InferenceError {
    InferenceError::ConstraintViolation {
        constraint,
        actual: Box::new(actual),
        range: expression.map(|expression| expression.range),
        expression_id: expression.map(|expression| expression.id),
    }
}

pub(super) fn numeric_operand_parts(core_type: &CoreType) -> Option<(OperandShape, Atomic)> {
    match core_type {
        CoreType::Scalar(atomic @ (Atomic::Integer | Atomic::Double)) => {
            Some((OperandShape::Scalar, *atomic))
        }
        CoreType::Vector(element) | CoreType::NamedVector(element) => {
            match element.element_atomic()? {
                atomic @ (Atomic::Integer | Atomic::Double) => Some((OperandShape::Vector, atomic)),
                _ => None,
            }
        }
        _ => None,
    }
}

pub(super) fn combine_operand_atomic(core_type: &CoreType) -> Option<Atomic> {
    match core_type {
        CoreType::Scalar(atomic) => Some(*atomic),
        CoreType::Vector(element) | CoreType::NamedVector(element) => element.element_atomic(),
        _ => None,
    }
}

// `c(...)` follows R's atomic coercion hierarchy: logical < integer < double < complex < character,
// so mixed arguments promote to the widest type (this is what lets `c(1L, NA)` be `integer` and
// `c(1L, "a")` be `character`). `raw` does not participate and only combines with itself.
pub(super) fn promote_combine_atomic(left: Atomic, right: Atomic) -> Option<Atomic> {
    if left == right {
        return Some(left);
    }
    let left_rank = combine_atomic_rank(left)?;
    let right_rank = combine_atomic_rank(right)?;
    Some(match left_rank.max(right_rank) {
        0 => Atomic::Logical,
        1 => Atomic::Integer,
        2 => Atomic::Double,
        3 => Atomic::Complex,
        _ => Atomic::Character,
    })
}

pub(super) fn combine_atomic_rank(atomic: Atomic) -> Option<u8> {
    match atomic {
        Atomic::Logical => Some(0),
        Atomic::Integer => Some(1),
        Atomic::Double => Some(2),
        Atomic::Complex => Some(3),
        Atomic::Character => Some(4),
        Atomic::Raw => None,
    }
}

pub(super) fn core_type_for_shape(shape: OperandShape, atomic: Atomic) -> CoreType {
    match shape {
        OperandShape::Scalar => CoreType::Scalar(atomic),
        OperandShape::Vector => CoreType::vector(atomic),
    }
}

// The (true-edge, false-edge) entry types a guard predicate induces over `entry`, `None` per edge
// when that edge leaves the entry untouched (and `None` overall when neither edge changes).
// Narrowing filters union members; a member whose family cannot be decided statically stays on
// both edges. The one non-union case: `is.null` on `Any`/`Unknown` pins the true edge to `NULL` —
// family guards deliberately do not invent a concrete shape for `Any`/`Unknown` (a refined union
// would false-positive against scalar-claim standard-library signatures).
pub(super) fn refine_guarded_type(
    entry: &CoreType,
    predicate: GuardPredicate,
) -> Option<(Option<CoreType>, Option<CoreType>)> {
    match entry {
        CoreType::Union(members) => {
            let mut true_members = Vec::with_capacity(members.len());
            let mut false_members = Vec::with_capacity(members.len());
            let mut decided_any = false;
            for member in members {
                match member_matches_guard(member, predicate) {
                    Some(true) => {
                        true_members.push(member.clone());
                        decided_any = true;
                    }
                    Some(false) => {
                        false_members.push(member.clone());
                        decided_any = true;
                    }
                    None => {
                        true_members.push(member.clone());
                        false_members.push(member.clone());
                    }
                }
            }
            if !decided_any {
                return None;
            }
            // An edge with no members is impossible at runtime; dead branches are not typed
            // specially, so such an edge stays untouched. An edge keeping every member changes
            // nothing and stays untouched too.
            let true_type = (!true_members.is_empty() && true_members.len() < members.len())
                .then(|| CoreType::union_of(true_members));
            let false_type = (!false_members.is_empty() && false_members.len() < members.len())
                .then(|| CoreType::union_of(false_members));
            if true_type.is_none() && false_type.is_none() {
                return None;
            }
            Some((true_type, false_type))
        }
        CoreType::Any | CoreType::Unknown if predicate == GuardPredicate::Null => {
            Some((Some(CoreType::Null), None))
        }
        _ => None,
    }
}

// Whether one (normalized, non-union) member satisfies a guard predicate: `Some(bool)` when the
// member's runtime family is statically certain, `None` when it is not (inference variables,
// flexible-element vectors, opaque nominals — `is.list(data.frame)` is true at runtime, for
// example, which the checker cannot see through a sealed type).
pub(super) fn member_matches_guard(member: &CoreType, predicate: GuardPredicate) -> Option<bool> {
    match member {
        // A nested union inside a normalized union cannot occur; treat it as undecidable if a
        // denormalized value ever reaches here rather than mis-filtering it.
        CoreType::Variable(_)
        | CoreType::Any
        | CoreType::Unknown
        | CoreType::Nominal(..)
        | CoreType::Union(_) => None,
        CoreType::Null => Some(predicate == GuardPredicate::Null),
        CoreType::Scalar(atomic) => Some(atomic_matches_guard(*atomic, predicate)),
        CoreType::Vector(element) | CoreType::NamedVector(element) => match element.as_ref() {
            CoreType::Scalar(atomic) => Some(atomic_matches_guard(*atomic, predicate)),
            _ => None,
        },
        CoreType::Function(_) => Some(predicate == GuardPredicate::Function),
        CoreType::List(_) | CoreType::NamedList(_) | CoreType::Tuple(_) | CoreType::Record(_) => {
            Some(predicate == GuardPredicate::List)
        }
    }
}

pub(super) fn atomic_matches_guard(atomic: Atomic, predicate: GuardPredicate) -> bool {
    match predicate {
        GuardPredicate::Character => atomic == Atomic::Character,
        GuardPredicate::Logical => atomic == Atomic::Logical,
        GuardPredicate::Integer => atomic == Atomic::Integer,
        GuardPredicate::Double => atomic == Atomic::Double,
        GuardPredicate::Numeric => matches!(atomic, Atomic::Integer | Atomic::Double),
        GuardPredicate::Null | GuardPredicate::Function | GuardPredicate::List => false,
    }
}

pub(super) fn nullable_type(core_type: CoreType) -> CoreType {
    CoreType::union_of(vec![core_type, CoreType::Null])
}

// The one atomic widening compatibility admits: `integer` fits where `double` is expected. All
// other atomic pairs must match exactly.
pub(super) fn atomic_widens_to(actual: Atomic, expected: Atomic) -> bool {
    actual == expected || (actual == Atomic::Integer && expected == Atomic::Double)
}

// Replaces every inference variable in an (already-resolved) type with `Unknown`, for values that
// must survive a later unification rollback: a stored variable id would dangle once the rollback
// reclaims (and later reuses) the id.
pub(super) fn erase_variables(core_type: CoreType) -> CoreType {
    match core_type {
        CoreType::Variable(_) => CoreType::Unknown,
        CoreType::Union(members) => {
            CoreType::union_of(members.into_iter().map(erase_variables).collect())
        }
        CoreType::List(inner) => CoreType::List(Box::new(erase_variables(*inner))),
        CoreType::NamedList(inner) => CoreType::NamedList(Box::new(erase_variables(*inner))),
        CoreType::Record(fields) => CoreType::Record(
            fields
                .into_iter()
                .map(|field| {
                    RecordField::with_optional(
                        field.name,
                        erase_variables(field.value),
                        field.optional,
                    )
                })
                .collect(),
        ),
        CoreType::Tuple(items) => CoreType::Tuple(items.into_iter().map(erase_variables).collect()),
        CoreType::Nominal(name, arguments) => {
            CoreType::Nominal(name, arguments.into_iter().map(erase_variables).collect())
        }
        CoreType::Function(function_type) => CoreType::Function(FunctionType {
            parameters: function_type
                .parameters
                .into_iter()
                .map(erase_variables)
                .collect(),
            named_parameters: function_type
                .named_parameters
                .into_iter()
                .map(|parameter| {
                    RecordField::with_optional(
                        parameter.name,
                        erase_variables(parameter.value),
                        parameter.optional,
                    )
                })
                .collect(),
            variadic: function_type.variadic.map(|variadic| RestParameter {
                element: Box::new(erase_variables(*variadic.element)),
                preceding_named: variadic.preceding_named,
            }),
            return_type: Box::new(erase_variables(*function_type.return_type)),
        }),
        CoreType::Vector(element) => CoreType::Vector(Box::new(erase_variables(*element))),
        CoreType::NamedVector(element) => {
            CoreType::NamedVector(Box::new(erase_variables(*element)))
        }
        other @ (CoreType::Any | CoreType::Unknown | CoreType::Null | CoreType::Scalar(_)) => other,
    }
}

// Rewrites an indexing error raised against one union member so it reports the full union — the
// subject's actual type. Only the container/actual payload changes; range and identity stay.
pub(super) fn widen_error_container_to_union(
    error: InferenceError,
    union_type: &CoreType,
) -> InferenceError {
    match error {
        InferenceError::FieldDoesNotExist {
            field,
            range,
            expression_id,
            ..
        } => InferenceError::FieldDoesNotExist {
            field,
            container: Box::new(union_type.clone()),
            range,
            expression_id,
        },
        InferenceError::PositionDoesNotExist {
            position,
            range,
            expression_id,
            ..
        } => InferenceError::PositionDoesNotExist {
            position,
            container: Box::new(union_type.clone()),
            range,
            expression_id,
        },
        InferenceError::NotAList {
            range,
            expression_id,
            ..
        } => InferenceError::NotAList {
            actual: Box::new(union_type.clone()),
            range,
            expression_id,
        },
        InferenceError::NotIterable {
            range,
            expression_id,
            ..
        } => InferenceError::NotIterable {
            actual: Box::new(union_type.clone()),
            range,
            expression_id,
        },
        InferenceError::DollarOnAtomicVector {
            range,
            expression_id,
            ..
        } => InferenceError::DollarOnAtomicVector {
            actual: Box::new(union_type.clone()),
            range,
            expression_id,
        },
        InferenceError::UnsupportedSubset {
            range,
            expression_id,
            ..
        } => InferenceError::UnsupportedSubset {
            actual: Box::new(union_type.clone()),
            range,
            expression_id,
        },
        other => other,
    }
}

// The one member-wise union shape `unify` handles: a normalized two-member `T | NULL` union
// exposes its non-`NULL` member. Everything else returns `None`.
pub(super) fn nullable_single_member(members: &[CoreType]) -> Option<CoreType> {
    match members {
        [member, CoreType::Null] if *member != CoreType::Null => Some(member.clone()),
        _ => None,
    }
}

pub(super) fn iterable_item_type(core_type: &CoreType) -> Option<CoreType> {
    match core_type {
        CoreType::Scalar(atomic) => Some(CoreType::Scalar(*atomic)),
        CoreType::Vector(element) | CoreType::NamedVector(element) => Some((**element).clone()),
        CoreType::List(item_type) | CoreType::NamedList(item_type) => Some((**item_type).clone()),
        // A fixed-shape list iterates every element, so the loop variable can hold any of the item
        // types: the element type is their union (which collapses back to the single item type for
        // a homogeneous list; the empty list's element type is `NULL`, the union of zero members).
        CoreType::Tuple(items) => Some(CoreType::union_of(items.clone())),
        CoreType::Record(fields) => Some(CoreType::union_of(
            fields.iter().map(|field| field.value.clone()).collect(),
        )),
        // Member-wise over a union iterable: every member must itself be iterable, and the loop
        // variable can hold any member's element type.
        CoreType::Union(members) => {
            let mut item_types = Vec::with_capacity(members.len());
            for member in members {
                item_types.push(iterable_item_type(member)?);
            }
            Some(CoreType::union_of(item_types))
        }
        // `for (x in NULL)` runs zero iterations and is legal R; the loop variable holds the
        // union of no members.
        CoreType::Null => Some(CoreType::Null),
        // Gradual types iterate as themselves: `Any` sequences yield `Any` items, and an
        // `Unknown` sequence must not spray a second error on the loop.
        CoreType::Any => Some(CoreType::Any),
        CoreType::Unknown => Some(CoreType::Unknown),
        // Opaque nominals (`data.frame` above all) are containers whose element shape the
        // checker cannot see; iterating them is idiomatic R, so the item degrades to `Any`
        // rather than erroring.
        CoreType::Nominal(..) => Some(CoreType::Any),
        _ => None,
    }
}

pub(super) fn import_core_type(
    core_type: &CoreType,
    substitutions: &BTreeMap<InferenceVariableId, InferenceVariableId>,
) -> CoreType {
    match core_type {
        CoreType::Any => CoreType::Any,
        CoreType::Unknown => CoreType::Unknown,
        CoreType::Null => CoreType::Null,
        // Re-normalize: importing can collapse members (a free variable erases to `Unknown`, which
        // absorbs the union) or make two members equal.
        CoreType::Union(members) => CoreType::union_of(
            members
                .iter()
                .map(|member| import_core_type(member, substitutions))
                .collect(),
        ),
        CoreType::Scalar(atomic) => CoreType::Scalar(*atomic),
        CoreType::Nominal(symbol, type_arguments) => CoreType::Nominal(
            *symbol,
            type_arguments
                .iter()
                .map(|type_argument| import_core_type(type_argument, substitutions))
                .collect(),
        ),
        CoreType::Vector(element) => {
            CoreType::Vector(Box::new(import_core_type(element, substitutions)))
        }
        CoreType::NamedVector(element) => {
            CoreType::NamedVector(Box::new(import_core_type(element, substitutions)))
        }
        CoreType::List(item_type) => {
            CoreType::List(Box::new(import_core_type(item_type, substitutions)))
        }
        CoreType::NamedList(item_type) => {
            CoreType::NamedList(Box::new(import_core_type(item_type, substitutions)))
        }
        CoreType::Record(fields) => CoreType::Record(
            fields
                .iter()
                .map(|field| {
                    RecordField::with_optional(
                        field.name,
                        import_core_type(&field.value, substitutions),
                        field.optional,
                    )
                })
                .collect(),
        ),
        CoreType::Tuple(items) => CoreType::Tuple(
            items
                .iter()
                .map(|item| import_core_type(item, substitutions))
                .collect(),
        ),
        CoreType::Function(function_type) => CoreType::Function(FunctionType::with_variadic(
            function_type
                .parameters
                .iter()
                .map(|parameter| import_core_type(parameter, substitutions))
                .collect(),
            function_type
                .named_parameters
                .iter()
                .map(|parameter| {
                    RecordField::with_optional(
                        parameter.name,
                        import_core_type(&parameter.value, substitutions),
                        parameter.optional,
                    )
                })
                .collect(),
            function_type
                .variadic
                .as_ref()
                .map(|variadic| RestParameter {
                    element: Box::new(import_core_type(&variadic.element, substitutions)),
                    preceding_named: variadic.preceding_named,
                }),
            import_core_type(&function_type.return_type, substitutions),
        )),
        // A variable absent from `substitutions` is a free scheme variable that belongs to another
        // document's `InferenceState`; it has no meaning here, so importing erases it to `Unknown`.
        // This is a deliberate erasure of a map lookup miss, not a discarded error.
        CoreType::Variable(variable) => substitutions
            .get(variable)
            .copied()
            .map(CoreType::Variable)
            .unwrap_or(CoreType::Unknown),
    }
}

pub(super) fn is_whole_number_double_literal(expression: &Expression) -> bool {
    let ExpressionKind::Double(text) = &expression.kind else {
        return false;
    };
    text.parse::<f64>()
        .is_ok_and(|value| value.fract() == 0.0 && value.is_finite())
}

pub(super) fn integer_literal_position(expression: &Expression) -> Option<usize> {
    // R indexes identically with `x[[2]]` and `x[[2L]]`, so a whole-number double literal is just as
    // valid a statically known position as an integer literal.
    let one_based_index = match &expression.kind {
        ExpressionKind::Integer(text) => text.trim_end_matches('L').parse::<usize>().ok()?,
        ExpressionKind::Double(text) => {
            let value = text.parse::<f64>().ok()?;
            if value.fract() != 0.0 || value < 1.0 || !value.is_finite() {
                return None;
            }
            value as usize
        }
        _ => return None,
    };
    one_based_index.checked_sub(1)
}

pub(super) fn literal_name_symbol(expression: &Expression) -> Option<Symbol> {
    let ExpressionKind::StringLiteralName(symbol) = &expression.kind else {
        return None;
    };
    Some(*symbol)
}

pub(super) fn alias_cycle_error(symbol: Symbol, expression: Option<&Expression>) -> InferenceError {
    let fallback_range = Range {
        start_byte: 0,
        end_byte: 0,
        start_point: tree_sitter::Point::new(0, 0),
        end_point: tree_sitter::Point::new(0, 0),
    };
    InferenceError::AliasCycle {
        symbol,
        range: expression
            .map(|expression| expression.range)
            .unwrap_or(fallback_range),
        expression_id: expression.map(|expression| expression.id),
    }
}

// Aligns an expected (annotated) function type's parameter types to the definition's formals.
// R call sites match arguments against the *definition's* formal names, so a named annotation
// parameter must bind to the same-named formal — a positional zip would type the body's formals
// against the wrong slots whenever the annotation and definition order them differently, and a
// by-name call would then route a value into a formal checked at a different type.
//
// Named annotation parameters bind by name; unnamed (positional) annotation parameters fill the
// remaining formals left to right. `Ok(None)` means the shapes cannot align for a reason ordinary
// function compatibility will diagnose (an arity mismatch); a named parameter that matches no
// formal is its own hard error, because the annotation would otherwise promise callers a name the
// runtime rejects.
// The overloadable name a call's callee spells, when it is a bare or namespace-qualified name
// that does NOT resolve to a local slot or package global (an overload set never shadows user
// code — only base-environment stub names participate).
pub(super) fn callee_overload_symbol(
    callee: &Expression,
    resolution_context: Option<&ResolutionContext<'_>>,
) -> Option<Symbol> {
    match &callee.kind {
        ExpressionKind::Symbol(symbol) => {
            if let Some(context) = resolution_context {
                if context
                    .local_naming
                    .expression_resolutions
                    .contains_key(&callee.id)
                {
                    return None;
                }
                if context.package_naming.global_bindings.contains_key(symbol) {
                    return None;
                }
            }
            Some(*symbol)
        }
        ExpressionKind::NamespaceGet { name, .. } => Some(*name),
        _ => None,
    }
}

pub(super) fn align_expected_parameter_types(
    function_type: &FunctionType<CoreType>,
    parameters: &[crate::hir::Parameter],
    annotation_range: Range,
) -> Result<Option<Vec<CoreType>>, InferenceError> {
    if function_type.parameters.len() + function_type.named_parameters.len() != parameters.len() {
        return Ok(None);
    }
    let mut aligned: Vec<Option<CoreType>> = vec![None; parameters.len()];
    for named_parameter in &function_type.named_parameters {
        let Some(index) = parameters
            .iter()
            .position(|parameter| parameter.symbol == named_parameter.name)
        else {
            return Err(InferenceError::AnnotationParameterNameMismatch {
                name: named_parameter.name,
                range: Some(annotation_range),
            });
        };
        if aligned[index].is_some() {
            return Ok(None);
        }
        aligned[index] = Some(named_parameter.value.clone());
    }
    let mut positional = function_type.parameters.iter();
    for slot in aligned.iter_mut() {
        if slot.is_none() {
            *slot = positional.next().cloned();
        }
    }
    Ok(aligned.into_iter().collect())
}
