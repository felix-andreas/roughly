use {
    crate::{
        diagnostic::{Diagnostic, DocumentDiagnostics},
        hir::{DefinitionItem, DefinitionKind, ExpressionId, ExpressionKind, HirArena, Module},
        interner::{Interner, Symbol},
        types::{Annotation, AttachedAnnotation, NamedTypeRef, SurfaceType},
    },
    std::{
        collections::{BTreeMap, BTreeSet, HashMap},
        path::{Path, PathBuf},
    },
    tree_sitter::Range,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NamingResult {
    pub bindings: BTreeMap<BindingId, BindingInfo>,
    pub resolutions: BTreeMap<ExpressionId, BindingId>,
    pub diagnostics: Vec<DocumentDiagnostics>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingInfo {
    pub id: BindingId,
    pub symbol: Symbol,
    pub range: Range,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BindingId(pub u32);

pub(crate) fn resolve_package(modules: &[(PathBuf, Module)], interner: &Interner) -> NamingResult {
    let mut context = PackageNamingContext::new(interner);

    for (path, module) in modules {
        context.resolve_document(path, module);
    }

    context.collect_types(modules);

    for (path, module) in modules {
        context.resolve_package_document(path, module);
    }

    context.finish()
}

struct PackageNamingContext<'a> {
    interner: &'a Interner,
    documents: HashMap<PathBuf, DocumentNaming>,
    provisional_bindings: BTreeMap<ProvisionalBindingId, ProvisionalBindingInfo>,
    bindings: BTreeMap<BindingId, BindingInfo>,
    provisional_to_final: HashMap<ProvisionalBindingId, BindingId>,
    resolutions: BTreeMap<ExpressionId, BindingId>,
    diagnostics: BTreeMap<PathBuf, Vec<Diagnostic>>,
    top_level_bindings: BTreeMap<Symbol, BindingId>,
    types: BTreeMap<Symbol, TypeInfo>,
    next_provisional_binding_id: u32,
    next_binding_id: u32,
}

impl<'a> PackageNamingContext<'a> {
    fn new(interner: &'a Interner) -> Self {
        Self {
            interner,
            documents: HashMap::new(),
            provisional_bindings: BTreeMap::new(),
            bindings: BTreeMap::new(),
            provisional_to_final: HashMap::new(),
            resolutions: BTreeMap::new(),
            diagnostics: BTreeMap::new(),
            top_level_bindings: BTreeMap::new(),
            types: BTreeMap::new(),
            next_provisional_binding_id: 0,
            next_binding_id: 0,
        }
    }

    fn finish(self) -> NamingResult {
        NamingResult {
            bindings: self.bindings,
            resolutions: self.resolutions,
            diagnostics: self.diagnostics.into_iter().collect(),
        }
    }

    fn resolve_document(&mut self, path: &Path, module: &Module) {
        let document_naming = DocumentNamingContext::new(
            &module.arena,
            &mut self.next_provisional_binding_id,
            &mut self.provisional_bindings,
        )
        .resolve_module(module);
        self.documents.insert(path.to_path_buf(), document_naming);
    }

    fn collect_types(&mut self, modules: &[(PathBuf, Module)]) {
        for (path, module) in modules {
            for definition in &module.definitions {
                if let Some(existing_type) = self.types.get(&definition.definition.name) {
                    self.push_duplicate_type_definition_diagnostic(
                        definition.definition.name,
                        definition.range,
                        existing_type.kind,
                        path,
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
    }

    fn resolve_package_document(&mut self, path: &Path, module: &Module) {
        for definition in &module.definitions {
            self.resolve_definition(definition, path);
        }

        let Some(document_naming) = self.documents.get(path).cloned() else {
            return;
        };

        for expression_id in &module.expressions {
            self.resolve_package_expression(module, *expression_id, &document_naming, path);
        }
    }

    fn resolve_definition(&mut self, definition_item: &DefinitionItem, path: &Path) {
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

    fn resolve_package_expression(
        &mut self,
        module: &Module,
        expression_id: ExpressionId,
        document_naming: &DocumentNaming,
        path: &Path,
    ) {
        let expression = module.arena.get(expression_id);
        if let Some(annotation) = &expression.annotation {
            self.resolve_annotation(annotation, path);
        }

        match &expression.kind {
            ExpressionKind::Symbol(_) => {
                if let Some(resolution) = document_naming.expression_resolutions.get(&expression_id)
                {
                    match resolution {
                        ExpressionResolution::Binding(binding_id) => {
                            let final_binding_id = self.finalize_binding(*binding_id);
                            self.resolutions.insert(expression_id, final_binding_id);
                        }
                        ExpressionResolution::UnresolvedValue(symbol) => {
                            if let Some(binding_id) = self.top_level_bindings.get(symbol) {
                                self.resolutions.insert(expression_id, *binding_id);
                            } else if !self.is_namespace_symbol(*symbol, path)
                                && !self.is_builtin_symbol(*symbol)
                            {
                                let name = self.interner.resolve(*symbol).unwrap_or("<unknown>");
                                self.push_diagnostic(
                                    path,
                                    Diagnostic::naming_warning(
                                        expression.range,
                                        format!(
                                            "I could not resolve `{name}` in this package, its imports, or builtins."
                                        ),
                                    ),
                                );
                            }
                        }
                    }
                }
            }
            ExpressionKind::Block { expressions, .. } => {
                for nested_expression in expressions {
                    self.resolve_package_expression(
                        module,
                        *nested_expression,
                        document_naming,
                        path,
                    );
                }
            }
            ExpressionKind::Assign { value, .. } => {
                self.resolve_package_expression(module, *value, document_naming, path);

                let Some(ExpressionResolution::Binding(binding_id)) =
                    document_naming.expression_resolutions.get(&expression_id)
                else {
                    return;
                };

                let final_binding_id = self.finalize_binding(*binding_id);
                self.resolutions.insert(expression_id, final_binding_id);

                if self.binding_info(*binding_id).kind == BindingKind::TopLevel {
                    let symbol = self.binding_info(*binding_id).symbol;
                    if self.is_namespace_symbol(symbol, path) {
                        let name = self.interner.resolve(symbol).unwrap_or("<unknown>");
                        self.push_diagnostic(
                            path,
                            Diagnostic::naming_warning(
                                expression.range,
                                format!(
                                    "Top-level binding `{name}` shadows an imported namespace symbol."
                                ),
                            ),
                        );
                    }
                    if self.is_builtin_symbol(symbol) {
                        let name = self.interner.resolve(symbol).unwrap_or("<unknown>");
                        self.push_diagnostic(
                            path,
                            Diagnostic::naming_warning(
                                expression.range,
                                format!("Top-level binding `{name}` shadows a builtin."),
                            ),
                        );
                    }
                    self.top_level_bindings.insert(symbol, final_binding_id);
                }
            }
            ExpressionKind::Function { body, .. } => {
                if let Some(parameter_bindings) =
                    document_naming.function_parameters.get(&expression_id)
                {
                    for binding_id in parameter_bindings {
                        self.finalize_binding(*binding_id);
                    }
                }
                self.resolve_package_expression(module, *body, document_naming, path);
            }
            ExpressionKind::If {
                condition,
                consequence,
                alternative,
            } => {
                self.resolve_package_expression(module, *condition, document_naming, path);
                self.resolve_package_expression(module, *consequence, document_naming, path);
                if let Some(alternative) = alternative {
                    self.resolve_package_expression(module, *alternative, document_naming, path);
                }
            }
            ExpressionKind::For { sequence, body, .. } => {
                self.resolve_package_expression(module, *sequence, document_naming, path);
                if let Some(binding_id) = document_naming.loop_bindings.get(&expression_id) {
                    self.finalize_binding(*binding_id);
                }
                self.resolve_package_expression(module, *body, document_naming, path);
            }
            ExpressionKind::While { condition, body } => {
                self.resolve_package_expression(module, *condition, document_naming, path);
                self.resolve_package_expression(module, *body, document_naming, path);
            }
            ExpressionKind::Repeat { body } => {
                self.resolve_package_expression(module, *body, document_naming, path);
            }
            ExpressionKind::UnaryMinus { value } => {
                self.resolve_package_expression(module, *value, document_naming, path);
            }
            ExpressionKind::Call { callee, arguments } => {
                self.resolve_package_expression(module, *callee, document_naming, path);
                for argument in arguments {
                    self.resolve_package_expression(
                        module,
                        argument.expression,
                        document_naming,
                        path,
                    );
                }
            }
            ExpressionKind::Subset { value, arguments }
            | ExpressionKind::Subset2 { value, arguments } => {
                self.resolve_package_expression(module, *value, document_naming, path);
                for argument in arguments {
                    self.resolve_package_expression(
                        module,
                        argument.expression,
                        document_naming,
                        path,
                    );
                }
            }
            ExpressionKind::Dollar { value, .. } => {
                self.resolve_package_expression(module, *value, document_naming, path);
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

    fn finalize_binding(&mut self, provisional_binding_id: ProvisionalBindingId) -> BindingId {
        if let Some(binding_id) = self.provisional_to_final.get(&provisional_binding_id) {
            return *binding_id;
        }

        let binding_id = BindingId(self.next_binding_id);
        self.next_binding_id += 1;
        let binding_info = self.binding_info(provisional_binding_id);
        self.bindings.insert(
            binding_id,
            BindingInfo {
                id: binding_id,
                symbol: binding_info.symbol,
                range: binding_info.range,
            },
        );
        self.provisional_to_final
            .insert(provisional_binding_id, binding_id);
        binding_id
    }

    fn binding_info(
        &self,
        provisional_binding_id: ProvisionalBindingId,
    ) -> &ProvisionalBindingInfo {
        self.provisional_bindings
            .get(&provisional_binding_id)
            .expect("provisional binding should exist")
    }

    fn resolve_annotation(&mut self, annotation: &AttachedAnnotation, path: &Path) {
        match annotation.annotation() {
            Annotation::Type { surface_type, .. } => {
                self.resolve_surface_type(surface_type, &BTreeSet::new(), annotation.range(), path);
            }
            Annotation::New { nominal_type } => {
                self.resolve_nominal_type_ref(nominal_type, annotation.range(), path);
            }
        }
    }

    fn resolve_nominal_type_ref(&mut self, nominal_type: &NamedTypeRef, range: Range, path: &Path) {
        match self.types.get(&nominal_type.name) {
            Some(type_info) if type_info.kind == DefinitionKind::Type => {}
            Some(type_info) if type_info.kind == DefinitionKind::Alias => {
                let name = self
                    .render_type_name(nominal_type.name)
                    .unwrap_or_else(|| "<unknown>".to_owned());
                self.push_diagnostic(
                    path,
                    Diagnostic::syntax_error(
                        range,
                        format!(
                            "invalid semantics: `@new` requires a nominal type declared with `@type`, but `{name}` is an alias."
                        ),
                    ),
                );
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
        path: &Path,
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

    fn push_unknown_type_diagnostic(&mut self, symbol: Symbol, range: Range, path: &Path) {
        let name = self
            .render_type_name(symbol)
            .unwrap_or_else(|| "<unknown>".to_owned());
        self.push_diagnostic(
            path,
            Diagnostic::syntax_error(range, format!("type syntax error: unknown type `{name}`")),
        );
    }

    fn push_duplicate_type_definition_diagnostic(
        &mut self,
        symbol: Symbol,
        range: Range,
        existing_kind: DefinitionKind,
        path: &Path,
    ) {
        let name = self
            .render_type_name(symbol)
            .unwrap_or_else(|| "<unknown>".to_owned());
        self.push_diagnostic(
            path,
            Diagnostic::syntax_error(
                range,
                format!(
                    "invalid semantics: type name `{name}` is already defined by an earlier {} declaration.",
                    existing_kind.directive_name()
                ),
            ),
        );
    }

    fn is_namespace_symbol(&self, _symbol: Symbol, _path: &Path) -> bool {
        false
    }

    fn is_builtin_symbol(&self, symbol: Symbol) -> bool {
        matches!(
            self.interner.resolve(symbol),
            Some("+" | "-" | "*" | "/" | "**" | "&&" | "||" | "c" | "list")
        )
    }

    fn render_type_name(&self, symbol: Symbol) -> Option<String> {
        self.interner.resolve(symbol).map(str::to_owned)
    }

    fn push_diagnostic(&mut self, path: &Path, diagnostic: Diagnostic) {
        self.diagnostics
            .entry(path.to_path_buf())
            .or_default()
            .push(diagnostic);
    }
}

struct DocumentNamingContext<'a> {
    arena: &'a HirArena,
    next_provisional_binding_id: &'a mut u32,
    provisional_bindings: &'a mut BTreeMap<ProvisionalBindingId, ProvisionalBindingInfo>,
    local_scopes: Vec<BTreeMap<Symbol, ProvisionalBindingId>>,
    document_naming: DocumentNaming,
}

impl<'a> DocumentNamingContext<'a> {
    fn new(
        arena: &'a HirArena,
        next_provisional_binding_id: &'a mut u32,
        provisional_bindings: &'a mut BTreeMap<ProvisionalBindingId, ProvisionalBindingInfo>,
    ) -> Self {
        Self {
            arena,
            next_provisional_binding_id,
            provisional_bindings,
            local_scopes: Vec::new(),
            document_naming: DocumentNaming::default(),
        }
    }

    fn resolve_module(mut self, module: &Module) -> DocumentNaming {
        for expression_id in &module.expressions {
            self.resolve_expression(*expression_id);
        }

        self.document_naming
    }

    fn resolve_expression(&mut self, expression_id: ExpressionId) {
        let expression = self.arena.get(expression_id);

        match &expression.kind {
            ExpressionKind::Symbol(symbol) => match self.resolve_local_symbol(*symbol) {
                Some(binding_id) => {
                    self.document_naming
                        .expression_resolutions
                        .insert(expression_id, ExpressionResolution::Binding(binding_id));
                }
                None => {
                    self.document_naming.expression_resolutions.insert(
                        expression_id,
                        ExpressionResolution::UnresolvedValue(*symbol),
                    );
                }
            },
            ExpressionKind::Block { expressions, .. } => {
                for nested_expression in expressions {
                    self.resolve_expression(*nested_expression);
                }
            }
            ExpressionKind::Assign { target, value, .. } => {
                self.resolve_expression(*value);
                let binding_kind = if self.local_scopes.is_empty() {
                    BindingKind::TopLevel
                } else {
                    BindingKind::Local
                };
                let binding_id = self.fresh_binding(*target, expression.range, binding_kind);
                if let Some(scope) = self.local_scopes.last_mut() {
                    scope.insert(*target, binding_id);
                }
                self.document_naming
                    .expression_resolutions
                    .insert(expression_id, ExpressionResolution::Binding(binding_id));
            }
            ExpressionKind::Function { parameters, body } => {
                let mut scope = BTreeMap::new();
                let mut parameter_bindings = Vec::with_capacity(parameters.len());
                for parameter in parameters {
                    let binding_id =
                        self.fresh_binding(parameter.symbol, parameter.range, BindingKind::Local);
                    scope.insert(parameter.symbol, binding_id);
                    parameter_bindings.push(binding_id);
                }
                self.document_naming
                    .function_parameters
                    .insert(expression_id, parameter_bindings);
                self.local_scopes.push(scope);
                self.resolve_expression(*body);
                self.local_scopes.pop();
            }
            ExpressionKind::If {
                condition,
                consequence,
                alternative,
            } => {
                self.resolve_expression(*condition);
                self.resolve_expression(*consequence);
                if let Some(alternative) = alternative {
                    self.resolve_expression(*alternative);
                }
            }
            ExpressionKind::For {
                variable,
                sequence,
                body,
            } => {
                self.resolve_expression(*sequence);
                let binding_id =
                    self.fresh_binding(*variable, expression.range, BindingKind::Local);
                self.document_naming
                    .loop_bindings
                    .insert(expression_id, binding_id);
                self.local_scopes
                    .push(BTreeMap::from([(*variable, binding_id)]));
                self.resolve_expression(*body);
                self.local_scopes.pop();
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
                for argument in arguments {
                    self.resolve_expression(argument.expression);
                }
            }
            ExpressionKind::Subset { value, arguments }
            | ExpressionKind::Subset2 { value, arguments } => {
                self.resolve_expression(*value);
                for argument in arguments {
                    self.resolve_expression(argument.expression);
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

    fn fresh_binding(
        &mut self,
        symbol: Symbol,
        range: Range,
        kind: BindingKind,
    ) -> ProvisionalBindingId {
        let binding_id = ProvisionalBindingId(*self.next_provisional_binding_id);
        *self.next_provisional_binding_id += 1;
        self.provisional_bindings.insert(
            binding_id,
            ProvisionalBindingInfo {
                symbol,
                range,
                kind,
            },
        );
        binding_id
    }

    fn resolve_local_symbol(&self, symbol: Symbol) -> Option<ProvisionalBindingId> {
        for scope in self.local_scopes.iter().rev() {
            if let Some(binding_id) = scope.get(&symbol) {
                return Some(*binding_id);
            }
        }

        None
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct DocumentNaming {
    expression_resolutions: BTreeMap<ExpressionId, ExpressionResolution>,
    function_parameters: BTreeMap<ExpressionId, Vec<ProvisionalBindingId>>,
    loop_bindings: BTreeMap<ExpressionId, ProvisionalBindingId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ExpressionResolution {
    Binding(ProvisionalBindingId),
    UnresolvedValue(Symbol),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProvisionalBindingInfo {
    symbol: Symbol,
    range: Range,
    kind: BindingKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BindingKind {
    Local,
    TopLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct ProvisionalBindingId(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TypeInfo {
    kind: DefinitionKind,
}
