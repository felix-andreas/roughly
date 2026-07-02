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
    // Reads and assignment sites mapped to the variable slot they touch. Every `<-`/`=` to one name
    // in one scope resolves to the same slot (R assignment mutates the scope's environment), so a
    // read after a conditional or loop write still resolves to the variable.
    pub expression_resolutions: BTreeMap<ExpressionId, BindingId>,
    pub maybe_undefined_expressions: BTreeSet<ExpressionId>,
    pub non_locals: BTreeMap<ExpressionId, Symbol>,
    pub named_type_annotations: Vec<ExpressionId>,
    // Assignment writes whose value no read can reach on any control-flow path (dead stores).
    // Computed from the reaching-write sets during resolution; surfaced as diagnostics only when
    // the `unused` check is enabled.
    pub unused_assignments: Vec<UnusedAssignment>,
}

// A dead store: an assignment whose written value is never read. Carries the diagnostic payload
// directly so consumers can render it without re-walking the HIR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnusedAssignment {
    pub symbol: Symbol,
    pub range: Range,
}

// A variable slot (or a package-visible top-level assignment site; see `BindingKind`). For a slot
// with several writes, `range` is the first write's site — the slot's definition site for hover
// and goto-definition.
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
    // A package document's top-level `name <- value`: one binding per assignment site (winner
    // semantics picks the exported one), or a document slot created by conditionally executed
    // top-level code (not package-visible). Never reported as unused.
    TopLevelAssignment,
    // A variable slot in a function, `local()`, or script top-level scope, created by its first
    // assignment. Writes to these slots are the ones eligible for the unused (dead-store) check.
    LocalAssignment,
    // A function parameter's slot. R does not treat unused parameters as errors, so the parameter
    // itself is never reported (explicit assignments to it still are).
    Parameter,
    // A `for (variable in ...)` loop variable's slot, scoped to the loop body and re-initialized
    // each iteration. Often intentionally unused, so the variable itself is never reported.
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
    // A script executes top-down like one big function body, so its top level is an ordinary
    // variable-slot frame. A package document's top level is the package-global namespace instead:
    // direct assignments are per-site package definitions resolved through winner semantics, and
    // only conditionally executed top-level code gets document slots.
    context.scopes.push(Scope::new(match document_kind {
        DocumentKind::Script => ScopeKind::Frame,
        DocumentKind::Package => ScopeKind::TopLevel,
    }));
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopeKind {
    // A real R environment frame: a function body, `local()`, or a script's top level. `<-` writes
    // resolve or create their variable slot in the innermost frame.
    Frame,
    // The synthetic scope holding only a `for` loop's variable slot. Writes to other names pass
    // through it into the enclosing frame (the loop body shares the enclosing R environment).
    Loop,
    // A package document's top level. Direct assignments here are per-site package definitions
    // (no slot); only conditionally executed top-level code creates document slots in this scope.
    TopLevel,
}

struct Scope {
    kind: ScopeKind,
    slots: BTreeMap<Symbol, BindingId>,
}

impl Scope {
    fn new(kind: ScopeKind) -> Self {
        Scope {
            kind,
            slots: BTreeMap::new(),
        }
    }
}

// One element of a variable slot's reaching-write set at a program point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Reach {
    // Some path reaches this point without any write to the slot.
    Unassigned,
    // A write with no reportable site: a parameter or `for`-variable initialization, or a write to
    // a top-level slot (excluded from the unused check).
    Implicit,
    // An assignment write, by index into `assignment_writes`.
    Assignment(u32),
}

// Per-slot reaching-write sets at the current program point. Forked and joined at control-flow
// splits; loop bodies iterate to a fixed point (the lattice is finite and joins only grow it).
type FlowState = BTreeMap<BindingId, BTreeSet<Reach>>;

struct AssignmentWrite {
    symbol: Symbol,
    range: Range,
    used: bool,
}

enum WriteTarget {
    Slot {
        binding_id: BindingId,
        // Whether this write participates in the unused (dead-store) check. Top-level slots are
        // package-level names, so their writes are excluded like per-site top-level assignments.
        reportable: bool,
    },
    // A package document's per-site top-level assignment: no slot, resolved through the
    // package-global winner semantics instead.
    TopLevelSite(BindingId),
}

struct DocumentNamingContext<'a> {
    document_id: DocumentId,
    arena: &'a HirArena,
    interner: &'a Interner,
    next_binding_id: u32,
    scopes: Vec<Scope>,
    // Names assigned by an unconditional top-level (or bare-block) assignment so far. A conditional
    // top-level assignment to such a name targets the package global at run time, so it gets a
    // per-site binding rather than capturing later top-level reads into a document slot.
    top_level_names: BTreeSet<Symbol>,
    // Depth of conditionally executed constructs (`if` branches, loop regions). Only consulted at
    // package top level, where it routes writes between per-site definitions and document slots;
    // inside a frame every write targets the frame regardless.
    conditional_depth: usize,
    // Loop bodies are re-walked to a fixed point, so binding creation must be idempotent across
    // walks: one binding per (site, symbol), keyed by the site's byte range.
    bindings_by_site: HashMap<(usize, usize, Symbol), BindingId>,
    flow: FlowState,
    assignment_writes: Vec<AssignmentWrite>,
    write_indexes_by_expression: HashMap<ExpressionId, u32>,
    // Diagnostics and the maybe-undefined set are recorded only on a loop region's final walk, so
    // fixed-point pre-passes cannot emit duplicates from not-yet-stable flow states.
    emit: bool,
    named_type_annotation_expressions: BTreeSet<ExpressionId>,
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
            scopes: Vec::new(),
            top_level_names: BTreeSet::new(),
            conditional_depth: 0,
            bindings_by_site: HashMap::new(),
            flow: FlowState::new(),
            assignment_writes: Vec::new(),
            write_indexes_by_expression: HashMap::new(),
            emit: true,
            named_type_annotation_expressions: BTreeSet::new(),
            diagnostics: Vec::new(),
            document_naming: NamesLocal::default(),
        }
    }

    fn resolve_module(mut self, module: &Module) -> DocumentNamingComputation {
        for expression_id in &module.expressions {
            self.resolve_expression(*expression_id);
        }
        self.collect_unused_assignments();
        DocumentNamingComputation {
            naming: self.document_naming,
            diagnostics: self.diagnostics,
        }
    }

    // A write is unused when no read's reaching-write set ever contained it: its value is dead on
    // every path. Reads marked writes as used during resolution, so this only filters. Names
    // beginning with `.` or `_` are treated as intentional throwaways and skipped, matching common
    // R linting conventions.
    fn collect_unused_assignments(&mut self) {
        for write in &self.assignment_writes {
            if write.used {
                continue;
            }
            // A leading `_` is only writable in R as a backtick-quoted name, so strip backticks
            // before checking the throwaway-name conventions.
            let name = self
                .interner
                .resolve(write.symbol)
                .unwrap_or("")
                .trim_matches('`');
            if name.starts_with('.') || name.starts_with('_') {
                continue;
            }
            self.document_naming
                .unused_assignments
                .push(UnusedAssignment {
                    symbol: write.symbol,
                    range: write.range,
                });
        }
    }

    fn resolve_expression(&mut self, expression_id: ExpressionId) {
        let expression = self.arena.get(expression_id);
        if let Some(annotation) = &expression.annotation
            && annotation_contains_named_type(annotation)
            && self
                .named_type_annotation_expressions
                .insert(expression_id)
        {
            self.document_naming
                .named_type_annotations
                .push(expression_id);
        }

        match &expression.kind {
            ExpressionKind::Symbol(symbol) => match self.resolve_read(*symbol) {
                Some((binding_id, unassigned_reachable)) => {
                    // A loop pre-pass may have filed this read as a non-local before the slot's
                    // write was seen; the later pass's resolution replaces it (and vice versa), so
                    // the final walk always leaves exactly one of the two records.
                    self.document_naming.non_locals.remove(&expression_id);
                    self.document_naming
                        .expression_resolutions
                        .insert(expression_id, binding_id);
                    if unassigned_reachable && self.emit {
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
                    }
                }
                None => {
                    self.document_naming
                        .expression_resolutions
                        .remove(&expression_id);
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
                match self.resolve_write(*target, expression.range) {
                    WriteTarget::Slot {
                        binding_id,
                        reportable,
                    } => {
                        let reach = if reportable {
                            let index = self.record_assignment_write(
                                expression_id,
                                *target,
                                expression.range,
                            );
                            Reach::Assignment(index)
                        } else {
                            Reach::Implicit
                        };
                        // A write kills every earlier write to the slot on this path.
                        self.flow.insert(binding_id, BTreeSet::from([reach]));
                        self.document_naming
                            .expression_resolutions
                            .insert(expression_id, binding_id);
                    }
                    WriteTarget::TopLevelSite(binding_id) => {
                        self.document_naming
                            .expression_resolutions
                            .insert(expression_id, binding_id);
                    }
                }
            }
            ExpressionKind::Function { parameters, body } => {
                let mut scope = Scope::new(ScopeKind::Frame);
                for parameter in parameters {
                    let binding_id =
                        self.binding(parameter.symbol, parameter.range, BindingKind::Parameter);
                    scope.slots.insert(parameter.symbol, binding_id);
                    self.flow
                        .insert(binding_id, BTreeSet::from([Reach::Implicit]));
                }
                self.scopes.push(scope);
                // Defaults are evaluated in the function frame with every parameter in scope, so
                // they resolve after all parameters are bound and before the body.
                for parameter in parameters {
                    if let Some(default_expression_id) = parameter.default {
                        self.resolve_expression(default_expression_id);
                    }
                }
                self.resolve_expression(*body);
                self.scopes.pop();
            }
            ExpressionKind::Local { body } => {
                // `local(expr)` introduces a fresh child frame like a function body (no
                // parameters), so an assignment inside is local and does not leak, while references
                // still see the enclosing scope's names because reads walk outward.
                self.scopes.push(Scope::new(ScopeKind::Frame));
                self.resolve_expression(*body);
                self.scopes.pop();
            }
            ExpressionKind::If {
                condition,
                consequence,
                alternative,
            } => {
                self.resolve_expression(*condition);
                let before = self.flow.clone();
                self.conditional_depth += 1;
                self.resolve_expression(*consequence);
                let after_consequence = std::mem::replace(&mut self.flow, before.clone());
                match alternative {
                    Some(alternative) => {
                        self.resolve_expression(*alternative);
                        let after_alternative = std::mem::take(&mut self.flow);
                        self.flow = join_flow(&after_consequence, &after_alternative);
                    }
                    // Without an `else`, the construct may fall through untouched, so the branch
                    // state joins with the state before the `if`.
                    None => self.flow = join_flow(&before, &after_consequence),
                }
                self.conditional_depth -= 1;
            }
            ExpressionKind::For {
                variable,
                sequence,
                body,
            } => {
                // The sequence is evaluated once, before any iteration, so it stays outside the
                // loop region.
                self.resolve_expression(*sequence);
                let loop_binding_id =
                    self.binding(*variable, expression.range, BindingKind::ForVariable);
                let mut scope = Scope::new(ScopeKind::Loop);
                scope.slots.insert(*variable, loop_binding_id);
                self.scopes.push(scope);
                let body = *body;
                self.resolve_loop_region(Some(loop_binding_id), false, &mut |context| {
                    context.resolve_expression(body);
                });
                self.scopes.pop();
                // The loop variable is not visible after the loop, so its flow entry is dead.
                self.flow.remove(&loop_binding_id);
            }
            ExpressionKind::While { condition, body } => {
                // The condition re-evaluates before every iteration, so it belongs to the iterated
                // region: a read in it sees writes flowing around the back edge.
                let condition = *condition;
                let body = *body;
                self.resolve_loop_region(None, false, &mut |context| {
                    context.resolve_expression(condition);
                    context.resolve_expression(body);
                });
            }
            ExpressionKind::Repeat { body } => {
                let body = *body;
                self.resolve_loop_region(None, true, &mut |context| {
                    context.resolve_expression(body);
                });
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

    // A loop body may run zero or more times, so state flowing around the back edge joins into the
    // body's entry state: iterate the region walk until that entry stabilizes (`entry` grows
    // monotonically in the finite reaching-set lattice, so this terminates), then run one final
    // walk with diagnostics enabled. `repeat` runs at least once, so its exit state is the region's
    // own out-state instead of a join with the pre-state.
    fn resolve_loop_region(
        &mut self,
        loop_variable: Option<BindingId>,
        runs_at_least_once: bool,
        region: &mut dyn FnMut(&mut Self),
    ) {
        self.conditional_depth += 1;
        let before = self.flow.clone();
        let saved_emit = self.emit;
        self.emit = false;
        let mut entry = before.clone();
        loop {
            self.flow = entry.clone();
            if let Some(loop_binding_id) = loop_variable {
                // The loop variable is re-initialized from the iterable on every iteration.
                self.flow
                    .insert(loop_binding_id, BTreeSet::from([Reach::Implicit]));
            }
            region(self);
            let next_entry = join_flow(&before, &self.flow);
            if next_entry == entry {
                break;
            }
            entry = next_entry;
        }
        self.emit = saved_emit;
        self.flow = entry.clone();
        if let Some(loop_binding_id) = loop_variable {
            self.flow
                .insert(loop_binding_id, BTreeSet::from([Reach::Implicit]));
        }
        region(self);
        if !runs_at_least_once {
            let exit = std::mem::take(&mut self.flow);
            self.flow = join_flow(&before, &exit);
        }
        self.conditional_depth -= 1;
    }

    // Resolves a read: the innermost visible slot that can actually be assigned at this point. A
    // slot none of whose reaching entries is a write behaves as if it did not exist — R only
    // creates the frame variable when an assignment executes — so the lookup continues outward
    // (and ultimately to package globals via `non_locals`). Returns whether an unassigned path
    // also reaches, which drives the maybe-undefined warning.
    fn resolve_read(&mut self, symbol: Symbol) -> Option<(BindingId, bool)> {
        for scope in self.scopes.iter().rev() {
            let Some(&binding_id) = scope.slots.get(&symbol) else {
                continue;
            };
            let Some(reach) = self.flow.get(&binding_id) else {
                continue;
            };
            if !reach
                .iter()
                .any(|element| *element != Reach::Unassigned)
            {
                continue;
            }
            let unassigned_reachable = reach.contains(&Reach::Unassigned);
            for element in reach.clone() {
                if let Reach::Assignment(index) = element {
                    self.assignment_writes
                        .get_mut(index as usize)
                        .unwrap_or_else(|| {
                            panic!("reaching set holds an unrecorded write index {index}")
                        })
                        .used = true;
                }
            }
            return Some((binding_id, unassigned_reachable));
        }
        None
    }

    // Resolves an assignment's target. `<-` writes the current R frame: the innermost frame's slot
    // (created on first write), or the loop variable when the name is the enclosing `for`'s
    // variable. At package top level, unconditional writes are per-site package definitions and
    // conditional writes get document slots (see `ScopeKind::TopLevel`).
    fn resolve_write(&mut self, symbol: Symbol, range: Range) -> WriteTarget {
        for index in (0..self.scopes.len()).rev() {
            if let Some(&binding_id) = self.scopes[index].slots.get(&symbol) {
                match self.scopes[index].kind {
                    ScopeKind::Frame | ScopeKind::Loop => {
                        return WriteTarget::Slot {
                            binding_id,
                            reportable: true,
                        };
                    }
                    ScopeKind::TopLevel => {
                        if self.conditional_depth > 0 {
                            return WriteTarget::Slot {
                                binding_id,
                                reportable: false,
                            };
                        }
                        // An unconditional top-level assignment takes the name package-global;
                        // the conditional slot no longer owns later reads.
                        self.scopes[index].slots.remove(&symbol);
                        return self.top_level_write(symbol, range, index);
                    }
                }
            }
            match self.scopes[index].kind {
                ScopeKind::Frame => {
                    let binding_id = self.binding(symbol, range, BindingKind::LocalAssignment);
                    self.scopes[index].slots.insert(symbol, binding_id);
                    return WriteTarget::Slot {
                        binding_id,
                        reportable: true,
                    };
                }
                ScopeKind::TopLevel => return self.top_level_write(symbol, range, index),
                ScopeKind::Loop => {}
            }
        }
        unreachable!("the scope stack always ends in a frame or top-level scope")
    }

    fn top_level_write(
        &mut self,
        symbol: Symbol,
        range: Range,
        top_level_index: usize,
    ) -> WriteTarget {
        if self.conditional_depth == 0 {
            self.top_level_names.insert(symbol);
            let binding_id = self.binding(symbol, range, BindingKind::TopLevelAssignment);
            return WriteTarget::TopLevelSite(binding_id);
        }
        if self.top_level_names.contains(&symbol) {
            // A conditional reassignment of a package-visible name overwrites the package global
            // at run time; it keeps a per-site binding but never captures later top-level reads,
            // which keep resolving to the exported winner.
            let binding_id = self.binding(symbol, range, BindingKind::TopLevelAssignment);
            return WriteTarget::TopLevelSite(binding_id);
        }
        let binding_id = self.binding(symbol, range, BindingKind::TopLevelAssignment);
        self.scopes[top_level_index].slots.insert(symbol, binding_id);
        WriteTarget::Slot {
            binding_id,
            reportable: false,
        }
    }

    fn record_assignment_write(
        &mut self,
        expression_id: ExpressionId,
        symbol: Symbol,
        range: Range,
    ) -> u32 {
        if let Some(&index) = self.write_indexes_by_expression.get(&expression_id) {
            return index;
        }
        let index = u32::try_from(self.assignment_writes.len())
            .unwrap_or_else(|_| panic!("assignment write count exceeds u32 for {expression_id:?}"));
        self.write_indexes_by_expression.insert(expression_id, index);
        self.assignment_writes.push(AssignmentWrite {
            symbol,
            range,
            used: false,
        });
        index
    }

    fn binding(&mut self, symbol: Symbol, range: Range, kind: BindingKind) -> BindingId {
        let site = (range.start_byte, range.end_byte, symbol);
        if let Some(&binding_id) = self.bindings_by_site.get(&site) {
            return binding_id;
        }
        let binding_id = BindingId(self.next_binding_id);
        self.next_binding_id += 1;
        self.bindings_by_site.insert(site, binding_id);
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
}

// Joins the reaching-write sets of two control-flow paths: a slot's set is the union of both
// sides, and a slot one side never saw was unassigned on that path.
fn join_flow(left: &FlowState, right: &FlowState) -> FlowState {
    let mut joined = left.clone();
    for (binding_id, right_reach) in right {
        match joined.get_mut(binding_id) {
            Some(reach) => reach.extend(right_reach.iter().copied()),
            None => {
                let mut reach = right_reach.clone();
                reach.insert(Reach::Unassigned);
                joined.insert(*binding_id, reach);
            }
        }
    }
    for (binding_id, reach) in &mut joined {
        if !right.contains_key(binding_id) {
            reach.insert(Reach::Unassigned);
        }
    }
    joined
}
