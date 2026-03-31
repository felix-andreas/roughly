use {
    crate::{
        diagnostic::{Diagnostic, DocumentDiagnostics},
        hir::{
            DefinitionItem, DefinitionKind, ExpressionId, ExpressionKind, HirArena, Module,
            ModuleId,
        },
        interner::{Interner, Symbol},
        types::{Annotation, AttachedAnnotation, NamedTypeRef, SurfaceType},
    },
    std::{
        collections::{BTreeMap, BTreeSet, HashMap},
        path::PathBuf,
    },
    tree_sitter::Range,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NamingResult {
    pub bindings: BTreeMap<BindingId, BindingInfo>,
    pub global_bindings: BTreeMap<Symbol, BindingId>,
    pub resolutions: BTreeMap<ExpressionKey, BindingId>,
    pub module_paths: BTreeMap<ModuleId, PathBuf>,
    pub diagnostics: Vec<DocumentDiagnostics>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingInfo {
    pub id: BindingId,
    pub module_id: ModuleId,
    pub symbol: Symbol,
    pub range: Range,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BindingId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExpressionKey {
    pub module_id: ModuleId,
    pub expression_id: ExpressionId,
}

pub(crate) fn resolve_package(
    modules: &[(ModuleId, PathBuf, Module)],
    interner: &Interner,
) -> NamingResult {
    let mut context = PackageNamingContext::new(modules, interner);

    for (module_id, _, module) in modules {
        context.resolve_document(*module_id, module);
    }

    context.finalize_all_bindings();
    context.record_local_resolutions();
    context.collect_types(modules);
    context.build_global_bindings(modules);
    context.resolve_definitions(modules);
    context.resolve_annotations(modules);
    context.resolve_unresolved_values();
    context.finish()
}

struct PackageNamingContext<'a> {
    interner: &'a Interner,
    documents: BTreeMap<ModuleId, DocumentNaming>,
    module_paths: BTreeMap<ModuleId, PathBuf>,
    provisional_bindings: BTreeMap<ProvisionalBindingId, ProvisionalBindingInfo>,
    bindings: BTreeMap<BindingId, BindingInfo>,
    provisional_to_final: HashMap<ProvisionalBindingId, BindingId>,
    resolutions: BTreeMap<ExpressionKey, BindingId>,
    diagnostics: BTreeMap<PathBuf, Vec<Diagnostic>>,
    global_bindings: BTreeMap<Symbol, BindingId>,
    types: BTreeMap<Symbol, TypeInfo>,
    next_provisional_binding_id: u32,
    next_binding_id: u32,
}

impl<'a> PackageNamingContext<'a> {
    fn new(modules: &[(ModuleId, PathBuf, Module)], interner: &'a Interner) -> Self {
        Self {
            interner,
            documents: BTreeMap::new(),
            module_paths: modules
                .iter()
                .map(|(module_id, path, _)| (*module_id, path.clone()))
                .collect(),
            provisional_bindings: BTreeMap::new(),
            bindings: BTreeMap::new(),
            provisional_to_final: HashMap::new(),
            resolutions: BTreeMap::new(),
            diagnostics: BTreeMap::new(),
            global_bindings: BTreeMap::new(),
            types: BTreeMap::new(),
            next_provisional_binding_id: 0,
            next_binding_id: 0,
        }
    }

    fn finish(self) -> NamingResult {
        NamingResult {
            bindings: self.bindings,
            global_bindings: self.global_bindings,
            resolutions: self.resolutions,
            module_paths: self.module_paths,
            diagnostics: self.diagnostics.into_iter().collect(),
        }
    }

    fn resolve_document(&mut self, module_id: ModuleId, module: &Module) {
        let document_naming = DocumentNamingContext::new(
            module_id,
            &module.arena,
            &mut self.next_provisional_binding_id,
            &mut self.provisional_bindings,
        )
        .resolve_module(module);
        self.documents.insert(module_id, document_naming);
    }

    fn record_local_resolutions(&mut self) {
        let documents = self
            .documents
            .iter()
            .map(|(module_id, document_naming)| {
                (*module_id, document_naming.expression_resolutions.clone())
            })
            .collect::<Vec<_>>();

        for (module_id, expression_resolutions) in documents {
            for (expression_id, provisional_binding_id) in expression_resolutions {
                let binding_id = self.finalize_binding(provisional_binding_id);
                self.resolutions.insert(
                    ExpressionKey {
                        module_id,
                        expression_id,
                    },
                    binding_id,
                );
            }
        }
    }

    fn collect_types(&mut self, modules: &[(ModuleId, PathBuf, Module)]) {
        for (module_id, _, module) in modules {
            for definition in &module.definitions {
                if let Some(existing_type) = self.types.get(&definition.definition.name) {
                    self.push_duplicate_type_definition_diagnostic(
                        definition.definition.name,
                        definition.range,
                        existing_type.kind,
                        *module_id,
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

    fn build_global_bindings(&mut self, modules: &[(ModuleId, PathBuf, Module)]) {
        for (module_id, _, _) in modules {
            let Some(document_naming) = self.documents.get(module_id).cloned() else {
                continue;
            };

            for provisional_binding_id in document_naming.top_level_exports {
                let binding_id = self.finalize_binding(provisional_binding_id);
                self.push_global_shadowing_diagnostics(binding_id);

                let symbol = self.binding_info(provisional_binding_id).symbol;
                if let Some(previous_binding_id) = self.global_bindings.insert(symbol, binding_id) {
                    self.push_duplicate_global_binding_diagnostics(previous_binding_id, binding_id);
                }
            }
        }
    }

    fn resolve_definitions(&mut self, modules: &[(ModuleId, PathBuf, Module)]) {
        for (module_id, _, module) in modules {
            for definition in &module.definitions {
                self.resolve_definition(definition, *module_id);
            }
        }
    }

    fn resolve_annotations(&mut self, modules: &[(ModuleId, PathBuf, Module)]) {
        for (module_id, _, module) in modules {
            let Some(document_naming) = self.documents.get(module_id).cloned() else {
                continue;
            };

            for expression_id in document_naming.annotated_expressions {
                let expression = module.arena.get(expression_id);
                if let Some(annotation) = &expression.annotation {
                    self.resolve_annotation(annotation, *module_id);
                }
            }
        }
    }

    fn resolve_unresolved_values(&mut self) {
        let documents = self
            .documents
            .iter()
            .map(|(module_id, document_naming)| {
                (*module_id, document_naming.unresolved_values.clone())
            })
            .collect::<Vec<_>>();

        for (module_id, unresolved_values) in documents {
            for (expression_id, symbol) in unresolved_values {
                if let Some(binding_id) = self.global_bindings.get(&symbol) {
                    self.resolutions.insert(
                        ExpressionKey {
                            module_id,
                            expression_id,
                        },
                        *binding_id,
                    );
                    continue;
                }

                if !self.is_namespace_symbol(symbol, module_id) && !self.is_builtin_symbol(symbol) {
                    let name = self.interner.resolve(symbol).unwrap_or("<unknown>");
                    let range = self.module_expression_range(module_id, expression_id);
                    self.push_diagnostic(
                        module_id,
                        Diagnostic::naming_warning(
                            range,
                            format!(
                                "I could not resolve `{name}` in this package, its imports, or builtins."
                            ),
                        ),
                    );
                }
            }
        }
    }

    fn finalize_all_bindings(&mut self) {
        let provisional_binding_ids = self
            .provisional_bindings
            .keys()
            .copied()
            .collect::<Vec<_>>();

        for provisional_binding_id in provisional_binding_ids {
            self.finalize_binding(provisional_binding_id);
        }
    }

    fn resolve_definition(&mut self, definition_item: &DefinitionItem, module_id: ModuleId) {
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
            module_id,
        );
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
                module_id: binding_info.module_id,
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

    fn binding(&self, binding_id: BindingId) -> &BindingInfo {
        self.bindings
            .get(&binding_id)
            .expect("final binding should exist")
    }

    fn resolve_annotation(&mut self, annotation: &AttachedAnnotation, module_id: ModuleId) {
        match annotation.annotation() {
            Annotation::Type { surface_type, .. } => {
                self.resolve_surface_type(
                    surface_type,
                    &BTreeSet::new(),
                    annotation.range(),
                    module_id,
                );
            }
            Annotation::New { nominal_type } => {
                self.resolve_nominal_type_ref(nominal_type, annotation.range(), module_id);
            }
        }
    }

    fn resolve_nominal_type_ref(
        &mut self,
        nominal_type: &NamedTypeRef,
        range: Range,
        module_id: ModuleId,
    ) {
        match self.types.get(&nominal_type.name) {
            Some(type_info) if type_info.kind == DefinitionKind::Type => {}
            Some(type_info) if type_info.kind == DefinitionKind::Alias => {
                let name = self
                    .render_type_name(nominal_type.name)
                    .unwrap_or_else(|| "<unknown>".to_owned());
                self.push_diagnostic(
                    module_id,
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
                self.push_unknown_type_diagnostic(nominal_type.name, range, module_id);
            }
        }

        for type_argument in &nominal_type.type_arguments {
            self.resolve_surface_type(type_argument, &BTreeSet::new(), range, module_id);
        }
    }

    fn resolve_surface_type(
        &mut self,
        surface_type: &SurfaceType,
        local_type_parameters: &BTreeSet<Symbol>,
        range: Range,
        module_id: ModuleId,
    ) {
        match surface_type {
            SurfaceType::Named(name, arguments) => {
                if !local_type_parameters.contains(name) && !self.types.contains_key(name) {
                    self.push_unknown_type_diagnostic(*name, range, module_id);
                }

                for argument in arguments {
                    self.resolve_surface_type(argument, local_type_parameters, range, module_id);
                }
            }
            SurfaceType::Nullable(inner_type)
            | SurfaceType::Vector(inner_type)
            | SurfaceType::NamedVector(inner_type)
            | SurfaceType::List(inner_type)
            | SurfaceType::NamedList(inner_type) => {
                self.resolve_surface_type(inner_type, local_type_parameters, range, module_id);
            }
            SurfaceType::Record(fields) => {
                for field in fields {
                    self.resolve_surface_type(
                        &field.value,
                        local_type_parameters,
                        range,
                        module_id,
                    );
                }
            }
            SurfaceType::Tuple(items) => {
                for item in items {
                    self.resolve_surface_type(item, local_type_parameters, range, module_id);
                }
            }
            SurfaceType::Function(function_type) => {
                for parameter in &function_type.parameters {
                    self.resolve_surface_type(parameter, local_type_parameters, range, module_id);
                }
                for parameter in &function_type.named_parameters {
                    self.resolve_surface_type(
                        &parameter.value,
                        local_type_parameters,
                        range,
                        module_id,
                    );
                }
                self.resolve_surface_type(
                    &function_type.return_type,
                    local_type_parameters,
                    range,
                    module_id,
                );
            }
            SurfaceType::Binders(type_parameters, inner_type) => {
                let mut nested_type_parameters = local_type_parameters.clone();
                nested_type_parameters.extend(type_parameters.iter().copied());
                self.resolve_surface_type(inner_type, &nested_type_parameters, range, module_id);
            }
            SurfaceType::Any
            | SurfaceType::Unknown
            | SurfaceType::Null
            | SurfaceType::Scalar(_) => {}
        }
    }

    fn push_unknown_type_diagnostic(&mut self, symbol: Symbol, range: Range, module_id: ModuleId) {
        let name = self
            .render_type_name(symbol)
            .unwrap_or_else(|| "<unknown>".to_owned());
        self.push_diagnostic(
            module_id,
            Diagnostic::syntax_error(range, format!("type syntax error: unknown type `{name}`")),
        );
    }

    fn push_duplicate_type_definition_diagnostic(
        &mut self,
        symbol: Symbol,
        range: Range,
        existing_kind: DefinitionKind,
        module_id: ModuleId,
    ) {
        let name = self
            .render_type_name(symbol)
            .unwrap_or_else(|| "<unknown>".to_owned());
        self.push_diagnostic(
            module_id,
            Diagnostic::syntax_error(
                range,
                format!(
                    "invalid semantics: type name `{name}` is already defined by an earlier {} declaration.",
                    existing_kind.directive_name()
                ),
            ),
        );
    }

    fn push_duplicate_global_binding_diagnostics(
        &mut self,
        previous_binding_id: BindingId,
        current_binding_id: BindingId,
    ) {
        let previous_binding = self.binding(previous_binding_id).clone();
        let current_binding = self.binding(current_binding_id).clone();
        let name = self
            .interner
            .resolve(current_binding.symbol)
            .unwrap_or("<unknown>")
            .to_owned();

        self.push_diagnostic(
            previous_binding.module_id,
            Diagnostic::naming_warning(
                previous_binding.range,
                format!(
                    "Top-level binding `{name}` is overwritten by a later top-level binding in this package."
                ),
            ),
        );
        self.push_diagnostic(
            current_binding.module_id,
            Diagnostic::naming_warning(
                current_binding.range,
                format!(
                    "Top-level binding `{name}` overwrites an earlier top-level binding in this package."
                ),
            ),
        );
    }

    fn push_global_shadowing_diagnostics(&mut self, binding_id: BindingId) {
        let binding = self.binding(binding_id).clone();
        if self.is_namespace_symbol(binding.symbol, binding.module_id) {
            let name = self.interner.resolve(binding.symbol).unwrap_or("<unknown>");
            self.push_diagnostic(
                binding.module_id,
                Diagnostic::naming_warning(
                    binding.range,
                    format!("Top-level binding `{name}` shadows an imported namespace symbol."),
                ),
            );
        }
        if self.is_builtin_symbol(binding.symbol) {
            let name = self.interner.resolve(binding.symbol).unwrap_or("<unknown>");
            self.push_diagnostic(
                binding.module_id,
                Diagnostic::naming_warning(
                    binding.range,
                    format!("Top-level binding `{name}` shadows a builtin."),
                ),
            );
        }
    }

    fn module_expression_range(&self, module_id: ModuleId, expression_id: ExpressionId) -> Range {
        let document_naming = self
            .documents
            .get(&module_id)
            .expect("module naming should exist");
        document_naming
            .expression_ranges
            .get(&expression_id)
            .copied()
            .expect("expression range should exist")
    }

    fn is_namespace_symbol(&self, _symbol: Symbol, _module_id: ModuleId) -> bool {
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

    fn push_diagnostic(&mut self, module_id: ModuleId, diagnostic: Diagnostic) {
        let path = self
            .module_paths
            .get(&module_id)
            .cloned()
            .expect("module path should exist");
        self.diagnostics.entry(path).or_default().push(diagnostic);
    }
}

struct DocumentNamingContext<'a> {
    module_id: ModuleId,
    arena: &'a HirArena,
    next_provisional_binding_id: &'a mut u32,
    provisional_bindings: &'a mut BTreeMap<ProvisionalBindingId, ProvisionalBindingInfo>,
    local_scopes: Vec<BTreeMap<Symbol, ProvisionalBindingId>>,
    document_naming: DocumentNaming,
}

impl<'a> DocumentNamingContext<'a> {
    fn new(
        module_id: ModuleId,
        arena: &'a HirArena,
        next_provisional_binding_id: &'a mut u32,
        provisional_bindings: &'a mut BTreeMap<ProvisionalBindingId, ProvisionalBindingInfo>,
    ) -> Self {
        Self {
            module_id,
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
        self.document_naming
            .expression_ranges
            .insert(expression_id, expression.range);
        if expression.annotation.is_some() {
            self.document_naming
                .annotated_expressions
                .push(expression_id);
        }

        match &expression.kind {
            ExpressionKind::Symbol(symbol) => match self.resolve_local_symbol(*symbol) {
                Some(binding_id) => {
                    self.document_naming
                        .expression_resolutions
                        .insert(expression_id, binding_id);
                }
                None => {
                    self.document_naming
                        .unresolved_values
                        .insert(expression_id, *symbol);
                }
            },
            ExpressionKind::Block { expressions, .. } => {
                for nested_expression in expressions {
                    self.resolve_expression(*nested_expression);
                }
            }
            ExpressionKind::Assign { target, value, .. } => {
                self.resolve_expression(*value);
                let binding_id = self.fresh_binding(*target, expression.range);
                if let Some(scope) = self.local_scopes.last_mut() {
                    scope.insert(*target, binding_id);
                } else {
                    self.document_naming.top_level_exports.push(binding_id);
                }
                self.document_naming
                    .expression_resolutions
                    .insert(expression_id, binding_id);
            }
            ExpressionKind::Function { parameters, body } => {
                let mut scope = BTreeMap::new();
                for parameter in parameters {
                    let binding_id = self.fresh_binding(parameter.symbol, parameter.range);
                    scope.insert(parameter.symbol, binding_id);
                }
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
                let binding_id = self.fresh_binding(*variable, expression.range);
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

    fn fresh_binding(&mut self, symbol: Symbol, range: Range) -> ProvisionalBindingId {
        let binding_id = ProvisionalBindingId(*self.next_provisional_binding_id);
        *self.next_provisional_binding_id += 1;
        self.provisional_bindings.insert(
            binding_id,
            ProvisionalBindingInfo {
                module_id: self.module_id,
                symbol,
                range,
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
    expression_ranges: BTreeMap<ExpressionId, Range>,
    expression_resolutions: BTreeMap<ExpressionId, ProvisionalBindingId>,
    top_level_exports: Vec<ProvisionalBindingId>,
    unresolved_values: BTreeMap<ExpressionId, Symbol>,
    annotated_expressions: Vec<ExpressionId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProvisionalBindingInfo {
    module_id: ModuleId,
    symbol: Symbol,
    range: Range,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct ProvisionalBindingId(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TypeInfo {
    kind: DefinitionKind,
}
