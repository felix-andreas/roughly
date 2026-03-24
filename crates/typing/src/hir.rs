use {
    crate::{
        interner::Symbol,
        types::{Annotation, AttachedAnnotation},
    },
    tree_sitter::Range,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExpressionId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    pub expressions: Vec<Expression>,
    pub annotations: Vec<PendingAnnotation>,
}

impl Module {
    pub fn new(expressions: Vec<Expression>, annotations: Vec<PendingAnnotation>) -> Self {
        Self {
            expressions,
            annotations,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expression {
    pub id: ExpressionId,
    pub range: Range,
    pub annotation: Option<AttachedAnnotation>,
    pub kind: ExpressionKind,
}

impl Expression {
    pub fn new(
        id: ExpressionId,
        range: Range,
        annotation: Option<AttachedAnnotation>,
        kind: ExpressionKind,
    ) -> Self {
        Self {
            id,
            range,
            annotation,
            kind,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpressionKind {
    Null,
    Logical(bool),
    Integer(String),
    Double(String),
    Character(String),
    StringLiteralName(Symbol),
    Symbol(Symbol),
    Block {
        expressions: Vec<Expression>,
        has_trailing_semicolon: bool,
    },
    Assign {
        target: Symbol,
        annotation: Option<AttachedAnnotation>,
        value: Box<Expression>,
    },
    Function {
        parameters: Vec<Parameter>,
        body: Box<Expression>,
    },
    If {
        condition: Box<Expression>,
        consequence: Box<Expression>,
        alternative: Option<Box<Expression>>,
    },
    For {
        variable: Symbol,
        sequence: Box<Expression>,
        body: Box<Expression>,
    },
    While {
        condition: Box<Expression>,
        body: Box<Expression>,
    },
    Repeat {
        body: Box<Expression>,
    },
    UnaryMinus {
        value: Box<Expression>,
    },
    Call {
        callee: Box<Expression>,
        arguments: Vec<Argument>,
    },
    Subset {
        value: Box<Expression>,
        arguments: Vec<Argument>,
    },
    Subset2 {
        value: Box<Expression>,
        arguments: Vec<Argument>,
    },
    Dollar {
        value: Box<Expression>,
        name: Symbol,
    },
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    pub symbol: Symbol,
    pub range: Range,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Argument {
    pub expression: Expression,
    pub name: Option<Symbol>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAnnotation {
    pub range: Range,
    pub annotation: Annotation,
}
