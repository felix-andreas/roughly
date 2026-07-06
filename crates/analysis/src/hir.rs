use {
    crate::{
        document::DocumentId,
        interner::Symbol,
        type_syntax::{render_named_type_ref, render_surface_type},
        types::{Atomic, AttachedAnnotation, SurfaceType},
    },
    std::collections::BTreeMap,
    tree_sitter::Range,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExpressionId(pub u32);

pub type ModuleId = DocumentId;

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

    // IDE paths may hold an ExpressionId that no longer exists after a re-lowering; they must
    // degrade to `None` rather than panic. Internal phase code uses the infallible `get`.
    pub fn try_get(&self, id: ExpressionId) -> Option<&Expression> {
        self.expressions.get(id.0 as usize)
    }

    pub fn expressions(&self) -> &[Expression] {
        &self.expressions
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    pub arena: HirArena,
    pub definitions: Vec<DefinitionItem>,
    pub expressions: Vec<ExpressionId>,
    // A top-level `#: @strict` / `#: @strict off` directive overriding the configured strict
    // switch for this file (last directive wins). `None` when the file has no directive.
    pub strict_override: Option<bool>,
    // Derived from `arena` once at construction so IDE range lookups are O(log n) instead of a
    // linear arena scan. Keyed by (start_byte, end_byte).
    span_index: BTreeMap<(usize, usize), ExpressionId>,
}

impl Module {
    pub fn new(
        arena: HirArena,
        definitions: Vec<DefinitionItem>,
        expressions: Vec<ExpressionId>,
    ) -> Self {
        Self::with_strict_override(arena, definitions, expressions, None)
    }

    pub fn with_strict_override(
        arena: HirArena,
        definitions: Vec<DefinitionItem>,
        expressions: Vec<ExpressionId>,
        strict_override: Option<bool>,
    ) -> Self {
        let mut span_index = BTreeMap::new();
        // Expressions are allocated in ascending ExpressionId order, so `or_insert` keeps the
        // smallest id when several expressions share a range (e.g. a parent that wraps a single
        // child). This matches the previous first-match scan over the arena.
        for expression in arena.expressions() {
            span_index
                .entry((expression.range.start_byte, expression.range.end_byte))
                .or_insert(expression.id);
        }
        Self {
            arena,
            definitions,
            expressions,
            strict_override,
            span_index,
        }
    }

    pub fn expression_id_by_range(&self, range: Range) -> Option<ExpressionId> {
        self.span_index
            .get(&(range.start_byte, range.end_byte))
            .copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DefinitionId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionItem {
    pub id: DefinitionId,
    pub range: Range,
    pub definition: Definition,
}

impl DefinitionItem {
    pub fn new(id: DefinitionId, range: Range, definition: Definition) -> Self {
        Self {
            id,
            range,
            definition,
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
    // A reserved constant whose atomic type is fixed, such as `NA` (logical), the typed `NA_*`
    // forms, or an imaginary literal like `1i` (complex).
    AtomicConstant(Atomic),
    StringLiteralName(Symbol),
    Symbol(Symbol),
    Block {
        expressions: Vec<ExpressionId>,
        has_trailing_semicolon: bool,
    },
    Assign {
        target: AssignTarget,
        scope: AssignmentScope,
        value: ExpressionId,
    },
    Function {
        parameters: Vec<Parameter>,
        body: ExpressionId,
    },
    // R's `local(expr)`: evaluates `expr` in a fresh child environment and returns its value. It is a
    // scope-introducing form (an assignment inside does not leak to the enclosing scope), but unlike a
    // function it takes no parameters and its type is the body's value, so it is its own node rather than
    // an immediately-invoked `Function`.
    Local {
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
    UnaryNot {
        value: ExpressionId,
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
    // `x@slot`: an S4 slot read. The subject is fully lowered so its variable read participates in
    // naming and every IDE feature; the checker does not model S4 objects, so the read itself
    // types as `Unknown` (a strict origin).
    Slot {
        value: ExpressionId,
        name: Symbol,
    },
    Break,
    Next,
    // `pkg::name` / `pkg:::name`: a direct read of one name from a package namespace, bypassing
    // lexical scoping. Both operators lower identically — the checker does not model the
    // exported/internal distinction.
    NamespaceGet {
        namespace: Symbol,
        name: Symbol,
    },
    Unsupported,
}

// Which environment an assignment writes. R's `<-`/`=`/`->` mutate the current frame; `<<-`/`->>`
// walk the lexical chain outward past the current frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignmentScope {
    Local,
    Enclosing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssignTarget {
    // `name <- value`. `range` is the name token itself — for right assignment (`value -> name`)
    // the name sits at the end of the expression, so the expression range cannot locate it.
    Variable { symbol: Symbol, range: Range },
    // A replacement form such as `x[i] <- v`, `x$a <- v`, or `names(x) <- v`. `lhs` is the fully
    // lowered target expression (the read of the base variable plus every index/argument
    // expression), so naming and typecheck visit it like ordinary code; the written variable is the
    // symbol at the root of that expression's accessor spine (`replacement_base`).
    Replacement { lhs: ExpressionId },
}

impl ExpressionKind {
    // The plain `name <- value` / `name = value` / `value -> name` shape: the only assignment form
    // that defines a package global at top level and exports through winner semantics.
    // Super-assignments and replacement forms never define package globals, so every walker that
    // enumerates package definitions must filter through this one helper (the engine crate mirrors
    // these walks and relies on it staying the single membership rule).
    pub fn simple_assignment_target(&self) -> Option<Symbol> {
        match self {
            ExpressionKind::Assign {
                target: AssignTarget::Variable { symbol, .. },
                scope: AssignmentScope::Local,
                ..
            } => Some(*symbol),
            _ => None,
        }
    }

    // Any assignment to a plain variable name, regardless of scope — used to phrase diagnostics
    // against the assigned name (`x` in both `x <- v` and `x <<- v`).
    pub fn assignment_variable(&self) -> Option<Symbol> {
        match self {
            ExpressionKind::Assign {
                target: AssignTarget::Variable { symbol, .. },
                ..
            } => Some(*symbol),
            _ => None,
        }
    }
}

// The base variable of a replacement-form target: the symbol expression at the root of the
// accessor spine. `x$a[i] <- v` descends `Subset` → `Dollar` → `Symbol(x)`; a replacement call
// like `names(x) <- v` mutates its first argument, so `Call` descends there. Returns `None` when
// the spine does not bottom out in a plain name (for example `f(x)$a <- v`), which R itself
// rejects at run time; such targets are refused loudly rather than silently mistyped.
pub fn replacement_base(arena: &HirArena, lhs: ExpressionId) -> Option<(ExpressionId, Symbol)> {
    let mut current = lhs;
    loop {
        match &arena.get(current).kind {
            ExpressionKind::Symbol(symbol) => return Some((current, *symbol)),
            ExpressionKind::Subset { value, .. }
            | ExpressionKind::Subset2 { value, .. }
            | ExpressionKind::Dollar { value, .. }
            | ExpressionKind::Slot { value, .. } => current = *value,
            ExpressionKind::Call { arguments, .. } => {
                current = arguments.first()?.expression;
            }
            _ => return None,
        }
    }
}

// Whether an expression contains a `break` or `next` that targets the enclosing loop. Nested loop
// bodies are skipped (their `break`/`next` bind to the inner loop), but nested loop headers
// (sequence, condition) still belong to the enclosing loop's iteration. Function bodies are
// skipped entirely: a `break` inside a function definition does not exit the loop that lexically
// contains the definition.
pub fn contains_loop_exit(arena: &HirArena, root: ExpressionId) -> bool {
    match &arena.get(root).kind {
        ExpressionKind::Break | ExpressionKind::Next => true,
        ExpressionKind::NamespaceGet { .. } => false,
        ExpressionKind::Block { expressions, .. } => expressions
            .iter()
            .any(|expression| contains_loop_exit(arena, *expression)),
        ExpressionKind::Assign { target, value, .. } => {
            let target_contains = match target {
                AssignTarget::Variable { .. } => false,
                AssignTarget::Replacement { lhs } => contains_loop_exit(arena, *lhs),
            };
            target_contains || contains_loop_exit(arena, *value)
        }
        ExpressionKind::Local { body } => contains_loop_exit(arena, *body),
        ExpressionKind::If {
            condition,
            consequence,
            alternative,
        } => {
            contains_loop_exit(arena, *condition)
                || contains_loop_exit(arena, *consequence)
                || alternative.is_some_and(|alternative| contains_loop_exit(arena, alternative))
        }
        ExpressionKind::For { sequence, .. } => contains_loop_exit(arena, *sequence),
        ExpressionKind::While { condition, .. } => contains_loop_exit(arena, *condition),
        ExpressionKind::Repeat { .. } => false,
        ExpressionKind::UnaryMinus { value } | ExpressionKind::UnaryNot { value } => {
            contains_loop_exit(arena, *value)
        }
        ExpressionKind::Call { callee, arguments } => {
            contains_loop_exit(arena, *callee)
                || arguments
                    .iter()
                    .any(|argument| contains_loop_exit(arena, argument.expression))
        }
        ExpressionKind::Subset { value, arguments }
        | ExpressionKind::Subset2 { value, arguments } => {
            contains_loop_exit(arena, *value)
                || arguments
                    .iter()
                    .any(|argument| contains_loop_exit(arena, argument.expression))
        }
        ExpressionKind::Dollar { value, .. } | ExpressionKind::Slot { value, .. } => {
            contains_loop_exit(arena, *value)
        }
        ExpressionKind::Null
        | ExpressionKind::Logical(_)
        | ExpressionKind::Integer(_)
        | ExpressionKind::Double(_)
        | ExpressionKind::Character(_)
        | ExpressionKind::AtomicConstant(_)
        | ExpressionKind::StringLiteralName(_)
        | ExpressionKind::Symbol(_)
        | ExpressionKind::Function { .. }
        | ExpressionKind::Unsupported => false,
    }
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

impl DefinitionKind {
    pub fn directive_name(self) -> &'static str {
        match self {
            Self::Type => "@type",
            Self::Alias => "@alias",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    pub symbol: Symbol,
    pub range: Range,
    pub default: Option<ExpressionId>,
}

impl Parameter {
    pub fn has_default(&self) -> bool {
        self.default.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Argument {
    pub expression: ExpressionId,
    pub name: Option<Symbol>,
}

impl Module {
    pub fn render(&self, interner: &crate::interner::Interner) -> String {
        let mut out = String::new();
        for definition in &self.definitions {
            self.render_definition(definition, 0, &mut out, interner);
        }
        for expression_id in &self.expressions {
            self.render_expression(*expression_id, 0, &mut out, interner);
        }
        out
    }

    fn render_definition(
        &self,
        definition_item: &DefinitionItem,
        indent: usize,
        out: &mut String,
        interner: &crate::interner::Interner,
    ) {
        let prefix = "  ".repeat(indent);
        let definition = &definition_item.definition;
        let label = match definition.kind {
            DefinitionKind::Type => "TypeDefinition",
            DefinitionKind::Alias => "TypeAlias",
        };
        let name_str = interner.resolve(definition.name).unwrap_or("<unknown>");
        let params = if definition.type_parameters.is_empty() {
            String::new()
        } else {
            let rendered_params = definition
                .type_parameters
                .iter()
                .map(|&parameter| {
                    interner
                        .resolve(parameter)
                        .unwrap_or("<unknown>")
                        .to_owned()
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("<{rendered_params}>")
        };
        let ty_str = render_surface_type(&definition.surface_type, interner);
        out.push_str(&format!("{prefix}{label} {name_str}{params} = {ty_str}\n"));
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
            match annotation.annotation() {
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
                crate::types::Annotation::New { nominal_type } => {
                    out.push_str(&format!(
                        "{prefix}#: @new {}\n",
                        render_named_type_ref(nominal_type, interner)
                    ));
                }
            }
        }

        match &expr.kind {
            ExpressionKind::Null => out.push_str(&format!("{prefix}Null\n")),
            ExpressionKind::Logical(b) => out.push_str(&format!("{prefix}Logical({b})\n")),
            ExpressionKind::Integer(s) => out.push_str(&format!("{prefix}Integer({s:?})\n")),
            ExpressionKind::Double(s) => out.push_str(&format!("{prefix}Double({s:?})\n")),
            ExpressionKind::Character(s) => out.push_str(&format!("{prefix}Character({s:?})\n")),
            ExpressionKind::AtomicConstant(atomic) => {
                out.push_str(&format!("{prefix}AtomicConstant({atomic:?})\n"))
            }
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
                scope,
                value,
            } => {
                let label = match scope {
                    AssignmentScope::Local => "Assign",
                    AssignmentScope::Enclosing => "SuperAssign",
                };
                match target {
                    AssignTarget::Variable { symbol, .. } => {
                        let name = interner.resolve(*symbol).unwrap_or("<unknown>");
                        out.push_str(&format!("{prefix}{label} {name}\n"));
                    }
                    AssignTarget::Replacement { lhs } => {
                        out.push_str(&format!("{prefix}{label} (replacement)\n"));
                        self.render_expression(*lhs, indent + 1, out, interner);
                    }
                }
                self.render_expression(*value, indent + 1, out, interner);
            }
            ExpressionKind::Function { parameters, body } => {
                let params = parameters
                    .iter()
                    .map(|p| {
                        let name = interner.resolve(p.symbol).unwrap_or("<unknown>");
                        if p.has_default() {
                            format!("{name} = <default>")
                        } else {
                            name.to_owned()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push_str(&format!("{prefix}Function ({params})\n"));
                self.render_expression(*body, indent + 1, out, interner);
            }
            ExpressionKind::Local { body } => {
                out.push_str(&format!("{prefix}Local\n"));
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
            ExpressionKind::UnaryNot { value } => {
                out.push_str(&format!("{prefix}UnaryNot\n"));
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
            ExpressionKind::Slot { value, name } => {
                let name_str = interner.resolve(*name).unwrap_or("<unknown>");
                out.push_str(&format!("{prefix}Slot {name_str}\n"));
                self.render_expression(*value, indent + 1, out, interner);
            }
            ExpressionKind::NamespaceGet { namespace, name } => {
                out.push_str(&format!("{prefix}NamespaceGet({namespace:?}, {name:?})\n"))
            }
            ExpressionKind::Break => out.push_str(&format!("{prefix}Break\n")),
            ExpressionKind::Next => out.push_str(&format!("{prefix}Next\n")),
            ExpressionKind::Unsupported => out.push_str(&format!("{prefix}Unsupported\n")),
        }
    }
}

#[cfg(test)]
mod tests {
    use {super::*, tree_sitter::Point};

    fn range(start_byte: usize, end_byte: usize) -> Range {
        Range {
            start_byte,
            end_byte,
            start_point: Point::new(0, start_byte),
            end_point: Point::new(0, end_byte),
        }
    }

    // The span index must return the same expression the old linear arena scan did: the first
    // allocated (smallest ExpressionId) when several expressions share a range.
    #[test]
    fn expression_id_by_range_keeps_smallest_id_on_shared_range() {
        let mut arena = HirArena::new();
        let shared = range(0, 5);
        let first = arena.alloc(Expression::new(
            ExpressionId(0),
            shared,
            None,
            ExpressionKind::Null,
        ));
        let _second = arena.alloc(Expression::new(
            ExpressionId(0),
            shared,
            None,
            ExpressionKind::Null,
        ));
        let other = arena.alloc(Expression::new(
            ExpressionId(0),
            range(6, 9),
            None,
            ExpressionKind::Null,
        ));

        let module = Module::new(arena, Vec::new(), Vec::new());

        assert_eq!(module.expression_id_by_range(shared), Some(first));
        assert_eq!(module.expression_id_by_range(range(6, 9)), Some(other));
        assert_eq!(module.expression_id_by_range(range(0, 9)), None);
    }
}
