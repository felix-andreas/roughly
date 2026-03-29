use {
    crate::{
        diagnostic::Diagnostic,
        hir::{
            DefinitionId, DefinitionItem, DefinitionKind, ExpressionId, ExpressionKind, HirArena,
            Module,
        },
        interner::{Interner, Symbol},
        types::{Annotation, AttachedAnnotation, NamedTypeRef, SurfaceType},
    },
    std::{
        collections::{BTreeMap, BTreeSet, HashMap},
        path::{Path, PathBuf},
    },
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
    definition_paths: &'a HashMap<DefinitionId, PathBuf>,
    expression_paths: &'a HashMap<ExpressionId, PathBuf>,
    bindings: BTreeMap<BindingId, BindingInfo>,
    resolutions: BTreeMap<ExpressionId, BindingId>,
    diagnostics: Vec<Diagnostic>,
    scopes: Vec<BTreeMap<Symbol, BindingId>>,
    types: BTreeMap<Symbol, TypeInfo>,
    next_binding_id: u32,
}

impl<'a> NamingContext<'a> {
    pub fn new(
        arena: &'a HirArena,
        interner: &'a Interner,
        definition_paths: &'a HashMap<DefinitionId, PathBuf>,
        expression_paths: &'a HashMap<ExpressionId, PathBuf>,
    ) -> Self {
        Self {
            arena,
            interner,
            definition_paths,
            expression_paths,
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
            let definition_path = self.definition_path(definition.id);
            self.resolve_definition(definition, definition_path.as_deref());
        }

        for expression_id in &module.expressions {
            let expression_path = self.expression_path(*expression_id);
            self.resolve_expression(*expression_id, expression_path.as_deref());
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
                let definition_path = self.definition_path(definition.id);
                self.push_duplicate_type_definition_diagnostic(
                    definition.definition.name,
                    definition.range,
                    existing_type.kind,
                    definition_path.as_deref(),
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

    fn resolve_definition(&mut self, definition_item: &DefinitionItem, path: Option<&Path>) {
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
            path,
        );
    }

    fn resolve_expression(&mut self, expr_id: ExpressionId, path: Option<&Path>) {
        let expr = self.arena.get(expr_id);
        if let Some(annotation) = &expr.annotation {
            self.resolve_annotation(annotation, path);
        }

        match &expr.kind {
            ExpressionKind::Symbol(symbol) => {
                if let Some(binding_id) = self.resolve_symbol(*symbol) {
                    self.resolutions.insert(expr_id, binding_id);
                }
            }
            ExpressionKind::Block { expressions, .. } => {
                for id in expressions {
                    self.resolve_expression(*id, path);
                }
            }
            ExpressionKind::Assign { target, value, .. } => {
                self.resolve_expression(*value, path);
                let binding_id = self.introduce_binding(*target, expr.range);
                self.resolutions.insert(expr_id, binding_id);
            }
            ExpressionKind::Function { parameters, body } => {
                self.scopes.push(BTreeMap::new());
                for param in parameters {
                    self.introduce_binding(param.symbol, param.range);
                }
                self.resolve_expression(*body, path);
                self.scopes.pop();
            }
            ExpressionKind::If {
                condition,
                consequence,
                alternative,
            } => {
                self.resolve_expression(*condition, path);
                self.resolve_expression(*consequence, path);
                if let Some(alt) = alternative {
                    self.resolve_expression(*alt, path);
                }
            }
            ExpressionKind::For {
                variable,
                sequence,
                body,
            } => {
                self.resolve_expression(*sequence, path);
                self.scopes.push(BTreeMap::new());
                self.introduce_binding(*variable, expr.range);
                self.resolve_expression(*body, path);
                self.scopes.pop();
            }
            ExpressionKind::While { condition, body } => {
                self.resolve_expression(*condition, path);
                self.resolve_expression(*body, path);
            }
            ExpressionKind::Repeat { body } => {
                self.resolve_expression(*body, path);
            }
            ExpressionKind::UnaryMinus { value } => {
                self.resolve_expression(*value, path);
            }
            ExpressionKind::Call { callee, arguments } => {
                self.resolve_expression(*callee, path);
                for arg in arguments {
                    self.resolve_expression(arg.expression, path);
                }
            }
            ExpressionKind::Subset { value, arguments }
            | ExpressionKind::Subset2 { value, arguments } => {
                self.resolve_expression(*value, path);
                for arg in arguments {
                    self.resolve_expression(arg.expression, path);
                }
            }
            ExpressionKind::Dollar { value, .. } => {
                self.resolve_expression(*value, path);
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

    fn resolve_annotation(&mut self, annotation: &AttachedAnnotation, path: Option<&Path>) {
        match annotation.annotation() {
            Annotation::Type { surface_type, .. } => {
                self.resolve_surface_type(surface_type, &BTreeSet::new(), annotation.range(), path);
            }
            Annotation::New { nominal_type } => {
                self.resolve_nominal_type_ref(nominal_type, annotation.range(), path);
            }
        }
    }

    fn resolve_nominal_type_ref(
        &mut self,
        nominal_type: &NamedTypeRef,
        range: Range,
        path: Option<&Path>,
    ) {
        match self.types.get(&nominal_type.name) {
            Some(type_info) if type_info.kind == DefinitionKind::Type => {}
            Some(type_info) if type_info.kind == DefinitionKind::Alias => {
                let name = self
                    .render_type_name(nominal_type.name)
                    .unwrap_or_else(|| "<unknown>".to_owned());
                self.diagnostics.push(self.diagnostic_with_path(
                    Diagnostic::syntax_error(
                        range,
                        format!(
                            "invalid semantics: `@new` requires a nominal type declared with `@type`, but `{name}` is an alias."
                        ),
                    ),
                    path,
                ));
            }
            Some(_) => {}
            None => {
                self.push_unknown_type_diagnostic(nominal_type.name, range, path);
            }
        }

        for type_argument in &nominal_type.type_arguments {
            self.resolve_surface_type(type_argument, &BTreeSet::new(), range, path);
        }
    }

    fn resolve_surface_type(
        &mut self,
        surface_type: &SurfaceType,
        local_type_parameters: &BTreeSet<Symbol>,
        range: Range,
        path: Option<&Path>,
    ) {
        match surface_type {
            SurfaceType::Named(name, arguments) => {
                if !local_type_parameters.contains(name) && !self.types.contains_key(name) {
                    self.push_unknown_type_diagnostic(*name, range, path);
                }

                for argument in arguments {
                    self.resolve_surface_type(argument, local_type_parameters, range, path);
                }
            }
            SurfaceType::Nullable(inner_type)
            | SurfaceType::Vector(inner_type)
            | SurfaceType::NamedVector(inner_type)
            | SurfaceType::List(inner_type)
            | SurfaceType::NamedList(inner_type) => {
                self.resolve_surface_type(inner_type, local_type_parameters, range, path);
            }
            SurfaceType::Record(fields) => {
                for field in fields {
                    self.resolve_surface_type(&field.value, local_type_parameters, range, path);
                }
            }
            SurfaceType::Tuple(items) => {
                for item in items {
                    self.resolve_surface_type(item, local_type_parameters, range, path);
                }
            }
            SurfaceType::Function(function_type) => {
                for parameter in &function_type.parameters {
                    self.resolve_surface_type(parameter, local_type_parameters, range, path);
                }
                for parameter in &function_type.named_parameters {
                    self.resolve_surface_type(&parameter.value, local_type_parameters, range, path);
                }
                self.resolve_surface_type(
                    &function_type.return_type,
                    local_type_parameters,
                    range,
                    path,
                );
            }
            SurfaceType::Binders(type_parameters, inner_type) => {
                let mut nested_type_parameters = local_type_parameters.clone();
                nested_type_parameters.extend(type_parameters.iter().copied());
                self.resolve_surface_type(inner_type, &nested_type_parameters, range, path);
            }
            SurfaceType::Any
            | SurfaceType::Unknown
            | SurfaceType::Null
            | SurfaceType::Scalar(_) => {}
        }
    }

    fn push_unknown_type_diagnostic(&mut self, symbol: Symbol, range: Range, path: Option<&Path>) {
        let name = self
            .render_type_name(symbol)
            .unwrap_or_else(|| "<unknown>".to_owned());
        self.diagnostics.push(self.diagnostic_with_path(
            Diagnostic::syntax_error(range, format!("type syntax error: unknown type `{name}`")),
            path,
        ));
    }

    fn push_duplicate_type_definition_diagnostic(
        &mut self,
        symbol: Symbol,
        range: Range,
        existing_kind: DefinitionKind,
        path: Option<&Path>,
    ) {
        let name = self
            .render_type_name(symbol)
            .unwrap_or_else(|| "<unknown>".to_owned());
        self.diagnostics.push(self.diagnostic_with_path(
            Diagnostic::syntax_error(
                range,
                format!(
                    "invalid semantics: type name `{name}` is already defined by an earlier {} declaration.",
                    existing_kind.directive_name()
                ),
            ),
            path,
        ));
    }

    fn render_type_name(&self, symbol: Symbol) -> Option<String> {
        self.interner.resolve(symbol).map(str::to_owned)
    }

    fn definition_path(&self, definition_id: DefinitionId) -> Option<PathBuf> {
        self.definition_paths.get(&definition_id).cloned()
    }

    fn expression_path(&self, expression_id: ExpressionId) -> Option<PathBuf> {
        self.expression_paths.get(&expression_id).cloned()
    }

    fn diagnostic_with_path(&self, diagnostic: Diagnostic, path: Option<&Path>) -> Diagnostic {
        match path {
            Some(path) => diagnostic.with_path(path.to_path_buf()),
            None => diagnostic,
        }
    }
}
