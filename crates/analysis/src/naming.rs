use {
    crate::{
        diagnostic::Diagnostic,
        document::DocumentId,
        hir::{
            DefinitionItem, DefinitionKind, ExpressionId, ExpressionKind, HirArena, Module,
            ModuleId,
        },
        interner::{Interner, Symbol},
        types::{Annotation, AttachedAnnotation, NamedTypeRef, SurfaceType},
    },
    std::collections::{BTreeMap, BTreeSet, HashMap},
    tree_sitter::Range,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NamesGlobal {
    pub bindings: BTreeMap<BindingId, BindingInfo>,
    pub global_bindings: BTreeMap<Symbol, BindingId>,
    pub resolutions: BTreeMap<ExpressionKey, BindingId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NamesLocal {
    pub bindings: BTreeMap<ProvisionalBindingId, ProvisionalBindingInfo>,
    pub expression_resolutions: BTreeMap<ExpressionId, ProvisionalBindingId>,
    pub top_level_exports: Vec<ProvisionalBindingId>,
    pub unresolved_values: BTreeMap<ExpressionId, Symbol>,
    pub annotated_expressions: Vec<ExpressionId>,
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PackageNamingComputation {
    pub locals: HashMap<DocumentId, NamesLocal>,
    pub naming: NamesGlobal,
    pub diagnostics: HashMap<DocumentId, Vec<Diagnostic>>,
}

pub(crate) fn resolve_package(
    package_modules: &[(DocumentId, &Module)],
    extra_modules: &[(DocumentId, &Module)],
    interner: &Interner,
) -> PackageNamingComputation {
    let all_modules = package_modules
        .iter()
        .chain(extra_modules.iter())
        .copied()
        .collect::<Vec<_>>();
    let mut context = PackageNamingContext::new(interner, &all_modules);

    for (document_id, module) in &all_modules {
        context.resolve_document(*document_id, module);
    }

    context.finalize_all_bindings();
    context.record_local_resolutions();
    context.collect_types(package_modules);
    context.build_global_bindings(package_modules);
    context.resolve_definitions(&all_modules);
    context.resolve_annotations(&all_modules);
    context.resolve_unresolved_values();
    context.finish()
}

pub fn resolve_document_locally(document_id: DocumentId, module: &Module) -> NamesLocal {
    let mut next_provisional_binding_id = 0;
    let mut provisional_bindings = BTreeMap::new();
    DocumentNamingContext::new(
        document_id,
        &module.arena,
        &mut next_provisional_binding_id,
        &mut provisional_bindings,
    )
    .resolve_module(module)
}

struct PackageNamingContext<'a> {
    interner: &'a Interner,
    modules: BTreeMap<DocumentId, &'a Module>,
    documents: BTreeMap<DocumentId, NamesLocal>,
    provisional_bindings: BTreeMap<ProvisionalBindingId, ProvisionalBindingInfo>,
    bindings: BTreeMap<BindingId, BindingInfo>,
    provisional_to_final: HashMap<ProvisionalBindingId, BindingId>,
    resolutions: BTreeMap<ExpressionKey, BindingId>,
    diagnostics: HashMap<DocumentId, Vec<Diagnostic>>,
    global_bindings: BTreeMap<Symbol, BindingId>,
    types: BTreeMap<Symbol, TypeInfo>,
    next_provisional_binding_id: u32,
    next_binding_id: u32,
}

impl<'a> PackageNamingContext<'a> {
    fn new(interner: &'a Interner, modules: &[(DocumentId, &'a Module)]) -> Self {
        Self {
            interner,
            modules: modules.iter().copied().collect(),
            documents: BTreeMap::new(),
            provisional_bindings: BTreeMap::new(),
            bindings: BTreeMap::new(),
            provisional_to_final: HashMap::new(),
            resolutions: BTreeMap::new(),
            diagnostics: HashMap::new(),
            global_bindings: BTreeMap::new(),
            types: BTreeMap::new(),
            next_provisional_binding_id: 0,
            next_binding_id: 0,
        }
    }

    fn finish(self) -> PackageNamingComputation {
        PackageNamingComputation {
            locals: self.documents.into_iter().collect(),
            naming: NamesGlobal {
                bindings: self.bindings,
                global_bindings: self.global_bindings,
                resolutions: self.resolutions,
            },
            diagnostics: self.diagnostics,
        }
    }

    fn resolve_document(&mut self, document_id: DocumentId, module: &Module) {
        let document_naming = DocumentNamingContext::new(
            document_id,
            &module.arena,
            &mut self.next_provisional_binding_id,
            &mut self.provisional_bindings,
        )
        .resolve_module(module);
        self.documents.insert(document_id, document_naming);
    }

    fn record_local_resolutions(&mut self) {
        let documents = self
            .documents
            .iter()
            .map(|(document_id, document_naming)| {
                (*document_id, document_naming.expression_resolutions.clone())
            })
            .collect::<Vec<_>>();

        for (document_id, expression_resolutions) in documents {
            for (expression_id, provisional_binding_id) in expression_resolutions {
                let binding_id = self.finalize_binding(provisional_binding_id);
                self.resolutions.insert(
                    ExpressionKey {
                        module_id: document_id,
                        expression_id,
                    },
                    binding_id,
                );
            }
        }
    }

    fn collect_types(&mut self, modules: &[(DocumentId, &Module)]) {
        let mut definitions_by_symbol = BTreeMap::<Symbol, Vec<TypeDefinitionSite>>::new();

        for (document_id, module) in modules {
            for definition in &module.definitions {
                definitions_by_symbol
                    .entry(definition.definition.name)
                    .or_default()
                    .push(TypeDefinitionSite {
                        document_id: *document_id,
                        range: definition.range,
                        kind: definition.definition.kind,
                        arity: definition.definition.type_parameters.len(),
                    });
            }
        }

        for (symbol, definition_sites) in definitions_by_symbol {
            if definition_sites.len() == 1 {
                let definition_site = definition_sites
                    .into_iter()
                    .next()
                    .expect("single-site type definition should exist");
                self.types.insert(
                    symbol,
                    TypeInfo {
                        kind: definition_site.kind,
                        arity: definition_site.arity,
                    },
                );
                continue;
            }

            for definition_site in definition_sites {
                self.push_duplicate_type_definition_diagnostic(
                    symbol,
                    definition_site.range,
                    definition_site.document_id,
                );
            }
        }
    }

    fn build_global_bindings(&mut self, modules: &[(DocumentId, &Module)]) {
        for (document_id, _) in modules {
            let Some(document_naming) = self.documents.get(document_id).cloned() else {
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

    fn resolve_definitions(&mut self, modules: &[(DocumentId, &Module)]) {
        for (document_id, module) in modules {
            for definition in &module.definitions {
                self.resolve_definition(definition, *document_id);
            }
        }
    }

    fn resolve_annotations(&mut self, modules: &[(DocumentId, &Module)]) {
        for (document_id, module) in modules {
            let Some(document_naming) = self.documents.get(document_id).cloned() else {
                continue;
            };

            for expression_id in document_naming.annotated_expressions {
                let expression = module.arena.get(expression_id);
                if let Some(annotation) = &expression.annotation {
                    self.resolve_annotation(annotation, *document_id);
                }
            }
        }
    }

    fn resolve_unresolved_values(&mut self) {
        let documents = self
            .documents
            .iter()
            .map(|(document_id, document_naming)| {
                (*document_id, document_naming.unresolved_values.clone())
            })
            .collect::<Vec<_>>();

        for (document_id, unresolved_values) in documents {
            for (expression_id, symbol) in unresolved_values {
                if let Some(binding_id) = self.global_bindings.get(&symbol) {
                    self.resolutions.insert(
                        ExpressionKey {
                            module_id: document_id,
                            expression_id,
                        },
                        *binding_id,
                    );
                    continue;
                }

                if !self.is_namespace_symbol(symbol, document_id) && !self.is_builtin_symbol(symbol)
                {
                    let name = self.interner.resolve(symbol).unwrap_or("<unknown>");
                    let range = self.module_expression_range(document_id, expression_id);
                    self.push_diagnostic(
                        document_id,
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

    fn resolve_definition(&mut self, definition_item: &DefinitionItem, document_id: DocumentId) {
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
            document_id,
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

    fn resolve_annotation(&mut self, annotation: &AttachedAnnotation, document_id: DocumentId) {
        match annotation.annotation() {
            Annotation::Type { surface_type, .. } => {
                self.resolve_surface_type(
                    surface_type,
                    &BTreeSet::new(),
                    annotation.range(),
                    document_id,
                );
            }
            Annotation::New { nominal_type } => {
                self.resolve_nominal_type_ref(nominal_type, annotation.range(), document_id);
            }
        }
    }

    fn resolve_nominal_type_ref(
        &mut self,
        nominal_type: &NamedTypeRef,
        range: Range,
        document_id: DocumentId,
    ) {
        match self.types.get(&nominal_type.name) {
            Some(type_info) => {
                let kind = type_info.kind;
                let arity = type_info.arity;
                self.push_type_argument_arity_diagnostic(
                    nominal_type.name,
                    arity,
                    nominal_type.type_arguments.len(),
                    range,
                    document_id,
                );
                if kind == DefinitionKind::Type {
                    for type_argument in &nominal_type.type_arguments {
                        self.resolve_surface_type(
                            type_argument,
                            &BTreeSet::new(),
                            range,
                            document_id,
                        );
                    }
                    return;
                }

                let name = self
                    .render_type_name(nominal_type.name)
                    .unwrap_or_else(|| "<unknown>".to_owned());
                self.push_diagnostic(
                    document_id,
                    Diagnostic::syntax_error(
                        range,
                        format!(
                            "invalid semantics: `@new` requires a nominal type declared with `@type`, but `{name}` is an alias."
                        ),
                    ),
                );
            }
            None => {
                self.push_unknown_type_diagnostic(nominal_type.name, range, document_id);
            }
        }

        for type_argument in &nominal_type.type_arguments {
            self.resolve_surface_type(type_argument, &BTreeSet::new(), range, document_id);
        }
    }

    fn resolve_surface_type(
        &mut self,
        surface_type: &SurfaceType,
        local_type_parameters: &BTreeSet<Symbol>,
        range: Range,
        document_id: DocumentId,
    ) {
        match surface_type {
            SurfaceType::Named(name, arguments) => {
                if local_type_parameters.contains(name) {
                    return;
                }

                if let Some(type_info) = self.types.get(name) {
                    let arity = type_info.arity;
                    self.push_type_argument_arity_diagnostic(
                        *name,
                        arity,
                        arguments.len(),
                        range,
                        document_id,
                    );
                } else {
                    self.push_unknown_type_diagnostic(*name, range, document_id);
                }

                for argument in arguments {
                    self.resolve_surface_type(argument, local_type_parameters, range, document_id);
                }
            }
            SurfaceType::Nullable(inner_type)
            | SurfaceType::Vector(inner_type)
            | SurfaceType::NamedVector(inner_type)
            | SurfaceType::List(inner_type)
            | SurfaceType::NamedList(inner_type) => {
                self.resolve_surface_type(inner_type, local_type_parameters, range, document_id);
            }
            SurfaceType::Record(fields) => {
                for field in fields {
                    self.resolve_surface_type(
                        &field.value,
                        local_type_parameters,
                        range,
                        document_id,
                    );
                }
            }
            SurfaceType::Tuple(items) => {
                for item in items {
                    self.resolve_surface_type(item, local_type_parameters, range, document_id);
                }
            }
            SurfaceType::Function(function_type) => {
                for parameter in &function_type.parameters {
                    self.resolve_surface_type(parameter, local_type_parameters, range, document_id);
                }
                for parameter in &function_type.named_parameters {
                    self.resolve_surface_type(
                        &parameter.value,
                        local_type_parameters,
                        range,
                        document_id,
                    );
                }
                self.resolve_surface_type(
                    &function_type.return_type,
                    local_type_parameters,
                    range,
                    document_id,
                );
            }
            SurfaceType::Binders(type_parameters, inner_type) => {
                let mut nested_type_parameters = local_type_parameters.clone();
                nested_type_parameters.extend(type_parameters.iter().copied());
                self.resolve_surface_type(inner_type, &nested_type_parameters, range, document_id);
            }
            SurfaceType::Any
            | SurfaceType::Unknown
            | SurfaceType::Null
            | SurfaceType::Scalar(_) => {}
        }
    }

    fn push_unknown_type_diagnostic(
        &mut self,
        symbol: Symbol,
        range: Range,
        document_id: DocumentId,
    ) {
        let name = self
            .render_type_name(symbol)
            .unwrap_or_else(|| "<unknown>".to_owned());
        self.push_diagnostic(
            document_id,
            Diagnostic::syntax_error(range, format!("type syntax error: unknown type `{name}`")),
        );
    }

    fn push_duplicate_type_definition_diagnostic(
        &mut self,
        symbol: Symbol,
        range: Range,
        document_id: DocumentId,
    ) {
        let name = self
            .render_type_name(symbol)
            .unwrap_or_else(|| "<unknown>".to_owned());
        self.push_diagnostic(
            document_id,
            Diagnostic::syntax_error(
                range,
                format!(
                    "invalid semantics: type name `{name}` is already defined by another top-level @type or @alias declaration in this package."
                ),
            ),
        );
    }

    fn push_type_argument_arity_diagnostic(
        &mut self,
        symbol: Symbol,
        expected: usize,
        found: usize,
        range: Range,
        document_id: DocumentId,
    ) {
        if expected == found {
            return;
        }

        let name = self
            .render_type_name(symbol)
            .unwrap_or_else(|| "<unknown>".to_owned());
        let message = if expected == 0 {
            format!("type `{name}` does not take type arguments, but found {found}.")
        } else {
            format!("generic type `{name}` expects {expected} type argument(s), but found {found}.")
        };
        self.push_diagnostic(document_id, Diagnostic::syntax_error(range, message));
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

    fn module_expression_range(
        &self,
        document_id: DocumentId,
        expression_id: ExpressionId,
    ) -> Range {
        self.modules
            .get(&document_id)
            .expect("module should exist")
            .arena
            .get(expression_id)
            .range
    }

    fn is_namespace_symbol(&self, _symbol: Symbol, _document_id: DocumentId) -> bool {
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

    fn push_diagnostic(&mut self, document_id: DocumentId, diagnostic: Diagnostic) {
        self.diagnostics
            .entry(document_id)
            .or_default()
            .push(diagnostic);
    }
}

struct DocumentNamingContext<'a> {
    document_id: DocumentId,
    arena: &'a HirArena,
    next_provisional_binding_id: &'a mut u32,
    provisional_bindings: &'a mut BTreeMap<ProvisionalBindingId, ProvisionalBindingInfo>,
    local_scopes: Vec<BTreeMap<Symbol, ProvisionalBindingId>>,
    document_naming: NamesLocal,
}

impl<'a> DocumentNamingContext<'a> {
    fn new(
        document_id: DocumentId,
        arena: &'a HirArena,
        next_provisional_binding_id: &'a mut u32,
        provisional_bindings: &'a mut BTreeMap<ProvisionalBindingId, ProvisionalBindingInfo>,
    ) -> Self {
        Self {
            document_id,
            arena,
            next_provisional_binding_id,
            provisional_bindings,
            local_scopes: Vec::new(),
            document_naming: NamesLocal::default(),
        }
    }

    fn resolve_module(mut self, module: &Module) -> NamesLocal {
        for expression_id in &module.expressions {
            self.resolve_expression(*expression_id);
        }

        self.document_naming
    }

    fn resolve_expression(&mut self, expression_id: ExpressionId) {
        let expression = self.arena.get(expression_id);
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
        let binding_info = ProvisionalBindingInfo {
            module_id: self.document_id,
            symbol,
            range,
        };
        self.provisional_bindings
            .insert(binding_id, binding_info.clone());
        self.document_naming
            .bindings
            .insert(binding_id, binding_info);
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionalBindingInfo {
    pub module_id: ModuleId,
    pub symbol: Symbol,
    pub range: Range,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProvisionalBindingId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TypeInfo {
    kind: DefinitionKind,
    arity: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TypeDefinitionSite {
    document_id: DocumentId,
    range: Range,
    kind: DefinitionKind,
    arity: usize,
}
