use {
    crate::{
        diagnostic::Diagnostic,
        hir::{DefinitionItem, DefinitionKind, ExpressionId, ExpressionKind, HirArena, Module},
        interner::{Interner, Symbol},
        types::{Annotation, AttachedAnnotation, NamedTypeRef, SurfaceType},
    },
    std::collections::{BTreeMap, BTreeSet},
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TypeInfo {
    kind: DefinitionKind,
}

pub struct NamingContext<'a> {
    arena: &'a HirArena,
    interner: &'a Interner,
    bindings: BTreeMap<BindingId, BindingInfo>,
    resolutions: BTreeMap<ExpressionId, BindingId>,
    diagnostics: Vec<Diagnostic>,
    scopes: Vec<BTreeMap<Symbol, BindingId>>,
    types: BTreeMap<Symbol, TypeInfo>,
    next_binding_id: u32,
}

impl<'a> NamingContext<'a> {
    pub fn new(arena: &'a HirArena, interner: &'a Interner) -> Self {
        Self {
            arena,
            interner,
            bindings: BTreeMap::new(),
            resolutions: BTreeMap::new(),
            diagnostics: Vec::new(),
            scopes: vec![BTreeMap::new()], // global scope
            types: BTreeMap::new(),
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
        self.collect_types(module);

        for definition in &module.definitions {
            self.resolve_definition(definition);
        }

        for expression_id in &module.expressions {
            self.resolve_expression(*expression_id);
        }

        NamingResult {
            bindings: self.bindings,
            resolutions: self.resolutions,
            diagnostics: self.diagnostics,
        }
    }

    fn collect_types(&mut self, module: &Module) {
        for definition in &module.definitions {
            if let Some(existing_type) = self.types.get(&definition.definition.name) {
                self.push_duplicate_type_definition_diagnostic(
                    definition.definition.name,
                    definition.range,
                    existing_type.kind,
                );
                continue;
            }

            self.types.insert(
                definition.definition.name,
                TypeInfo {
                    kind: definition.definition.kind,
                },
            );
        }
    }

    fn resolve_definition(&mut self, definition_item: &DefinitionItem) {
        let local_type_parameters = definition_item
            .definition
            .type_parameters
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();

        self.resolve_surface_type(
            &definition_item.definition.surface_type,
            &local_type_parameters,
            definition_item.range,
        );
    }

    fn resolve_expression(&mut self, expr_id: ExpressionId) {
        let expr = self.arena.get(expr_id);
        if let Some(annotation) = &expr.annotation {
            self.resolve_annotation(annotation);
        }

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
            ExpressionKind::Null
            | ExpressionKind::Logical(_)
            | ExpressionKind::Integer(_)
            | ExpressionKind::Double(_)
            | ExpressionKind::Character(_)
            | ExpressionKind::StringLiteralName(_)
            | ExpressionKind::Unsupported => {}
        }
    }

    fn resolve_annotation(&mut self, annotation: &AttachedAnnotation) {
        match annotation.annotation() {
            Annotation::Type { surface_type, .. } => {
                self.resolve_surface_type(surface_type, &BTreeSet::new(), annotation.range());
            }
            Annotation::New { nominal_type } => {
                self.resolve_nominal_type_ref(nominal_type, annotation.range());
            }
        }
    }

    fn resolve_nominal_type_ref(&mut self, nominal_type: &NamedTypeRef, range: Range) {
        match self.types.get(&nominal_type.name) {
            Some(type_info) if type_info.kind == DefinitionKind::Type => {}
            Some(type_info) if type_info.kind == DefinitionKind::Alias => {
                let name = self
                    .render_type_name(nominal_type.name)
                    .unwrap_or_else(|| "<unknown>".to_owned());
                self.diagnostics.push(Diagnostic::syntax_error(
                    range,
                    format!(
                        "invalid semantics: `@new` requires a nominal type declared with `@type`, but `{name}` is an alias."
                    ),
                ));
            }
            Some(_) => {}
            None => {
                self.push_unknown_type_diagnostic(nominal_type.name, range);
            }
        }

        for type_argument in &nominal_type.type_arguments {
            self.resolve_surface_type(type_argument, &BTreeSet::new(), range);
        }
    }

    fn resolve_surface_type(
        &mut self,
        surface_type: &SurfaceType,
        local_type_parameters: &BTreeSet<Symbol>,
        range: Range,
    ) {
        match surface_type {
            SurfaceType::Named(name, arguments) => {
                if !local_type_parameters.contains(name) && !self.types.contains_key(name) {
                    self.push_unknown_type_diagnostic(*name, range);
                }

                for argument in arguments {
                    self.resolve_surface_type(argument, local_type_parameters, range);
                }
            }
            SurfaceType::Nullable(inner_type)
            | SurfaceType::Vector(inner_type)
            | SurfaceType::NamedVector(inner_type)
            | SurfaceType::List(inner_type)
            | SurfaceType::NamedList(inner_type) => {
                self.resolve_surface_type(inner_type, local_type_parameters, range);
            }
            SurfaceType::Record(fields) => {
                for field in fields {
                    self.resolve_surface_type(&field.value, local_type_parameters, range);
                }
            }
            SurfaceType::Tuple(items) => {
                for item in items {
                    self.resolve_surface_type(item, local_type_parameters, range);
                }
            }
            SurfaceType::Function(function_type) => {
                for parameter in &function_type.parameters {
                    self.resolve_surface_type(parameter, local_type_parameters, range);
                }
                for parameter in &function_type.named_parameters {
                    self.resolve_surface_type(&parameter.value, local_type_parameters, range);
                }
                self.resolve_surface_type(&function_type.return_type, local_type_parameters, range);
            }
            SurfaceType::Binders(type_parameters, inner_type) => {
                let mut nested_type_parameters = local_type_parameters.clone();
                nested_type_parameters.extend(type_parameters.iter().copied());
                self.resolve_surface_type(inner_type, &nested_type_parameters, range);
            }
            SurfaceType::Any
            | SurfaceType::Unknown
            | SurfaceType::Null
            | SurfaceType::Scalar(_) => {}
        }
    }

    fn push_unknown_type_diagnostic(&mut self, symbol: Symbol, range: Range) {
        let name = self
            .render_type_name(symbol)
            .unwrap_or_else(|| "<unknown>".to_owned());
        self.diagnostics.push(Diagnostic::syntax_error(
            range,
            format!("type syntax error: unknown type `{name}`"),
        ));
    }

    fn push_duplicate_type_definition_diagnostic(
        &mut self,
        symbol: Symbol,
        range: Range,
        existing_kind: DefinitionKind,
    ) {
        let name = self
            .render_type_name(symbol)
            .unwrap_or_else(|| "<unknown>".to_owned());
        self.diagnostics.push(Diagnostic::syntax_error(
            range,
            format!(
                "invalid semantics: type name `{name}` is already defined by an earlier {} declaration.",
                existing_kind.directive_name()
            ),
        ));
    }

    fn render_type_name(&self, symbol: Symbol) -> Option<String> {
        self.interner.resolve(symbol).map(str::to_owned)
    }
}
