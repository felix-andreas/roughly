use {
    crate::{
        diagnostic::Diagnostic,
        hir::{ExpressionId, ExpressionKind, HirArena, Module},
        interner::Symbol,
    },
    std::collections::BTreeMap,
    tree_sitter::Range,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BindingId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingInfo {
    pub id: BindingId,
    pub symbol: Symbol,
    pub range: Range,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamingResult {
    pub bindings: BTreeMap<BindingId, BindingInfo>,
    pub resolutions: BTreeMap<ExpressionId, BindingId>,
    pub diagnostics: Vec<Diagnostic>,
}

pub struct NamingContext<'a> {
    arena: &'a HirArena,
    bindings: BTreeMap<BindingId, BindingInfo>,
    resolutions: BTreeMap<ExpressionId, BindingId>,
    diagnostics: Vec<Diagnostic>,
    scopes: Vec<BTreeMap<Symbol, BindingId>>,
    next_binding_id: u32,
}

impl<'a> NamingContext<'a> {
    pub fn new(arena: &'a HirArena) -> Self {
        Self {
            arena,
            bindings: BTreeMap::new(),
            resolutions: BTreeMap::new(),
            diagnostics: Vec::new(),
            scopes: vec![BTreeMap::new()], // global scope
            next_binding_id: 0,
        }
    }

    fn fresh_binding(&mut self, symbol: Symbol, range: Range) -> BindingId {
        let id = BindingId(self.next_binding_id);
        self.next_binding_id += 1;
        self.bindings.insert(id, BindingInfo { id, symbol, range });
        id
    }

    fn introduce_binding(&mut self, symbol: Symbol, range: Range) -> BindingId {
        let id = self.fresh_binding(symbol, range);
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(symbol, id);
        }
        id
    }

    fn resolve_symbol(&self, symbol: Symbol) -> Option<BindingId> {
        for scope in self.scopes.iter().rev() {
            if let Some(id) = scope.get(&symbol) {
                return Some(*id);
            }
        }
        None
    }

    pub fn resolve_module(mut self, module: &Module) -> NamingResult {
        for expr_id in &module.root_expressions {
            self.resolve_expression(*expr_id);
        }

        NamingResult {
            bindings: self.bindings,
            resolutions: self.resolutions,
            diagnostics: self.diagnostics,
        }
    }

    fn resolve_expression(&mut self, expr_id: ExpressionId) {
        let expr = self.arena.get(expr_id);
        match &expr.kind {
            ExpressionKind::Symbol(symbol) => {
                if let Some(binding_id) = self.resolve_symbol(*symbol) {
                    self.resolutions.insert(expr_id, binding_id);
                }
            }
            ExpressionKind::Block { expressions, .. } => {
                for id in expressions {
                    self.resolve_expression(*id);
                }
            }
            ExpressionKind::Assign { target, value, .. } => {
                self.resolve_expression(*value);
                let binding_id = self.introduce_binding(*target, expr.range);
                self.resolutions.insert(expr_id, binding_id);
            }
            ExpressionKind::Function { parameters, body } => {
                self.scopes.push(BTreeMap::new());
                for param in parameters {
                    self.introduce_binding(param.symbol, param.range);
                }
                self.resolve_expression(*body);
                self.scopes.pop();
            }
            ExpressionKind::If {
                condition,
                consequence,
                alternative,
            } => {
                self.resolve_expression(*condition);
                self.resolve_expression(*consequence);
                if let Some(alt) = alternative {
                    self.resolve_expression(*alt);
                }
            }
            ExpressionKind::For {
                variable,
                sequence,
                body,
            } => {
                self.resolve_expression(*sequence);
                self.scopes.push(BTreeMap::new());
                self.introduce_binding(*variable, expr.range);
                self.resolve_expression(*body);
                self.scopes.pop();
            }
            ExpressionKind::While { condition, body } => {
                self.resolve_expression(*condition);
                self.resolve_expression(*body);
            }
            ExpressionKind::Repeat { body } => {
                self.resolve_expression(*body);
            }
            ExpressionKind::UnaryMinus { value } => {
                self.resolve_expression(*value);
            }
            ExpressionKind::Call { callee, arguments } => {
                self.resolve_expression(*callee);
                for arg in arguments {
                    self.resolve_expression(arg.expression);
                }
            }
            ExpressionKind::Subset { value, arguments }
            | ExpressionKind::Subset2 { value, arguments } => {
                self.resolve_expression(*value);
                for arg in arguments {
                    self.resolve_expression(arg.expression);
                }
            }
            ExpressionKind::Dollar { value, .. } => {
                self.resolve_expression(*value);
            }
            ExpressionKind::TypeAlias { .. } => {
                // Note: type names are not currently resolved in the value namespace
            }
            ExpressionKind::Null
            | ExpressionKind::Logical(_)
            | ExpressionKind::Integer(_)
            | ExpressionKind::Double(_)
            | ExpressionKind::Character(_)
            | ExpressionKind::StringLiteralName(_)
            | ExpressionKind::Unsupported => {}
        }
    }
}
