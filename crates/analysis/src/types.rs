use {crate::interner::Symbol, tree_sitter::Range};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InferenceVariableId(pub u32);

// A type-class-style bound carried by an inference variable. `Numeric` restricts a variable to
// `integer` or `double` (any vector shape), so an unannotated parameter used arithmetically stays
// polymorphic over numeric types instead of being rejected. `AtomicElement` restricts a variable to
// a scalar atomic type — it is the bound a vector element carries (`T[]` requires `T` to be one of
// the six atomics). A variable that acquires both (a `T[]` element used arithmetically) holds their
// meet, `ScalarNumeric`: a scalar `integer` or `double`. Merge through `join`, not `Ord` — the
// lattice is not a chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum Constraint {
    #[default]
    Unconstrained,
    Numeric,
    AtomicElement,
    ScalarNumeric,
}

impl Constraint {
    pub fn join(self, other: Constraint) -> Constraint {
        use Constraint::*;
        match (self, other) {
            (Unconstrained, constraint) | (constraint, Unconstrained) => constraint,
            (left, right) if left == right => left,
            (Numeric, AtomicElement) | (AtomicElement, Numeric) => ScalarNumeric,
            (ScalarNumeric, _) | (_, ScalarNumeric) => ScalarNumeric,
            // Unreachable: every distinct pair is covered above, but the compiler cannot see it.
            (left, _) => left,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuantifiedVariable {
    pub variable: InferenceVariableId,
    pub constraint: Constraint,
}

impl QuantifiedVariable {
    pub fn new(variable: InferenceVariableId, constraint: Constraint) -> Self {
        Self {
            variable,
            constraint,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceType {
    Any,
    Unknown,
    Null,
    // Invariant: normalized — flattened (no `Union` member), structurally deduplicated, `Null`
    // last, no `Any`/`Unknown` member (they absorb the union), and at least two members. Build
    // through `SurfaceType::union_of`.
    Union(Vec<SurfaceType>),
    Scalar(Atomic),
    Named(Symbol, Vec<SurfaceType>),
    Vector(Box<SurfaceType>),
    NamedVector(Box<SurfaceType>),
    List(Box<SurfaceType>),
    NamedList(Box<SurfaceType>),
    Record(Vec<RecordField<SurfaceType>>),
    Tuple(Vec<SurfaceType>),
    Function(FunctionType<SurfaceType>),
    Binders(Vec<Symbol>, Box<SurfaceType>),
}

impl SurfaceType {
    pub fn union_of(members: Vec<SurfaceType>) -> SurfaceType {
        let Some(mut members) =
            normalize_union(members, SurfaceType::Null, |member| match member {
                SurfaceType::Union(inner) => Ok(inner),
                other => Err(other),
            })
        else {
            return SurfaceType::Null;
        };
        // `Any` and `Unknown` each already contain every value, so a union with such a member says
        // nothing more than the member itself. `Any` wins over `Unknown` because it is the wider
        // claim (explicitly opted out of checking).
        if members.contains(&SurfaceType::Any) {
            return SurfaceType::Any;
        }
        if members.contains(&SurfaceType::Unknown) {
            return SurfaceType::Unknown;
        }
        if members.len() == 1 {
            members.pop().expect("length was checked")
        } else {
            SurfaceType::Union(members)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreType {
    Any,
    Unknown,
    Null,
    // Invariant: normalized — flattened (no `Union` member), structurally deduplicated, `Null`
    // last, no `Any`/`Unknown` member (they absorb the union), and at least two members. Build
    // through `CoreType::union_of`. Member order otherwise preserves first occurrence; union
    // equality where it matters semantically (unification, compatibility) is set-based, so
    // `integer | NULL` and `NULL | integer` behave identically.
    Union(Vec<CoreType>),
    Scalar(Atomic),
    Nominal(Symbol, Vec<CoreType>),
    // The element is a full `CoreType` so a generic scheme can hold a type variable there
    // (`<T> fn(x: T[]) -> T[]`), but the only *valid* resolved elements are `Scalar(_)` (a concrete
    // atomic) and `Variable(_)` (an element still being inferred). Annotation lowering rejects
    // every other element shape, and element variables carry the atomic-element constraint so they
    // can never be bound to one.
    Vector(Box<CoreType>),
    NamedVector(Box<CoreType>),
    List(Box<CoreType>),
    NamedList(Box<CoreType>),
    Record(Vec<RecordField<CoreType>>),
    Tuple(Vec<CoreType>),
    Function(FunctionType<CoreType>),
    Variable(InferenceVariableId),
}

impl CoreType {
    pub fn vector(atomic: Atomic) -> CoreType {
        CoreType::Vector(Box::new(CoreType::Scalar(atomic)))
    }

    pub fn named_vector(atomic: Atomic) -> CoreType {
        CoreType::NamedVector(Box::new(CoreType::Scalar(atomic)))
    }

    // The concrete atomic of a vector element, when it has resolved to one (`None` for an element
    // still held by an inference variable). Callers that require a concrete element — the operator
    // kernel, `c(...)` promotion, subset results — go through this instead of matching the box.
    pub fn element_atomic(&self) -> Option<Atomic> {
        match self {
            CoreType::Scalar(atomic) => Some(*atomic),
            _ => None,
        }
    }

    pub fn union_of(members: Vec<CoreType>) -> CoreType {
        let Some(mut members) = normalize_union(members, CoreType::Null, |member| match member {
            CoreType::Union(inner) => Ok(inner),
            other => Err(other),
        }) else {
            return CoreType::Null;
        };
        // `Any` and `Unknown` each already contain every value, so a union with such a member says
        // nothing more than the member itself. `Any` wins over `Unknown` because it is the wider
        // claim (explicitly opted out of checking).
        if members.contains(&CoreType::Any) {
            return CoreType::Any;
        }
        if members.contains(&CoreType::Unknown) {
            return CoreType::Unknown;
        }
        if members.len() == 1 {
            members.pop().expect("length was checked")
        } else {
            CoreType::Union(members)
        }
    }
}

// Shared union normalization: flatten nested unions, deduplicate structurally (first occurrence
// wins), and move `Null` to the end. Returns `None` for an empty member list (the caller maps it
// to `Null`, the unit type) and the normalized members otherwise.
fn normalize_union<Type: PartialEq>(
    members: Vec<Type>,
    null: Type,
    into_union_members: impl Fn(Type) -> Result<Vec<Type>, Type> + Copy,
) -> Option<Vec<Type>> {
    let mut normalized: Vec<Type> = Vec::with_capacity(members.len());
    let mut saw_null = false;
    let mut queue: Vec<Type> = members.into_iter().rev().collect();
    while let Some(member) = queue.pop() {
        match into_union_members(member) {
            Ok(inner) => queue.extend(inner.into_iter().rev()),
            Err(member) => {
                if member == null {
                    saw_null = true;
                } else if !normalized.contains(&member) {
                    normalized.push(member);
                }
            }
        }
    }
    if saw_null {
        normalized.push(null);
    }
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeScheme {
    pub quantified_variables: Vec<QuantifiedVariable>,
    pub body: CoreType,
}

impl TypeScheme {
    pub fn monomorphic(body: CoreType) -> Self {
        Self {
            quantified_variables: Vec::new(),
            body,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Atomic {
    Logical,
    Integer,
    Double,
    Complex,
    Character,
    Raw,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordField<Type> {
    pub name: Symbol,
    pub value: Type,
    pub optional: bool,
}

impl<Type> RecordField<Type> {
    pub fn new(name: Symbol, value: Type) -> Self {
        Self {
            name,
            value,
            optional: false,
        }
    }

    pub fn optional(name: Symbol, value: Type) -> Self {
        Self {
            name,
            value,
            optional: true,
        }
    }

    pub fn with_optional(name: Symbol, value: Type, optional: bool) -> Self {
        Self {
            name,
            value,
            optional,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionType<Type> {
    pub parameters: Vec<Type>,
    pub named_parameters: Vec<RecordField<Type>>,
    // The element type of a trailing rest parameter (`...name: T`), if the function is variadic. A
    // variadic function accepts any number of additional positional arguments, each of this type. Making
    // this a single optional field makes "at most one rest parameter, and it is last" unrepresentable
    // otherwise.
    pub variadic: Option<Box<Type>>,
    pub return_type: Box<Type>,
}

impl<Type> FunctionType<Type> {
    pub fn new(
        parameters: Vec<Type>,
        named_parameters: Vec<RecordField<Type>>,
        return_type: Type,
    ) -> Self {
        Self {
            parameters,
            named_parameters,
            variadic: None,
            return_type: Box::new(return_type),
        }
    }

    pub fn with_variadic(
        parameters: Vec<Type>,
        named_parameters: Vec<RecordField<Type>>,
        variadic: Option<Type>,
        return_type: Type,
    ) -> Self {
        Self {
            parameters,
            named_parameters,
            variadic: variadic.map(Box::new),
            return_type: Box::new(return_type),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedTypeRef {
    pub name: Symbol,
    pub type_arguments: Vec<SurfaceType>,
}

impl NamedTypeRef {
    pub fn new(name: Symbol, type_arguments: Vec<SurfaceType>) -> Self {
        Self {
            name,
            type_arguments,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Annotation {
    Type {
        kind: TypeAnnotationKind,
        surface_type: SurfaceType,
    },
    New {
        nominal_type: NamedTypeRef,
    },
}

impl Annotation {
    pub fn checked(surface_type: SurfaceType) -> Self {
        Self::Type {
            kind: TypeAnnotationKind::Checked,
            surface_type,
        }
    }

    pub fn unknown_only(surface_type: SurfaceType) -> Self {
        Self::Type {
            kind: TypeAnnotationKind::UnknownOnly,
            surface_type,
        }
    }

    pub fn trusted(surface_type: SurfaceType) -> Self {
        Self::Type {
            kind: TypeAnnotationKind::Trusted,
            surface_type,
        }
    }

    pub fn new(nominal_type: NamedTypeRef) -> Self {
        Self::New { nominal_type }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeAnnotationKind {
    Checked,
    UnknownOnly,
    Trusted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachedAnnotation {
    Expression {
        annotation: Annotation,
        range: Range,
    },
    BindingAndExpression {
        annotation: Annotation,
        range: Range,
    },
}

impl AttachedAnnotation {
    pub fn expression(annotation: Annotation, range: Range) -> Self {
        Self::Expression { annotation, range }
    }

    pub fn binding_and_expression(annotation: Annotation, range: Range) -> Self {
        Self::BindingAndExpression { annotation, range }
    }

    pub fn annotation(&self) -> &Annotation {
        match self {
            Self::Expression { annotation, .. } | Self::BindingAndExpression { annotation, .. } => {
                annotation
            }
        }
    }

    pub fn applies_to_binding(&self) -> bool {
        matches!(self, Self::BindingAndExpression { .. })
    }

    pub fn range(&self) -> Range {
        match self {
            Self::Expression { range, .. } | Self::BindingAndExpression { range, .. } => *range,
        }
    }
}
