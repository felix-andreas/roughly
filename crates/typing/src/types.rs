use crate::interner::Symbol;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InferenceVariableId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceType {
    Any,
    Unknown,
    Null,
    Scalar(Atomic),
    Vector(Atomic),
    List(Box<SurfaceType>),
    Record(Vec<RecordField<SurfaceType>>),
    Tuple(Vec<SurfaceType>),
    Function(FunctionType<SurfaceType>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreType {
    Any,
    Unknown,
    Null,
    Scalar(Atomic),
    Vector(Atomic),
    List(Box<CoreType>),
    Record(Vec<RecordField<CoreType>>),
    Tuple(Vec<CoreType>),
    Function(FunctionType<CoreType>),
    Variable(InferenceVariableId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeScheme {
    pub quantified_variables: Vec<InferenceVariableId>,
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
}

impl<Type> RecordField<Type> {
    pub fn new(name: Symbol, value: Type) -> Self {
        Self { name, value }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionType<Type> {
    pub parameters: Vec<Type>,
    pub named_parameters: Vec<RecordField<Type>>,
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
            return_type: Box::new(return_type),
        }
    }
}
