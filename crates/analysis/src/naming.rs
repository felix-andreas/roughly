use {
    crate::{
        diagnostic::Diagnostic,
        document::DocumentId,
        hir::{
            DefinitionItem, DefinitionKind, ExpressionId, ExpressionKind, HirArena, Module,
            ModuleId,
        },
        interner::{Interner, Symbol},
        stdlib::StubLibrary,
        type_syntax::type_name_token_range,
        types::{Annotation, AttachedAnnotation, NamedTypeRef, SurfaceType},
    },
    ropey::Rope,
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
    pub maybe_undefined_expressions: BTreeSet<ExpressionId>,
    pub non_locals: BTreeMap<ExpressionId, Symbol>,
    pub named_type_annotations: Vec<ExpressionId>,
    // Local (function-scoped) assignments whose value is never read. Computed once after resolution;
    // surfaced as diagnostics only when the `unused` check is enabled.
    pub unused_bindings: Vec<BindingId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingInfo {
    pub id: BindingId,
    pub module_id: ModuleId,
    pub symbol: Symbol,
    pub range: Range,
    pub kind: BindingKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    // A top-level `name <- value`. Treated as package-visible, so it is never reported as unused.
    TopLevelAssignment,
    // A `name <- value` inside a function or block. The only kind reported as unused.
    LocalAssignment,
    // A function parameter. R does not treat unused parameters as errors, so they are never reported.
    Parameter,
    // A `for (variable in ...)` loop variable. Often intentionally unused, so it is never reported.
    ForVariable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BindingId(pub u32);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocumentNamingComputation {
    pub naming: NamesLocal,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PackageNamingComputation {
    pub naming: NamesGlobal,
    pub diagnostics: HashMap<DocumentId, Vec<Diagnostic>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TypeInfo {
    kind: DefinitionKind,
    arity: usize,
}

// The from-scratch package-naming pass: `analysis`'s sole path, computing `global_bindings` and every
// package-document naming diagnostic in one fold over the current modules. The engine crate provides
// incrementality; `analysis` is the from-scratch checker behind `run_full` (the CLI batch check) and the
// engine differential oracle, so this rebuild is the production path here, not a debug-only oracle.
pub(crate) fn rebuild_package_naming(
    package_modules: &[(DocumentId, &Module)],
    extra_modules: &[(DocumentId, &Module)],
    locals: &HashMap<DocumentId, NamesLocal>,
    ropes: &HashMap<DocumentId, Rope>,
    interner: &Interner,
    stub_library: &StubLibrary,
) -> PackageNamingComputation {
    let (types, duplicate_type_names) = build_type_index(package_modules);
    let candidate_order = build_candidate_order(package_modules, locals);
    let global_bindings = winners_from_candidate_order(&candidate_order);

    let mut diagnostics = HashMap::<DocumentId, Vec<Diagnostic>>::new();
    for (document_id, module) in package_modules {
        let local_naming = expect_local_naming(locals, *document_id);
        let rope = expect_rope(ropes, *document_id);
        let document_diagnostics =
            package_document_diagnostics(&PackageDocumentDiagnosticContext {
                document_id: *document_id,
                module,
                rope,
                local_naming,
                is_script: false,
                types: &types,
                duplicate_type_names: &duplicate_type_names,
                global_bindings: &global_bindings,
                candidate_order: &candidate_order,
                interner,
                stub_library,
            });
        if !document_diagnostics.is_empty() {
            diagnostics.insert(*document_id, document_diagnostics);
        }
    }
    // Scripts additionally see their own type declarations, which shadow package definitions of the
    // same name, so a script can declare and use a nominal/alias that the package does not define.
    for (document_id, module) in extra_modules {
        let local_naming = expect_local_naming(locals, *document_id);
        let rope = expect_rope(ropes, *document_id);
        let script_types = build_script_local_type_index(&types, module);
        let document_diagnostics =
            package_document_diagnostics(&PackageDocumentDiagnosticContext {
                document_id: *document_id,
                module,
                rope,
                local_naming,
                is_script: true,
                types: &script_types,
                duplicate_type_names: &duplicate_type_names,
                global_bindings: &global_bindings,
                candidate_order: &candidate_order,
                interner,
                stub_library,
            });
        if !document_diagnostics.is_empty() {
            diagnostics.insert(*document_id, document_diagnostics);
        }
    }

    PackageNamingComputation {
        naming: NamesGlobal { global_bindings },
        diagnostics,
    }
}

// All package-naming diagnostics a single document contributes, in a fixed category order:
// (T) duplicate-type-definition, (B) builtin/namespace shadow, (A) overwrite pairs, (C) type-reference
// resolution, (D) unresolved reference. A pure function of the document's naming plus the package-level
// inputs it reads (the type index, the global bindings it resolves references against, and the
// path-ordered candidate list it takes its overwrite position from). Used both by the rebuild oracle
// above (over all documents) and by incremental maintenance (over the affected documents only).
pub(crate) struct PackageDocumentDiagnosticContext<'a> {
    pub document_id: DocumentId,
    pub module: &'a Module,
    // The document's text, used to narrow a type-reference diagnostic from the whole annotation down to
    // the offending type name by re-lexing the `#:` notation.
    pub rope: &'a Rope,
    pub local_naming: &'a NamesLocal,
    pub is_script: bool,
    pub types: &'a BTreeMap<Symbol, TypeInfo>,
    pub duplicate_type_names: &'a BTreeSet<Symbol>,
    pub global_bindings: &'a BTreeMap<Symbol, DocumentId>,
    pub candidate_order: &'a BTreeMap<Symbol, Vec<DocumentId>>,
    pub interner: &'a Interner,
    // The stdlib stub corpus (base-environment names). A reference to a stub name resolves to its
    // base scheme, so it is neither an unresolved reference nor a shadow-free target; a binding over
    // one reports the builtin-shadow warning, exactly as for the hardcoded builtins.
    pub stub_library: &'a StubLibrary,
}

pub(crate) fn package_document_diagnostics(
    context: &PackageDocumentDiagnosticContext<'_>,
) -> Vec<Diagnostic> {
    let interner = context.interner;
    let mut diagnostics = Vec::new();

    // (A)/(B)/(T) apply only to package documents. Scripts define no package globals and their type
    // declarations are local, so they contribute only (C) and (D).
    if !context.is_script {
        for definition in &context.module.definitions {
            if context
                .duplicate_type_names
                .contains(&definition.definition.name)
            {
                let name = interner
                    .resolve(definition.definition.name)
                    .unwrap_or("<unknown>");
                diagnostics.push(Diagnostic::syntax_error(
                    definition.range,
                    format!(
                        "invalid semantics: type name `{name}` is already defined by another top-level @type or @alias declaration in this package."
                    ),
                ));
            }
        }

        // Overwrite warnings are per top-level *assignment*, not per document: `x <- 1; x <- 2` in one
        // file overwrites within that file. So a name's earlier/later neighbours can be in a path-earlier
        // or path-later document (its position in the candidate order) *or* an earlier/later assignment
        // to the same name in this document. The two are combined below.
        let mut occurrence_totals = BTreeMap::<Symbol, usize>::new();
        for expression_id in &context.module.expressions {
            if let ExpressionKind::Assign { target, .. } =
                context.module.arena.get(*expression_id).kind
                && top_level_binding(context.local_naming, *expression_id).is_some()
            {
                *occurrence_totals.entry(target).or_default() += 1;
            }
        }

        let mut occurrences_seen = BTreeMap::<Symbol, usize>::new();
        for expression_id in &context.module.expressions {
            let ExpressionKind::Assign { target, .. } =
                context.module.arena.get(*expression_id).kind
            else {
                continue;
            };
            let Some(binding) = top_level_binding(context.local_naming, *expression_id) else {
                continue;
            };

            if is_namespace_symbol(target, context.document_id) {
                let name = interner.resolve(target).unwrap_or("<unknown>");
                diagnostics.push(Diagnostic::naming_warning(
                    binding.range,
                    format!("Top-level binding `{name}` shadows an imported namespace symbol."),
                ));
            }
            if is_builtin_symbol(interner, target) || context.stub_library.contains(target) {
                let name = interner.resolve(target).unwrap_or("<unknown>");
                diagnostics.push(Diagnostic::naming_warning(
                    binding.range,
                    format!("Top-level binding `{name}` shadows a builtin."),
                ));
            }

            let (document_position, candidate_count) = match context.candidate_order.get(&target) {
                Some(candidates) => (
                    candidates
                        .iter()
                        .position(|candidate| *candidate == context.document_id)
                        .unwrap_or(0),
                    candidates.len(),
                ),
                None => (0, 1),
            };
            let occurrence_index = *occurrences_seen.entry(target).or_default();
            *occurrences_seen
                .get_mut(&target)
                .expect("occurrence counter exists") += 1;
            let occurrences_in_document = occurrence_totals.get(&target).copied().unwrap_or(1);
            let has_earlier = document_position > 0 || occurrence_index > 0;
            let has_later = document_position + 1 < candidate_count
                || occurrence_index + 1 < occurrences_in_document;

            if has_earlier {
                let name = interner.resolve(target).unwrap_or("<unknown>");
                diagnostics.push(Diagnostic::naming_warning(
                    binding.range,
                    format!(
                        "Top-level binding `{name}` overwrites an earlier top-level binding in this package."
                    ),
                ));
            }
            if has_later {
                let name = interner.resolve(target).unwrap_or("<unknown>");
                diagnostics.push(Diagnostic::naming_warning(
                    binding.range,
                    format!(
                        "Top-level binding `{name}` is overwritten by a later top-level binding in this package."
                    ),
                ));
            }
        }
    }

    let mut type_diagnostics = HashMap::<DocumentId, Vec<Diagnostic>>::new();
    {
        let mut type_resolver = TypeResolver {
            interner,
            rope: context.rope,
            types: context.types,
            diagnostics: &mut type_diagnostics,
        };
        resolve_module_type_references(
            &mut type_resolver,
            context.document_id,
            context.module,
            context.local_naming,
        );
    }
    if let Some(entries) = type_diagnostics.remove(&context.document_id) {
        diagnostics.extend(entries);
    }

    for (expression_id, symbol) in &context.local_naming.non_locals {
        if context.global_bindings.contains_key(symbol)
            || is_namespace_symbol(*symbol, context.document_id)
            || is_builtin_symbol(interner, *symbol)
            || context.stub_library.contains(*symbol)
        {
            continue;
        }
        let name = interner.resolve(*symbol).unwrap_or("<unknown>");
        let range = context.module.arena.get(*expression_id).range;
        diagnostics.push(Diagnostic::naming_warning(
            range,
            format!("I could not resolve `{name}` in this package, its imports, or builtins."),
        ));
    }

    diagnostics
}

// The path-ordered candidate list per package-global name: the documents whose top-level assignments
// bind the name, in `package_modules` order (package path order). `Winner(N)` is the last entry. Built
// in a single pass; `global_bindings` is derived from it as the path-last candidate.
pub(crate) fn build_candidate_order(
    package_modules: &[(DocumentId, &Module)],
    locals: &HashMap<DocumentId, NamesLocal>,
) -> BTreeMap<Symbol, Vec<DocumentId>> {
    let mut candidate_order = BTreeMap::<Symbol, Vec<DocumentId>>::new();
    for (document_id, module) in package_modules {
        let local_naming = expect_local_naming(locals, *document_id);
        // `document_package_definitions` is the single shared membership rule; each target appears
        // once per document, so the document is pushed onto a symbol's candidate list at most once.
        for target in document_package_definitions(module, local_naming) {
            candidate_order
                .entry(target)
                .or_default()
                .push(*document_id);
        }
    }
    candidate_order
}

// The package-global names a single document defines. The membership rule, shared with
// `build_candidate_order` so the candidate index and the rebuild oracle can never disagree: a target
// counts when it is the target of a top-level `Assign` that resolves to a top-level binding.
//
// A bare top-level `{ }` block executes unconditionally, so its direct-child assigns are package
// globals too; we recurse into bare `Block` expressions (including nested bare blocks). We do *not*
// recurse into `if`/`for`/`while`/function bodies — those are `If`/`For`/`While`/`Function`
// expressions, not bare `Block`s, so a conditionally-executed assign such as `if (TRUE) { x <- 1 }`
// stays excluded even though it leaves a `TopLevelAssignment`-kinded entry in `bindings`.
pub(crate) fn document_package_definitions(
    module: &Module,
    local_naming: &NamesLocal,
) -> BTreeSet<Symbol> {
    let mut symbols = BTreeSet::new();
    collect_package_definitions(module, local_naming, &module.expressions, &mut symbols);
    symbols
}

fn collect_package_definitions(
    module: &Module,
    local_naming: &NamesLocal,
    expressions: &[ExpressionId],
    symbols: &mut BTreeSet<Symbol>,
) {
    for expression_id in expressions {
        match &module.arena.get(*expression_id).kind {
            ExpressionKind::Assign { target, .. } => {
                if top_level_binding(local_naming, *expression_id).is_some() {
                    symbols.insert(*target);
                }
            }
            ExpressionKind::Block { expressions, .. } => {
                collect_package_definitions(module, local_naming, expressions, symbols);
            }
            _ => {}
        }
    }
}

pub(crate) fn winners_from_candidate_order(
    candidate_order: &BTreeMap<Symbol, Vec<DocumentId>>,
) -> BTreeMap<Symbol, DocumentId> {
    candidate_order
        .iter()
        .filter_map(|(symbol, candidates)| candidates.last().map(|winner| (*symbol, *winner)))
        .collect()
}

fn top_level_binding(
    local_naming: &NamesLocal,
    expression_id: ExpressionId,
) -> Option<&BindingInfo> {
    let binding_id = local_naming
        .expression_resolutions
        .get(&expression_id)
        .copied()?;
    local_naming.bindings.get(&binding_id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Package,
    Script,
}

pub fn resolve_document_locally(
    document_id: DocumentId,
    module: &Module,
    interner: &Interner,
    document_kind: DocumentKind,
) -> DocumentNamingComputation {
    let mut context = DocumentNamingContext::new(document_id, &module.arena, interner);
    // A script executes top-down like one big function body, so its top level is an
    // ordinary sequential lexical scope instead of the package-global namespace.
    if document_kind == DocumentKind::Script {
        context.local_scopes.push(BTreeMap::new());
    }
    context.resolve_module(module)
}

pub fn is_maybe_undefined_expression(
    local_naming: &NamesLocal,
    expression_id: ExpressionId,
) -> bool {
    local_naming
        .maybe_undefined_expressions
        .contains(&expression_id)
}

pub fn find_binding(
    local_naming: &NamesLocal,
    module_id: ModuleId,
    symbol: Symbol,
    range: Range,
) -> Option<BindingId> {
    local_naming
        .bindings
        .values()
        .find(|binding| {
            binding.module_id == module_id && binding.symbol == symbol && binding.range == range
        })
        .map(|binding| binding.id)
}

// The binding a package-global `symbol` resolves to inside its winning document: the last (source
// order) top-level `Assign` of `symbol`, recursing into bare top-level `{ }` blocks so a block-bound
// global is found. The bare-block recursion mirrors `document_package_definitions`, keeping the export
// lookup in step with the membership rule that admitted the global in the first place.
pub fn find_exported_binding(
    module: &Module,
    local_naming: &NamesLocal,
    symbol: Symbol,
) -> Option<BindingId> {
    find_exported_binding_in(module, local_naming, symbol, &module.expressions)
}

fn find_exported_binding_in(
    module: &Module,
    local_naming: &NamesLocal,
    symbol: Symbol,
    expressions: &[ExpressionId],
) -> Option<BindingId> {
    expressions.iter().rev().find_map(|expression_id| {
        match &module.arena.get(*expression_id).kind {
            ExpressionKind::Assign { target, .. } => (*target == symbol)
                .then(|| {
                    local_naming
                        .expression_resolutions
                        .get(expression_id)
                        .copied()
                })
                .flatten(),
            ExpressionKind::Block { expressions, .. } => {
                find_exported_binding_in(module, local_naming, symbol, expressions)
            }
            _ => None,
        }
    })
}

fn expect_local_naming(
    locals: &HashMap<DocumentId, NamesLocal>,
    document_id: DocumentId,
) -> &NamesLocal {
    locals.get(&document_id).unwrap_or_else(|| {
        panic!("missing local naming for module {document_id:?} during package rebuild")
    })
}

fn expect_rope(ropes: &HashMap<DocumentId, Rope>, document_id: DocumentId) -> &Rope {
    ropes
        .get(&document_id)
        .unwrap_or_else(|| panic!("missing rope for module {document_id:?} during package rebuild"))
}

fn resolve_module_type_references(
    type_resolver: &mut TypeResolver<'_>,
    document_id: DocumentId,
    module: &Module,
    local_naming: &NamesLocal,
) {
    for definition in &module.definitions {
        type_resolver.resolve_definition(definition, document_id);
    }
    for expression_id in &local_naming.named_type_annotations {
        let annotation = module
            .arena
            .get(*expression_id)
            .annotation
            .as_ref()
            .unwrap_or_else(|| {
                panic!("named type annotation should exist for {document_id:?}:{expression_id:?}")
            });
        type_resolver.resolve_annotation(annotation, document_id);
    }
}

// A script resolves type names against the package index plus its own `@type`/`@alias` declarations,
// which shadow package definitions of the same name. The package index already reports duplicates
// among package files; a script redefining a package name is intentional shadowing, not a duplicate.
pub(crate) fn build_script_local_type_index(
    package_types: &BTreeMap<Symbol, TypeInfo>,
    module: &Module,
) -> BTreeMap<Symbol, TypeInfo> {
    let mut types = package_types.clone();
    for definition in &module.definitions {
        types.insert(
            definition.definition.name,
            TypeInfo {
                kind: definition.definition.kind,
                arity: definition.definition.type_parameters.len(),
            },
        );
    }
    types
}

// The full-rebuild oracle for the package type index: each uniquely-defined `@type`/`@alias` name
// maps to its `TypeInfo`; a name defined by more than one package site stays out of the index (so
// references to it remain unresolved) and is returned in `duplicate_type_names` so
// `package_document_diagnostics` can emit the duplicate-type warning at each site. This from-scratch
// fold is `analysis`'s sole way to materialize the type index (rebuilt each `resolve_package`); it folds
// `document_type_definitions` and collates with `apply_type_definition_outcome`, so the membership and
// winner/duplicate rules have one source.
pub(crate) fn build_type_index(
    package_modules: &[(DocumentId, &Module)],
) -> (BTreeMap<Symbol, TypeInfo>, BTreeSet<Symbol>) {
    let mut sites_by_symbol = BTreeMap::<Symbol, Vec<TypeInfo>>::new();
    for (_, module) in package_modules {
        for (symbol, sites) in document_type_definitions(module) {
            sites_by_symbol.entry(symbol).or_default().extend(sites);
        }
    }

    let mut types = BTreeMap::new();
    let mut duplicate_type_names = BTreeSet::new();
    for (symbol, sites) in &sites_by_symbol {
        apply_type_definition_outcome(*symbol, sites, &mut types, &mut duplicate_type_names);
    }
    (types, duplicate_type_names)
}

// The package type definitions a single document contributes: for each `@type`/`@alias` name, the
// `TypeInfo`s it declares, in declaration order (a document may declare the same name more than once,
// which makes the name a duplicate). Shared by the incremental type-definition candidate index and by
// `build_type_index`, so the two cannot disagree on a document's contribution.
pub(crate) fn document_type_definitions(module: &Module) -> BTreeMap<Symbol, Vec<TypeInfo>> {
    let mut definitions = BTreeMap::<Symbol, Vec<TypeInfo>>::new();
    for definition in &module.definitions {
        definitions
            .entry(definition.definition.name)
            .or_default()
            .push(TypeInfo {
                kind: definition.definition.kind,
                arity: definition.definition.type_parameters.len(),
            });
    }
    definitions
}

// Collate a type name's package-wide definition sites into the materialized type index and duplicate
// set: a single site resolves to its `TypeInfo`; zero sites removes the name; two or more sites mark it
// a duplicate (and keep it out of the resolved index). The one source of truth for the winner/duplicate
// rule, applied by both the full rebuild and incremental maintenance into fresh or maintained maps.
pub(crate) fn apply_type_definition_outcome(
    symbol: Symbol,
    sites: &[TypeInfo],
    types: &mut BTreeMap<Symbol, TypeInfo>,
    duplicate_type_names: &mut BTreeSet<Symbol>,
) {
    match sites {
        [single] => {
            types.insert(symbol, *single);
            duplicate_type_names.remove(&symbol);
        }
        [] => {
            types.remove(&symbol);
            duplicate_type_names.remove(&symbol);
        }
        _ => {
            types.remove(&symbol);
            duplicate_type_names.insert(symbol);
        }
    }
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
        SurfaceType::Vector(inner_type)
        | SurfaceType::NamedVector(inner_type)
        | SurfaceType::List(inner_type)
        | SurfaceType::NamedList(inner_type) => surface_type_contains_named_type(inner_type),
        SurfaceType::Union(members) => members.iter().any(surface_type_contains_named_type),
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
    interner.resolve(symbol).is_some_and(|name| {
        crate::typecheck::BUILTINS
            .iter()
            .any(|(builtin_name, _)| *builtin_name == name)
    })
}

struct TypeResolver<'a> {
    interner: &'a Interner,
    rope: &'a Rope,
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
            SurfaceType::Vector(inner_type)
            | SurfaceType::NamedVector(inner_type)
            | SurfaceType::List(inner_type)
            | SurfaceType::NamedList(inner_type) => {
                self.resolve_surface_type(inner_type, local_type_parameters, range, document_id);
            }
            SurfaceType::Union(members) => {
                for member in members {
                    self.resolve_surface_type(member, local_type_parameters, range, document_id);
                }
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
                if let Some(element) = &function_type.variadic {
                    self.resolve_surface_type(element, local_type_parameters, range, document_id);
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
        // Narrow the underline from the whole annotation to the offending type name when it can be
        // located in the `#:` notation; fall back to the annotation range otherwise.
        let range = type_name_token_range(self.rope, range, &name).unwrap_or(range);
        push_diagnostic(
            self.diagnostics,
            document_id,
            Diagnostic::naming_error(range, format!("I could not resolve type `{name}`.")),
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
    interner: &'a Interner,
    next_binding_id: u32,
    local_scopes: Vec<BTreeMap<Symbol, BindingId>>,
    maybe_defined_scopes: Vec<BTreeMap<Symbol, BindingId>>,
    top_level_bindings: BTreeMap<Symbol, BindingId>,
    diagnostics: Vec<Diagnostic>,
    document_naming: NamesLocal,
}

impl<'a> DocumentNamingContext<'a> {
    fn new(document_id: DocumentId, arena: &'a HirArena, interner: &'a Interner) -> Self {
        Self {
            document_id,
            arena,
            interner,
            next_binding_id: 0,
            local_scopes: Vec::new(),
            maybe_defined_scopes: vec![BTreeMap::new()],
            top_level_bindings: BTreeMap::new(),
            diagnostics: Vec::new(),
            document_naming: NamesLocal::default(),
        }
    }

    fn resolve_module(mut self, module: &Module) -> DocumentNamingComputation {
        for expression_id in &module.expressions {
            self.resolve_expression(*expression_id);
        }
        self.compute_unused_bindings();
        DocumentNamingComputation {
            naming: self.document_naming,
            diagnostics: self.diagnostics,
        }
    }

    // A local assignment is unused when no symbol use resolves to it. Reassignment falls out for
    // free: `x <- 1; x <- 2; x` makes a fresh binding per `<-`, and only the last one is read, so the
    // earlier binding is reported. Names beginning with `.` or `_` are treated as intentional
    // throwaways and skipped, matching common R linting conventions.
    fn compute_unused_bindings(&mut self) {
        let mut used = BTreeSet::new();
        for (expression_id, binding_id) in &self.document_naming.expression_resolutions {
            if matches!(
                self.arena.get(*expression_id).kind,
                ExpressionKind::Symbol(_)
            ) {
                used.insert(*binding_id);
            }
        }

        for (binding_id, binding) in &self.document_naming.bindings {
            if binding.kind != BindingKind::LocalAssignment || used.contains(binding_id) {
                continue;
            }
            // A leading `_` is only writable in R as a backtick-quoted name, so strip backticks
            // before checking the throwaway-name conventions.
            let name = self
                .interner
                .resolve(binding.symbol)
                .unwrap_or("")
                .trim_matches('`');
            if name.starts_with('.') || name.starts_with('_') {
                continue;
            }
            self.document_naming.unused_bindings.push(*binding_id);
        }
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
                    if let Some(binding_id) = self.resolve_maybe_defined_symbol(*symbol) {
                        self.document_naming
                            .expression_resolutions
                            .insert(expression_id, binding_id);
                        self.document_naming
                            .maybe_undefined_expressions
                            .insert(expression_id);
                        self.diagnostics.push(Diagnostic::naming_warning(
                        expression.range,
                        format!(
                            "`{}` might be undefined here because it is introduced only in conditionally executed code.",
                            self.interner.resolve(*symbol).unwrap_or("<unknown>")
                        ),
                    ));
                    } else {
                        self.document_naming
                            .non_locals
                            .insert(expression_id, *symbol);
                    }
                }
            },
            ExpressionKind::Block { expressions, .. } => {
                for nested_expression in expressions {
                    self.resolve_expression(*nested_expression);
                }
            }
            ExpressionKind::Assign { target, value, .. } => {
                self.resolve_expression(*value);
                let kind = if self.local_scopes.is_empty() {
                    BindingKind::TopLevelAssignment
                } else {
                    BindingKind::LocalAssignment
                };
                let binding_id = self.fresh_binding(*target, expression.range, kind);
                if let Some(scope) = self.local_scopes.last_mut() {
                    scope.insert(*target, binding_id);
                } else {
                    self.top_level_bindings.insert(*target, binding_id);
                }
                self.document_naming
                    .expression_resolutions
                    .insert(expression_id, binding_id);
            }
            ExpressionKind::Function { parameters, body } => {
                let mut scope = BTreeMap::new();
                for parameter in parameters {
                    let binding_id = self.fresh_binding(
                        parameter.symbol,
                        parameter.range,
                        BindingKind::Parameter,
                    );
                    scope.insert(parameter.symbol, binding_id);
                }
                self.local_scopes.push(scope);
                self.maybe_defined_scopes.push(BTreeMap::new());
                // Defaults are evaluated in the function frame with every parameter in scope, so
                // they resolve after all parameters are bound and before the body.
                for parameter in parameters {
                    if let Some(default_expression_id) = parameter.default {
                        self.resolve_expression(default_expression_id);
                    }
                }
                self.resolve_expression(*body);
                self.local_scopes.pop();
                self.maybe_defined_scopes.pop();
            }
            ExpressionKind::Local { body } => {
                // `local(expr)` introduces a fresh child scope like a function body (no parameters), so
                // an assignment inside is local and does not leak, while references still see the
                // enclosing scope's names because `resolve_local_symbol` walks outward.
                self.local_scopes.push(BTreeMap::new());
                self.maybe_defined_scopes.push(BTreeMap::new());
                self.resolve_expression(*body);
                self.local_scopes.pop();
                self.maybe_defined_scopes.pop();
            }
            ExpressionKind::If {
                condition,
                consequence,
                alternative,
            } => {
                self.resolve_expression(*condition);
                let consequence_bindings =
                    self.resolve_conditionally_executed_expression(*consequence);
                if let Some(alternative) = alternative {
                    let alternative_bindings =
                        self.resolve_conditionally_executed_expression(*alternative);
                    for (symbol, binding_id) in &consequence_bindings {
                        if !alternative_bindings.contains_key(symbol) {
                            self.mark_symbol_maybe_defined(*symbol, *binding_id);
                        }
                    }
                    for (symbol, binding_id) in &alternative_bindings {
                        if !consequence_bindings.contains_key(symbol) {
                            self.mark_symbol_maybe_defined(*symbol, *binding_id);
                        }
                    }
                } else {
                    for (symbol, binding_id) in consequence_bindings {
                        self.mark_symbol_maybe_defined(symbol, binding_id);
                    }
                }
            }
            ExpressionKind::For {
                variable,
                sequence,
                body,
            } => {
                self.resolve_expression(*sequence);
                let binding_id =
                    self.fresh_binding(*variable, expression.range, BindingKind::ForVariable);
                self.local_scopes
                    .push(BTreeMap::from([(*variable, binding_id)]));
                let body_bindings = self.resolve_conditionally_executed_expression(*body);
                self.local_scopes
                    .pop()
                    .unwrap_or_else(|| panic!("for loop scope should exist for {expression_id:?}"));
                for (symbol, binding_id) in body_bindings {
                    self.mark_symbol_maybe_defined(symbol, binding_id);
                }
            }
            ExpressionKind::While { condition, body } => {
                self.resolve_expression(*condition);
                let body_bindings = self.resolve_conditionally_executed_expression(*body);
                for (symbol, binding_id) in body_bindings {
                    self.mark_symbol_maybe_defined(symbol, binding_id);
                }
            }
            ExpressionKind::Repeat { body } => {
                self.resolve_expression(*body);
            }
            ExpressionKind::UnaryMinus { value } | ExpressionKind::UnaryNot { value } => {
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
            | ExpressionKind::AtomicConstant(_)
            | ExpressionKind::StringLiteralName(_)
            | ExpressionKind::Unsupported => {}
        }
    }

    fn fresh_binding(&mut self, symbol: Symbol, range: Range, kind: BindingKind) -> BindingId {
        let binding_id = BindingId(self.next_binding_id);
        self.next_binding_id += 1;
        self.document_naming.bindings.insert(
            binding_id,
            BindingInfo {
                id: binding_id,
                module_id: self.document_id,
                symbol,
                range,
                kind,
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

    fn resolve_maybe_defined_symbol(&self, symbol: Symbol) -> Option<BindingId> {
        self.maybe_defined_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(&symbol).copied())
    }

    fn mark_symbol_maybe_defined(&mut self, symbol: Symbol, binding_id: BindingId) {
        let Some(scope) = self.maybe_defined_scopes.last_mut() else {
            panic!("maybe-defined scope should exist for {symbol:?}");
        };
        scope.insert(symbol, binding_id);
    }

    fn resolve_conditionally_executed_expression(
        &mut self,
        expression_id: ExpressionId,
    ) -> BTreeMap<Symbol, BindingId> {
        let local_scope_snapshot = self.local_scopes.clone();
        let maybe_defined_scopes_snapshot = self.maybe_defined_scopes.clone();
        let top_level_bindings_snapshot = self.top_level_bindings.clone();
        self.resolve_expression(expression_id);
        let introduced_symbols = collect_new_symbols(
            &self.local_scopes,
            &local_scope_snapshot,
            &self.maybe_defined_scopes,
            &maybe_defined_scopes_snapshot,
            &self.top_level_bindings,
            &top_level_bindings_snapshot,
        );
        self.local_scopes = local_scope_snapshot;
        self.maybe_defined_scopes = maybe_defined_scopes_snapshot;
        self.top_level_bindings = top_level_bindings_snapshot;
        introduced_symbols
    }
}

fn collect_new_symbols(
    current_local_scopes: &[BTreeMap<Symbol, BindingId>],
    local_scope_snapshot: &[BTreeMap<Symbol, BindingId>],
    current_maybe_defined_scopes: &[BTreeMap<Symbol, BindingId>],
    maybe_defined_scopes_snapshot: &[BTreeMap<Symbol, BindingId>],
    current_top_level_bindings: &BTreeMap<Symbol, BindingId>,
    top_level_bindings_snapshot: &BTreeMap<Symbol, BindingId>,
) -> BTreeMap<Symbol, BindingId> {
    let mut introduced_symbols = BTreeMap::new();
    let symbol_existed_before = |symbol: Symbol| {
        local_scope_snapshot
            .iter()
            .any(|scope| scope.contains_key(&symbol))
            || maybe_defined_scopes_snapshot
                .iter()
                .any(|scope| scope.contains_key(&symbol))
            || top_level_bindings_snapshot.contains_key(&symbol)
    };

    for (current_scope, snapshot_scope) in current_local_scopes.iter().zip(local_scope_snapshot) {
        for (symbol, binding_id) in current_scope {
            if !snapshot_scope.contains_key(symbol) && !symbol_existed_before(*symbol) {
                introduced_symbols.insert(*symbol, *binding_id);
            }
        }
    }

    for (current_scope, snapshot_scope) in current_maybe_defined_scopes
        .iter()
        .zip(maybe_defined_scopes_snapshot)
    {
        for (symbol, binding_id) in current_scope {
            if !snapshot_scope.contains_key(symbol) && !symbol_existed_before(*symbol) {
                introduced_symbols.insert(*symbol, *binding_id);
            }
        }
    }

    for (symbol, binding_id) in current_top_level_bindings {
        if !top_level_bindings_snapshot.contains_key(symbol) && !symbol_existed_before(*symbol) {
            introduced_symbols.insert(*symbol, *binding_id);
        }
    }

    introduced_symbols
}
