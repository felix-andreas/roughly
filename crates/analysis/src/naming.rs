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
    pub global_bindings: BTreeMap<Symbol, DocumentId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NamesLocal {
    pub bindings: BTreeMap<BindingId, BindingInfo>,
    pub expression_resolutions: BTreeMap<ExpressionId, BindingId>,
    pub global_exports: BTreeMap<Symbol, BindingId>,
    pub non_locals: BTreeMap<ExpressionId, Symbol>,
    pub named_type_annotations: Vec<ExpressionId>,
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PackageNamingComputation {
    pub naming: NamesGlobal,
    pub diagnostics: HashMap<DocumentId, Vec<Diagnostic>>,
}

pub(crate) fn rebuild_package_naming(
    package_modules: &[(DocumentId, &Module)],
    extra_modules: &[(DocumentId, &Module)],
    locals: &HashMap<DocumentId, NamesLocal>,
    interner: &Interner,
) -> PackageNamingComputation {
    let all_modules = package_modules
        .iter()
        .chain(extra_modules.iter())
        .copied()
        .collect::<Vec<_>>();
    let mut diagnostics = HashMap::<DocumentId, Vec<Diagnostic>>::new();
    let types = build_type_index(package_modules, interner, &mut diagnostics);
    let global_bindings =
        build_global_bindings(package_modules, locals, interner, &mut diagnostics);

    {
        let mut type_resolver = TypeResolver {
            interner,
            types: &types,
            diagnostics: &mut diagnostics,
        };

        for (document_id, module) in &all_modules {
            for definition in &module.definitions {
                type_resolver.resolve_definition(definition, *document_id);
            }

            let local_naming = locals.get(document_id).unwrap_or_else(|| {
                panic!("missing local naming for module {document_id:?} during package rebuild")
            });
            for expression_id in &local_naming.named_type_annotations {
                let annotation = module
                    .arena
                    .get(*expression_id)
                    .annotation
                    .as_ref()
                    .unwrap_or_else(|| {
                        panic!("named type annotation should exist for {document_id:?}:{expression_id:?}")
                    });
                type_resolver.resolve_annotation(annotation, *document_id);
            }
        }
    }

    for (document_id, module) in &all_modules {
        let local_naming = locals.get(document_id).unwrap_or_else(|| {
            panic!("missing local naming for module {document_id:?} during package rebuild")
        });

        for (expression_id, symbol) in &local_naming.non_locals {
            if global_bindings.contains_key(symbol)
                || is_namespace_symbol(*symbol, *document_id)
                || is_builtin_symbol(interner, *symbol)
            {
                continue;
            }

            let name = interner.resolve(*symbol).unwrap_or("<unknown>");
            let range = module.arena.get(*expression_id).range;
            push_diagnostic(
                &mut diagnostics,
                *document_id,
                Diagnostic::naming_warning(
                    range,
                    format!(
                        "I could not resolve `{name}` in this package, its imports, or builtins."
                    ),
                ),
            );
        }
    }

    PackageNamingComputation {
        naming: NamesGlobal { global_bindings },
        diagnostics,
    }
}

pub fn resolve_document_locally(document_id: DocumentId, module: &Module) -> NamesLocal {
    DocumentNamingContext::new(document_id, &module.arena).resolve_module(module)
}

fn build_type_index(
    package_modules: &[(DocumentId, &Module)],
    interner: &Interner,
    diagnostics: &mut HashMap<DocumentId, Vec<Diagnostic>>,
) -> BTreeMap<Symbol, TypeInfo> {
    let mut definitions_by_symbol = BTreeMap::<Symbol, Vec<TypeDefinitionSite>>::new();

    for (document_id, module) in package_modules {
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

    let mut types = BTreeMap::new();
    for (symbol, definition_sites) in definitions_by_symbol {
        if definition_sites.len() == 1 {
            let definition_site = definition_sites
                .into_iter()
                .next()
                .expect("single-site type definition should exist");
            types.insert(
                symbol,
                TypeInfo {
                    kind: definition_site.kind,
                    arity: definition_site.arity,
                },
            );
            continue;
        }

        let name = interner.resolve(symbol).unwrap_or("<unknown>").to_owned();
        for definition_site in definition_sites {
            push_diagnostic(
                diagnostics,
                definition_site.document_id,
                Diagnostic::syntax_error(
                    definition_site.range,
                    format!(
                        "invalid semantics: type name `{name}` is already defined by another top-level @type or @alias declaration in this package."
                    ),
                ),
            );
        }
    }

    types
}

fn build_global_bindings(
    package_modules: &[(DocumentId, &Module)],
    locals: &HashMap<DocumentId, NamesLocal>,
    interner: &Interner,
    diagnostics: &mut HashMap<DocumentId, Vec<Diagnostic>>,
) -> BTreeMap<Symbol, DocumentId> {
    let mut global_bindings = BTreeMap::<Symbol, DocumentId>::new();
    let mut winning_bindings = BTreeMap::<Symbol, BindingInfo>::new();

    for (document_id, module) in package_modules {
        let local_naming = locals.get(document_id).unwrap_or_else(|| {
            panic!("missing local naming for package module {document_id:?} during package rebuild")
        });

        for expression_id in &module.expressions {
            let expression = module.arena.get(*expression_id);
            let ExpressionKind::Assign { target, .. } = expression.kind else {
                continue;
            };

            let binding_id = local_naming
                .expression_resolutions
                .get(expression_id)
                .copied()
                .unwrap_or_else(|| {
                    panic!(
                        "top-level assignment should have local binding resolution for {document_id:?}:{expression_id:?}"
                    )
                });
            let binding = local_naming
                .bindings
                .get(&binding_id)
                .unwrap_or_else(|| {
                    panic!("top-level binding should exist for {document_id:?}:{binding_id:?}")
                })
                .clone();

            if is_namespace_symbol(target, *document_id) {
                let name = interner.resolve(target).unwrap_or("<unknown>");
                push_diagnostic(
                    diagnostics,
                    *document_id,
                    Diagnostic::naming_warning(
                        binding.range,
                        format!("Top-level binding `{name}` shadows an imported namespace symbol."),
                    ),
                );
            }

            if is_builtin_symbol(interner, target) {
                let name = interner.resolve(target).unwrap_or("<unknown>");
                push_diagnostic(
                    diagnostics,
                    *document_id,
                    Diagnostic::naming_warning(
                        binding.range,
                        format!("Top-level binding `{name}` shadows a builtin."),
                    ),
                );
            }

            if let Some(previous_binding) = winning_bindings.insert(target, binding.clone()) {
                let name = interner.resolve(target).unwrap_or("<unknown>").to_owned();
                push_diagnostic(
                    diagnostics,
                    previous_binding.module_id,
                    Diagnostic::naming_warning(
                        previous_binding.range,
                        format!(
                            "Top-level binding `{name}` is overwritten by a later top-level binding in this package."
                        ),
                    ),
                );
                push_diagnostic(
                    diagnostics,
                    binding.module_id,
                    Diagnostic::naming_warning(
                        binding.range,
                        format!(
                            "Top-level binding `{name}` overwrites an earlier top-level binding in this package."
                        ),
                    ),
                );
            }

            global_bindings.insert(target, *document_id);
        }
    }

    global_bindings
}

fn push_diagnostic(
    diagnostics: &mut HashMap<DocumentId, Vec<Diagnostic>>,
    document_id: DocumentId,
    diagnostic: Diagnostic,
) {
    diagnostics.entry(document_id).or_default().push(diagnostic);
}

fn annotation_contains_named_type(annotation: &AttachedAnnotation) -> bool {
    match annotation.annotation() {
        Annotation::Type { surface_type, .. } => surface_type_contains_named_type(surface_type),
        Annotation::New { .. } => true,
    }
}

fn surface_type_contains_named_type(surface_type: &SurfaceType) -> bool {
    match surface_type {
        SurfaceType::Named(_, _) => true,
        SurfaceType::Nullable(inner_type)
        | SurfaceType::Vector(inner_type)
        | SurfaceType::NamedVector(inner_type)
        | SurfaceType::List(inner_type)
        | SurfaceType::NamedList(inner_type) => surface_type_contains_named_type(inner_type),
        SurfaceType::Record(fields) => fields
            .iter()
            .any(|field| surface_type_contains_named_type(&field.value)),
        SurfaceType::Tuple(items) => items.iter().any(surface_type_contains_named_type),
        SurfaceType::Function(function_type) => {
            function_type
                .parameters
                .iter()
                .any(surface_type_contains_named_type)
                || function_type
                    .named_parameters
                    .iter()
                    .any(|parameter| surface_type_contains_named_type(&parameter.value))
                || surface_type_contains_named_type(&function_type.return_type)
        }
        SurfaceType::Binders(_, inner_type) => surface_type_contains_named_type(inner_type),
        SurfaceType::Any | SurfaceType::Unknown | SurfaceType::Null | SurfaceType::Scalar(_) => {
            false
        }
    }
}

fn is_namespace_symbol(_symbol: Symbol, _document_id: DocumentId) -> bool {
    false
}

fn is_builtin_symbol(interner: &Interner, symbol: Symbol) -> bool {
    matches!(
        interner.resolve(symbol),
        Some("+" | "-" | "*" | "/" | "**" | "&&" | "||" | "c" | "list")
    )
}

struct TypeResolver<'a> {
    interner: &'a Interner,
    types: &'a BTreeMap<Symbol, TypeInfo>,
    diagnostics: &'a mut HashMap<DocumentId, Vec<Diagnostic>>,
}

impl<'a> TypeResolver<'a> {
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
                self.push_type_argument_arity_diagnostic(
                    nominal_type.name,
                    type_info.arity,
                    nominal_type.type_arguments.len(),
                    range,
                    document_id,
                );

                if type_info.kind == DefinitionKind::Type {
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
                    .interner
                    .resolve(nominal_type.name)
                    .unwrap_or("<unknown>")
                    .to_owned();
                push_diagnostic(
                    self.diagnostics,
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
                    self.push_type_argument_arity_diagnostic(
                        *name,
                        type_info.arity,
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
                if type_parameters.is_empty() {
                    self.resolve_surface_type(
                        inner_type,
                        local_type_parameters,
                        range,
                        document_id,
                    );
                    return;
                }

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
            .interner
            .resolve(symbol)
            .unwrap_or("<unknown>")
            .to_owned();
        push_diagnostic(
            self.diagnostics,
            document_id,
            Diagnostic::syntax_error(range, format!("type syntax error: unknown type `{name}`")),
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
            .interner
            .resolve(symbol)
            .unwrap_or("<unknown>")
            .to_owned();
        let message = if expected == 0 {
            format!("type `{name}` does not take type arguments, but found {found}.")
        } else {
            format!("generic type `{name}` expects {expected} type argument(s), but found {found}.")
        };
        push_diagnostic(
            self.diagnostics,
            document_id,
            Diagnostic::syntax_error(range, message),
        );
    }
}

struct DocumentNamingContext<'a> {
    document_id: DocumentId,
    arena: &'a HirArena,
    next_binding_id: u32,
    local_scopes: Vec<BTreeMap<Symbol, BindingId>>,
    document_naming: NamesLocal,
}

impl<'a> DocumentNamingContext<'a> {
    fn new(document_id: DocumentId, arena: &'a HirArena) -> Self {
        Self {
            document_id,
            arena,
            next_binding_id: 0,
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
        if let Some(annotation) = &expression.annotation
            && annotation_contains_named_type(annotation)
        {
            self.document_naming
                .named_type_annotations
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
                        .non_locals
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
                    self.document_naming
                        .global_exports
                        .insert(*target, binding_id);
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

    fn fresh_binding(&mut self, symbol: Symbol, range: Range) -> BindingId {
        let binding_id = BindingId(self.next_binding_id);
        self.next_binding_id += 1;
        self.document_naming.bindings.insert(
            binding_id,
            BindingInfo {
                id: binding_id,
                module_id: self.document_id,
                symbol,
                range,
            },
        );
        binding_id
    }

    fn resolve_local_symbol(&self, symbol: Symbol) -> Option<BindingId> {
        for scope in self.local_scopes.iter().rev() {
            if let Some(binding_id) = scope.get(&symbol) {
                return Some(*binding_id);
            }
        }
        None
    }
}

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
