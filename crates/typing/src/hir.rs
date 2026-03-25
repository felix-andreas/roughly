use {
    crate::{
        interner::Symbol,
        types::{AttachedAnnotation, SurfaceType},
    },
    tree_sitter::Range,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExpressionId(pub u32);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HirArena {
    pub expressions: Vec<Expression>,
}

impl HirArena {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn alloc(&mut self, mut expression: Expression) -> ExpressionId {
        let id = ExpressionId(self.expressions.len() as u32);
        expression.id = id;
        self.expressions.push(expression);
        id
    }

    pub fn get(&self, id: ExpressionId) -> &Expression {
        &self.expressions[id.0 as usize]
    }

    pub fn get_mut(&mut self, id: ExpressionId) -> &mut Expression {
        &mut self.expressions[id.0 as usize]
    }

    pub fn expressions(&self) -> &[Expression] {
        &self.expressions
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    pub arena: HirArena,
    pub root_expressions: Vec<ExpressionId>,
}

impl Module {
    pub fn new(arena: HirArena, root_expressions: Vec<ExpressionId>) -> Self {
        Self {
            arena,
            root_expressions,
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
        expressions: Vec<ExpressionId>,
        has_trailing_semicolon: bool,
    },
    Assign {
        target: Symbol,
        annotation: Option<AttachedAnnotation>,
        value: ExpressionId,
    },
    Function {
        parameters: Vec<Parameter>,
        body: ExpressionId,
    },
    If {
        condition: ExpressionId,
        consequence: ExpressionId,
        alternative: Option<ExpressionId>,
    },
    For {
        variable: Symbol,
        sequence: ExpressionId,
        body: ExpressionId,
    },
    While {
        condition: ExpressionId,
        body: ExpressionId,
    },
    Repeat {
        body: ExpressionId,
    },
    UnaryMinus {
        value: ExpressionId,
    },
    Call {
        callee: ExpressionId,
        arguments: Vec<Argument>,
    },
    Subset {
        value: ExpressionId,
        arguments: Vec<Argument>,
    },
    Subset2 {
        value: ExpressionId,
        arguments: Vec<Argument>,
    },
    Dollar {
        value: ExpressionId,
        name: Symbol,
    },
    Definition(Definition),
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    pub kind: DefinitionKind,
    pub name: Symbol,
    pub type_parameters: Vec<Symbol>,
    pub surface_type: SurfaceType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefinitionKind {
    Type,
    Alias,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    pub symbol: Symbol,
    pub range: Range,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Argument {
    pub expression: ExpressionId,
    pub name: Option<Symbol>,
}

impl Module {
    pub fn render(&self, interner: &crate::interner::Interner) -> String {
        let mut out = String::new();
        for expr_id in &self.root_expressions {
            self.render_expression(*expr_id, 0, &mut out, interner);
        }
        out
    }

    fn render_expression(
        &self,
        id: ExpressionId,
        indent: usize,
        out: &mut String,
        interner: &crate::interner::Interner,
    ) {
        let expr = self.arena.get(id);
        let prefix = "  ".repeat(indent);

        if let Some(annotation) = &expr.annotation {
            use crate::annotations::render_surface_type;
            match &annotation.annotation {
                crate::types::Annotation::Type { kind, surface_type } => {
                    let prefix_kind = match kind {
                        crate::types::TypeAnnotationKind::Checked => "",
                        crate::types::TypeAnnotationKind::UnknownOnly => "@if-unknown ",
                        crate::types::TypeAnnotationKind::Trusted => "@trust ",
                    };
                    out.push_str(&format!(
                        "{prefix}#: {prefix_kind}{}\n",
                        render_surface_type(surface_type, interner)
                    ));
                }
                crate::types::Annotation::New { name } => {
                    let rendered_name = interner.resolve(*name).unwrap_or("<unknown>");
                    out.push_str(&format!("{prefix}#: @new {rendered_name}\n"));
                }
            }
        }

        match &expr.kind {
            ExpressionKind::Null => out.push_str(&format!("{prefix}Null\n")),
            ExpressionKind::Logical(b) => out.push_str(&format!("{prefix}Logical({b})\n")),
            ExpressionKind::Integer(s) => out.push_str(&format!("{prefix}Integer({s:?})\n")),
            ExpressionKind::Double(s) => out.push_str(&format!("{prefix}Double({s:?})\n")),
            ExpressionKind::Character(s) => out.push_str(&format!("{prefix}Character({s:?})\n")),
            ExpressionKind::StringLiteralName(sym) => {
                let name = interner.resolve(*sym).unwrap_or("<unknown>");
                out.push_str(&format!("{prefix}StringLiteralName({name:?})\n"))
            }
            ExpressionKind::Symbol(sym) => {
                let name = interner.resolve(*sym).unwrap_or("<unknown>");
                out.push_str(&format!("{prefix}Symbol({name})\n"))
            }
            ExpressionKind::Block {
                expressions,
                has_trailing_semicolon,
            } => {
                out.push_str(&format!(
                    "{prefix}Block (trailing_semi: {has_trailing_semicolon})\n"
                ));
                for e in expressions {
                    self.render_expression(*e, indent + 1, out, interner);
                }
            }
            ExpressionKind::Assign {
                target,
                value,
                annotation: _,
            } => {
                let name = interner.resolve(*target).unwrap_or("<unknown>");
                out.push_str(&format!("{prefix}Assign {name}\n"));
                self.render_expression(*value, indent + 1, out, interner);
            }
            ExpressionKind::Function { parameters, body } => {
                let params = parameters
                    .iter()
                    .map(|p| interner.resolve(p.symbol).unwrap_or("<unknown>"))
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push_str(&format!("{prefix}Function ({params})\n"));
                self.render_expression(*body, indent + 1, out, interner);
            }
            ExpressionKind::If {
                condition,
                consequence,
                alternative,
            } => {
                out.push_str(&format!("{prefix}If\n"));
                self.render_expression(*condition, indent + 1, out, interner);
                self.render_expression(*consequence, indent + 1, out, interner);
                if let Some(alt) = alternative {
                    self.render_expression(*alt, indent + 1, out, interner);
                }
            }
            ExpressionKind::For {
                variable,
                sequence,
                body,
            } => {
                let var_name = interner.resolve(*variable).unwrap_or("<unknown>");
                out.push_str(&format!("{prefix}For {var_name}\n"));
                self.render_expression(*sequence, indent + 1, out, interner);
                self.render_expression(*body, indent + 1, out, interner);
            }
            ExpressionKind::While { condition, body } => {
                out.push_str(&format!("{prefix}While\n"));
                self.render_expression(*condition, indent + 1, out, interner);
                self.render_expression(*body, indent + 1, out, interner);
            }
            ExpressionKind::Repeat { body } => {
                out.push_str(&format!("{prefix}Repeat\n"));
                self.render_expression(*body, indent + 1, out, interner);
            }
            ExpressionKind::UnaryMinus { value } => {
                out.push_str(&format!("{prefix}UnaryMinus\n"));
                self.render_expression(*value, indent + 1, out, interner);
            }
            ExpressionKind::Call { callee, arguments } => {
                out.push_str(&format!("{prefix}Call\n"));
                self.render_expression(*callee, indent + 1, out, interner);
                for arg in arguments {
                    let arg_prefix = "  ".repeat(indent + 1);
                    if let Some(name) = arg.name {
                        let name_str = interner.resolve(name).unwrap_or("<unknown>");
                        out.push_str(&format!("{arg_prefix}Argument {name_str}:\n"));
                        self.render_expression(arg.expression, indent + 2, out, interner);
                    } else {
                        out.push_str(&format!("{arg_prefix}Argument:\n"));
                        self.render_expression(arg.expression, indent + 2, out, interner);
                    }
                }
            }
            ExpressionKind::Subset { value, arguments } => {
                out.push_str(&format!("{prefix}Subset\n"));
                self.render_expression(*value, indent + 1, out, interner);
                for arg in arguments {
                    let arg_prefix = "  ".repeat(indent + 1);
                    if let Some(name) = arg.name {
                        let name_str = interner.resolve(name).unwrap_or("<unknown>");
                        out.push_str(&format!("{arg_prefix}Argument {name_str}:\n"));
                        self.render_expression(arg.expression, indent + 2, out, interner);
                    } else {
                        out.push_str(&format!("{arg_prefix}Argument:\n"));
                        self.render_expression(arg.expression, indent + 2, out, interner);
                    }
                }
            }
            ExpressionKind::Subset2 { value, arguments } => {
                out.push_str(&format!("{prefix}Subset2\n"));
                self.render_expression(*value, indent + 1, out, interner);
                for arg in arguments {
                    let arg_prefix = "  ".repeat(indent + 1);
                    if let Some(name) = arg.name {
                        let name_str = interner.resolve(name).unwrap_or("<unknown>");
                        out.push_str(&format!("{arg_prefix}Argument {name_str}:\n"));
                        self.render_expression(arg.expression, indent + 2, out, interner);
                    } else {
                        out.push_str(&format!("{arg_prefix}Argument:\n"));
                        self.render_expression(arg.expression, indent + 2, out, interner);
                    }
                }
            }
            ExpressionKind::Dollar { value, name } => {
                let name_str = interner.resolve(*name).unwrap_or("<unknown>");
                out.push_str(&format!("{prefix}Dollar {name_str}\n"));
                self.render_expression(*value, indent + 1, out, interner);
            }
            ExpressionKind::Definition(definition) => {
                use crate::annotations::render_surface_type;
                let label = match definition.kind {
                    DefinitionKind::Type => "TypeDefinition",
                    DefinitionKind::Alias => "TypeAlias",
                };
                let name_str = interner.resolve(definition.name).unwrap_or("<unknown>");
                let params = if definition.type_parameters.is_empty() {
                    "".to_owned()
                } else {
                    let rendered_params = definition
                        .type_parameters
                        .iter()
                        .map(|&p| interner.resolve(p).unwrap_or("<unknown>").to_owned())
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("<{rendered_params}>")
                };
                let ty_str = render_surface_type(&definition.surface_type, interner);
                out.push_str(&format!("{prefix}{label} {name_str}{params} = {ty_str}\n"));
            }
            ExpressionKind::Unsupported => out.push_str(&format!("{prefix}Unsupported\n")),
        }
    }
}
