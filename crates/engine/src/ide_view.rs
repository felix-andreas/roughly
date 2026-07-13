//! The engine-backed [`IdeDatabase`]: it serves the IDE features (`analysis::ide::generic::*`) off the
//! memoized query graph, reusing the *same* IDE orchestration that `analysis` exposes so both the engine
//! and the from-scratch path answer identically.
//!
//! # The prime-then-borrow discipline (the load-bearing mechanic)
//!
//! `analysis::ide::generic::*` reads facts through `&dyn IdeDatabase` returning `&T`. The engine's
//! [`Engine::fetch`] hands back a fresh `Shared<T>` (an `Rc`) each call and a query body borrows the shared
//! interner **mutably** while it runs. So an IDE call runs in three phases:
//!
//! 1. **prime** — fetch every fact the feature will read into an owned per-call [`Caches`] (`Rc` snapshots).
//!    All `fetch`es (which may `borrow_mut` the interner) happen here; no interner borrow is held.
//! 2. **borrow** — take one immutable `Ref<Interner>` ([`RoughlyQueries::interner_ref`]) as a stack local.
//! 3. **run** — build a borrowing [`EngineIdeRef`] over the caches + interner and call the generic core,
//!    which reads only the caches and therefore issues **no** `fetch` — so the interner stays soundly
//!    borrowed. Returning `&T` from the owned `Rc`s in `Caches` is exactly how `Analysis` returns `&T` from
//!    its retained maps; the per-call cache plays the role of the retained state.
//!
//! The prime scope is feature-specific and is what keeps the per-keystroke point-query features off the
//! O(project) path: hover/inlay/signature fetch `Typecheck` only for the *target* file, so a point query on
//! an unchanged file re-runs zero `Typecheck` bodies (proven by the exec-counter tests). references/rename
//! are whole-project scans and prime every file (text-prefiltered inside the generic occurrence scan), but
//! add no new query key — they record no memo dependency (IDE fetches are top-level, outside any
//! `recompute`).

use {
    crate::{
        Engine, Shared,
        queries::{FileId, Key, RoughlyQueries, SourceText},
    },
    analysis::{
        document::{Document, DocumentId},
        hir::{ExpressionId, Module},
        ide::{
            CodeAction, CompletionResult, HoverInfo, IdeDatabase, InlayHint, Location,
            RenameResult, SignatureHelp, generic, generic::GlobalCompletionEntry,
        },
        interner::{Interner, Symbol},
        naming::{DocumentKind, DocumentNamingComputation, NamesGlobal, NamesLocal},
        stdlib::StubLibrary,
        text::{TextPosition, TextRange},
        typecheck::ModuleCheck,
        types::{Constraint, CoreType, InferenceVariableId, TypeScheme},
    },
    ropey::Rope,
    std::{
        collections::{BTreeMap, BTreeSet, HashMap},
        path::{Path, PathBuf},
    },
};

/// The `FileId`↔path bijection. The engine keys every query by [`FileId`] so that paths never perturb a
/// memo; paths are a pure IDE/LSP concern, so this table is owned by the engine's *host* (the LSP
/// `ServerState`, or a test driver) and kept in lockstep with the `ProjectFiles` input — every
/// `set_input(SourceText(f))` pairs with [`PathTable::insert`] and every `remove_input(SourceText(f))` with
/// [`PathTable::remove`]. It is the one thing the engine cannot derive itself: outgoing `Location`s need a
/// `FileId → path` and the IDE entry needs a `path → FileId`.
pub struct PathTable {
    base: PathBuf,
    to_path: BTreeMap<FileId, PathBuf>,
    to_id: HashMap<PathBuf, FileId>,
}

impl PathTable {
    pub fn new(base: PathBuf) -> PathTable {
        PathTable {
            base,
            to_path: BTreeMap::new(),
            to_id: HashMap::new(),
        }
    }

    pub fn insert(&mut self, file: FileId, path: PathBuf) {
        // Reclassification (package ↔ script) re-inserts a file under a new path; drop the stale reverse
        // mapping for its old path so a lookup of the old path no longer resolves to this file.
        if let Some(previous_path) = self.to_path.insert(file, path.clone())
            && previous_path != path
        {
            self.to_id.remove(&previous_path);
        }
        self.to_id.insert(path, file);
    }

    pub fn remove(&mut self, file: FileId) {
        if let Some(path) = self.to_path.remove(&file) {
            self.to_id.remove(&path);
        }
    }

    pub fn base_path(&self) -> &Path {
        &self.base
    }

    pub fn id(&self, path: &Path) -> Option<DocumentId> {
        self.to_id.get(path).map(|file| DocumentId(*file))
    }

    pub fn path(&self, document_id: DocumentId) -> Option<&Path> {
        self.to_path.get(&document_id.0).map(PathBuf::as_path)
    }
}

/// An engine-backed IDE view, constructed per request over the engine and the host's [`PathTable`].
pub struct EngineIde<'engine> {
    engine: &'engine Engine<RoughlyQueries>,
    paths: &'engine PathTable,
}

impl<'engine> EngineIde<'engine> {
    pub fn new(
        engine: &'engine Engine<RoughlyQueries>,
        paths: &'engine PathTable,
    ) -> EngineIde<'engine> {
        EngineIde { engine, paths }
    }

    pub fn hover(&self, path: &Path, position: TextPosition) -> Option<HoverInfo> {
        let target = self.paths.id(path)?;
        let caches = self.prime_hover(target);
        let interner = self.engine.group().interner_ref();
        let database =
            EngineIdeRef::new(&caches, &interner, self.paths, self.engine.group().stubs());
        generic::hover(&database, path, position)
    }

    pub fn inlay_hints(&self, path: &Path, viewport: Option<TextRange>) -> Vec<InlayHint> {
        let Some(target) = self.paths.id(path) else {
            return Vec::new();
        };
        let caches = self.prime_typed(target);
        let interner = self.engine.group().interner_ref();
        let database =
            EngineIdeRef::new(&caches, &interner, self.paths, self.engine.group().stubs());
        generic::inlay_hints(&database, path, viewport)
    }

    pub fn signature_help(&self, path: &Path, position: TextPosition) -> Option<SignatureHelp> {
        let target = self.paths.id(path)?;
        let caches = self.prime_typed(target);
        let interner = self.engine.group().interner_ref();
        let database =
            EngineIdeRef::new(&caches, &interner, self.paths, self.engine.group().stubs());
        generic::signature_help(&database, path, position)
    }

    pub fn completion(&self, path: &Path, position: TextPosition) -> Option<CompletionResult> {
        let target = self.paths.id(path)?;
        let caches = self.prime_completion(target, position);
        let interner = self.engine.group().interner_ref();
        let database =
            EngineIdeRef::new(&caches, &interner, self.paths, self.engine.group().stubs());
        let global_entries = caches
            .completion_index
            .as_deref()
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        generic::completion(&database, path, position, global_entries)
    }

    pub fn definition(&self, path: &Path, position: TextPosition) -> Option<Vec<Location>> {
        let target = self.paths.id(path)?;
        let caches = self.prime_definition(target, position);
        let interner = self.engine.group().interner_ref();
        let database =
            EngineIdeRef::new(&caches, &interner, self.paths, self.engine.group().stubs());
        generic::definition(&database, path, position)
    }

    pub fn references(
        &self,
        path: &Path,
        position: TextPosition,
        include_declaration: bool,
    ) -> Option<Vec<Location>> {
        let target = self.paths.id(path)?;
        let caches = self.prime_occurrence_scan(target, position);
        let interner = self.engine.group().interner_ref();
        let database =
            EngineIdeRef::new(&caches, &interner, self.paths, self.engine.group().stubs());
        generic::references(&database, path, position, include_declaration)
    }

    pub fn rename(
        &self,
        path: &Path,
        position: TextPosition,
        new_name: &str,
    ) -> Option<RenameResult> {
        let target = self.paths.id(path)?;
        let caches = self.prime_occurrence_scan(target, position);
        let interner = self.engine.group().interner_ref();
        let database =
            EngineIdeRef::new(&caches, &interner, self.paths, self.engine.group().stubs());
        generic::rename(&database, path, position, new_name)
    }

    pub fn code_actions(&self, path: &Path, range: TextRange) -> Vec<CodeAction> {
        let Some(target) = self.paths.id(path) else {
            return Vec::new();
        };
        let caches = self.prime_code_actions(target);
        let interner = self.engine.group().interner_ref();
        let database =
            EngineIdeRef::new(&caches, &interner, self.paths, self.engine.group().stubs());
        generic::code_actions(&database, path, range)
    }

    pub fn type_definition(&self, path: &Path, position: TextPosition) -> Option<Vec<Location>> {
        let target = self.paths.id(path)?;
        let caches = self.prime_type_definition(target);
        let interner = self.engine.group().interner_ref();
        let database =
            EngineIdeRef::new(&caches, &interner, self.paths, self.engine.group().stubs());
        generic::type_definition(&database, path, position)
    }

    // ------------------------------------------------------------------------------------------------
    // Prime: fetch a feature's bounded fact scope into an owned `Caches`. The only place fetches happen.
    // ------------------------------------------------------------------------------------------------

    // hover: the target's module/naming/checked-types, plus module+naming of the export file of every
    // package global the target references (for the "defined at ..." line). `Typecheck` is fetched for the
    // target *only* — the export reads are naming/module, never types — so a point query re-runs no
    // `Typecheck` body on an unchanged package.
    fn prime_hover(&self, target: DocumentId) -> Caches {
        let mut caches = self.empty_caches();
        // The target's parse (rope) is read directly to re-lex a `#:` type annotation the cursor sits in.
        self.prime_document(&mut caches, target);
        self.prime_module(&mut caches, target);
        self.prime_naming(&mut caches, target);
        self.prime_check(&mut caches, target);
        self.prime_package_naming(&mut caches);
        for export in self.referenced_export_files(target, &caches) {
            self.prime_module(&mut caches, export);
            self.prime_naming(&mut caches, export);
        }
        caches
    }

    // inlay_hints / signature_help: the target's module + checked types. No naming, no cross-file facts.
    fn prime_typed(&self, target: DocumentId) -> Caches {
        let mut caches = self.empty_caches();
        self.prime_module(&mut caches, target);
        self.prime_check(&mut caches, target);
        caches
    }

    // completion: the target's parse (local bindings + context come from its tree), document kind (a
    // script's top-level bindings complete as locals), module + checked types (typed `$`/`[["` field
    // completion), plus the memoized package-wide completion index — the global source is served from
    // that one fold, so no other file's module or naming is touched. A cursor inside a `#:` annotation
    // completes type names against the project's `@type`/`@alias` declarations, so that position
    // additionally primes every module — the same predicate the generic completion branches on, so
    // gate and feature agree.
    fn prime_completion(&self, target: DocumentId, position: TextPosition) -> Caches {
        let mut caches = self.empty_caches();
        self.prime_document(&mut caches, target);
        self.prime_document_kind(&mut caches, target);
        self.prime_module(&mut caches, target);
        self.prime_check(&mut caches, target);
        self.prime_package_naming(&mut caches);
        caches.completion_index = Some(
            self.engine
                .fetch::<Vec<GlobalCompletionEntry>>(Key::PackageCompletionIndex),
        );
        if self.target_is_annotation_body(&caches, target, position) {
            for file in caches.all_ids.clone() {
                self.prime_module(&mut caches, file);
            }
        }
        caches
    }

    // definition: identifier path primes the target + the export files of its references; the S4 path (a
    // string-literal class/generic name) instead scans every file's tree, so it primes all parses. The S4
    // discriminator reads only the target's tree+rope+point, so it needs no interner.
    fn prime_definition(&self, target: DocumentId, position: TextPosition) -> Caches {
        let mut caches = self.empty_caches();
        self.prime_document(&mut caches, target);
        self.prime_module(&mut caches, target);
        self.prime_naming(&mut caches, target);
        self.prime_package_naming(&mut caches);
        for export in self.referenced_export_files(target, &caches) {
            self.prime_document(&mut caches, export);
            self.prime_module(&mut caches, export);
            self.prime_naming(&mut caches, export);
        }
        // The S4 path (a string-literal class/generic name) scans candidate files' trees, and the
        // annotation-type-name path re-lexes every document's rope — both are cross-file text
        // scans, so only they prime the rope set (keeping ordinary goto-definition constant-scope
        // at rest). The S4 scan narrows to documents that textually mention the name
        // (`candidate_document_ids`), so the prime materializes trees for exactly that set.
        if self.target_is_s4(&caches, target, position) {
            self.prime_ropes(&mut caches);
            if let Some(document) = caches.parses.get(&target)
                && let Some(name) = generic::occurrence_prefilter_name(document, position)
            {
                for file in Self::candidate_ids(&caches, &name) {
                    self.prime_document(&mut caches, file);
                }
            }
        } else if self.target_is_annotation_type_name(&caches, target, position) {
            self.prime_ropes(&mut caches);
        }
        caches
    }

    // code actions: the target's parse (edit text/indentation), module (assignment shapes), naming
    // (unused-assignment facts), and checked types (inferred-annotation text) — a single-file scope,
    // like inlay hints plus naming.
    fn prime_code_actions(&self, target: DocumentId) -> Caches {
        let mut caches = self.empty_caches();
        self.prime_document(&mut caches, target);
        self.prime_module(&mut caches, target);
        self.prime_naming(&mut caches, target);
        self.prime_check(&mut caches, target);
        caches
    }

    // type definition: the target's parse/module/checked types resolve the cursor's nominal type;
    // the declaration scan re-lexes every document's annotation text off the always-primed ropes.
    fn prime_type_definition(&self, target: DocumentId) -> Caches {
        let mut caches = self.empty_caches();
        self.prime_document(&mut caches, target);
        self.prime_module(&mut caches, target);
        self.prime_check(&mut caches, target);
        self.prime_ropes(&mut caches);
        caches
    }

    // references / rename (whole-project scans): the target's facts plus parse + module + naming
    // of every *candidate* file — the documents whose text mentions the name under the cursor
    // (`candidate_document_ids` serves the scan from the same predicate). A position that names
    // nothing recognizable falls back to priming every file.
    fn prime_occurrence_scan(&self, target: DocumentId, position: TextPosition) -> Caches {
        let mut caches = self.empty_caches();
        self.prime_ropes(&mut caches);
        self.prime_document(&mut caches, target);
        self.prime_module(&mut caches, target);
        self.prime_naming(&mut caches, target);
        self.prime_package_naming(&mut caches);
        let candidates = match caches
            .parses
            .get(&target)
            .and_then(|document| generic::occurrence_prefilter_name(document, position))
        {
            Some(name) => Self::candidate_ids(&caches, &name),
            None => caches.all_ids.clone(),
        };
        caches.parses.reserve(candidates.len());
        caches.modules.reserve(candidates.len());
        caches.namings.reserve(candidates.len());
        for file in candidates {
            self.prime_document(&mut caches, file);
            self.prime_module(&mut caches, file);
            self.prime_naming(&mut caches, file);
        }
        caches
    }

    // ------------------------------------------------------------------------------------------------
    // Prime primitives.
    // ------------------------------------------------------------------------------------------------

    fn empty_caches(&self) -> Caches {
        Caches {
            all_ids: self.project_ids(),
            ..Caches::default()
        }
    }

    fn prime_document(&self, caches: &mut Caches, document_id: DocumentId) {
        if let std::collections::hash_map::Entry::Vacant(entry) = caches.parses.entry(document_id)
            && let Some(document) = self.engine.group().document_for(self.engine, document_id.0)
        {
            entry.insert(document);
        }
    }

    // Every document's rope, from the `SourceText` inputs directly — no tree is materialized and
    // no query body can run (inputs validate in O(1)), so this is cheap enough to prime for every
    // request. It backs `document_rope` (annotation re-lex scans) and the text prefilter behind
    // `candidate_document_ids`.
    fn prime_ropes(&self, caches: &mut Caches) {
        for document_id in caches.all_ids.clone() {
            caches.ropes.entry(document_id).or_insert_with(|| {
                self.engine
                    .fetch_optional::<SourceText>(Key::SourceText(document_id.0))
                    .map(|source| source.rope().clone())
            });
        }
    }

    // The documents a whole-project occurrence scan for `name` can touch, mirrored by
    // `EngineIdeRef::candidate_document_ids`: the primed set and the served set must agree, so both
    // filter the same ropes with the same predicate.
    fn candidate_ids(caches: &Caches, name: &str) -> Vec<DocumentId> {
        caches
            .all_ids
            .iter()
            .copied()
            .filter(|id| {
                caches.ropes.get(id).is_some_and(|rope| {
                    rope.as_ref()
                        .is_some_and(|rope| analysis::text::rope_contains(rope, name))
                })
            })
            .collect()
    }

    fn prime_module(&self, caches: &mut Caches, document_id: DocumentId) {
        caches
            .modules
            .entry(document_id)
            .or_insert_with(|| self.engine.fetch::<Module>(Key::Lower(document_id.0)));
    }

    fn prime_naming(&self, caches: &mut Caches, document_id: DocumentId) {
        caches.namings.entry(document_id).or_insert_with(|| {
            self.engine
                .fetch::<DocumentNamingComputation>(Key::LocalNaming(document_id.0))
        });
    }

    fn prime_check(&self, caches: &mut Caches, document_id: DocumentId) {
        caches.checks.entry(document_id).or_insert_with(|| {
            self.engine
                .fetch::<ModuleCheck>(Key::Typecheck(document_id.0))
        });
    }

    fn prime_document_kind(&self, caches: &mut Caches, document_id: DocumentId) {
        caches.document_kinds.entry(document_id).or_insert_with(|| {
            *self
                .engine
                .fetch::<DocumentKind>(Key::DocumentKind(document_id.0))
        });
    }

    fn project_ids(&self) -> Vec<DocumentId> {
        self.engine
            .fetch::<Vec<FileId>>(Key::ProjectFiles)
            .iter()
            .map(|file| DocumentId(*file))
            .collect()
    }

    // The package-global binding map: the `PackageSymbolIndex` memo *is* the `NamesGlobal` shape
    // `analysis::ide` expects, so priming it is a shared-pointer clone, never a per-request rebuild.
    fn prime_package_naming(&self, caches: &mut Caches) {
        caches.package_naming = Some(self.engine.fetch::<NamesGlobal>(Key::PackageSymbolIndex));
    }

    // The export files of the package globals the target references — interner-free (reads the target's
    // recorded `non_locals` symbols and the primed index). Deduped.
    fn referenced_export_files(&self, target: DocumentId, caches: &Caches) -> BTreeSet<DocumentId> {
        let naming = self
            .engine
            .fetch::<DocumentNamingComputation>(Key::LocalNaming(target.0));
        let Some(package_naming) = caches.package_naming.as_ref() else {
            return BTreeSet::new();
        };
        naming
            .naming
            .non_locals
            .values()
            .filter_map(|symbol| package_naming.global_bindings.get(symbol).copied())
            .collect()
    }

    // Whether the cursor sits on an S4 string-literal symbol (class/generic name), which the identifier
    // naming analysis never sees and `analysis::ide` resolves structurally over candidate files' trees.
    fn target_is_s4(&self, caches: &Caches, target: DocumentId, position: TextPosition) -> bool {
        caches
            .parses
            .get(&target)
            .is_some_and(|document| generic::cursor_is_s4_symbol(document, position))
    }

    fn target_is_annotation_type_name(
        &self,
        caches: &Caches,
        target: DocumentId,
        position: TextPosition,
    ) -> bool {
        caches
            .parses
            .get(&target)
            .is_some_and(|document| generic::cursor_on_annotation_type_name(document, position))
    }

    fn target_is_annotation_body(
        &self,
        caches: &Caches,
        target: DocumentId,
        position: TextPosition,
    ) -> bool {
        caches
            .parses
            .get(&target)
            .is_some_and(|document| generic::cursor_in_annotation_body(document, position))
    }
}

/// The per-call fact cache: owned `Rc` snapshots of every memo the feature reads, plus the project
/// file set. Its references back the `&T` the [`IdeDatabase`] impl returns, exactly as `Analysis`'s
/// retained maps do. Hash maps, not ordered maps: a whole-project prime (references/rename) inserts
/// every file three times, and nothing iterates these — `all_ids` carries the ordering.
#[derive(Default)]
struct Caches {
    // Owned documents (rope + tree): the input's tree for open documents, an on-demand parse
    // otherwise. Primed per feature for exactly the documents its scans read.
    parses: HashMap<DocumentId, Document>,
    // Every document's rope (`None` for a tombstoned input), always primed: text-only scans and
    // the candidate prefilter read these instead of materializing trees.
    ropes: HashMap<DocumentId, Option<Rope>>,
    modules: HashMap<DocumentId, Shared<Module>>,
    namings: HashMap<DocumentId, Shared<DocumentNamingComputation>>,
    checks: HashMap<DocumentId, Shared<ModuleCheck>>,
    document_kinds: HashMap<DocumentId, DocumentKind>,
    package_naming: Option<Shared<NamesGlobal>>,
    completion_index: Option<Shared<Vec<GlobalCompletionEntry>>>,
    all_ids: Vec<DocumentId>,
}

/// The borrowing view handed to `analysis::ide::generic::*`. Holds borrows of two disjoint stack locals —
/// the primed [`Caches`] and the `Ref<Interner>` — so it is not self-referential and triggers no `fetch`.
struct EngineIdeRef<'a> {
    caches: &'a Caches,
    interner: &'a Interner,
    paths: &'a PathTable,
    stubs: &'a StubLibrary,
}

impl<'a> EngineIdeRef<'a> {
    // `interner` is passed as `&Ref<Interner>`, which deref-coerces to `&Interner` at this argument site;
    // the orchestrator holds the `Ref` on its stack frame for the duration of the generic call.
    fn new(
        caches: &'a Caches,
        interner: &'a Interner,
        paths: &'a PathTable,
        stubs: &'a StubLibrary,
    ) -> EngineIdeRef<'a> {
        EngineIdeRef {
            caches,
            interner,
            paths,
            stubs,
        }
    }
}

impl<'a> IdeDatabase for EngineIdeRef<'a> {
    fn interner(&self) -> &Interner {
        self.interner
    }

    fn base_path(&self) -> &Path {
        self.paths.base_path()
    }

    fn document_id_for_path(&self, path: &Path) -> Option<DocumentId> {
        self.paths.id(path)
    }

    fn path_for_document_id(&self, document_id: DocumentId) -> Option<&Path> {
        self.paths.path(document_id)
    }

    fn document_by_id(&self, document_id: DocumentId) -> Option<&Document> {
        debug_assert!(
            self.caches.parses.contains_key(&document_id),
            "ide: document {document_id:?} not primed for document_by_id"
        );
        self.caches.parses.get(&document_id)
    }

    fn document_rope(&self, document_id: DocumentId) -> Option<&Rope> {
        debug_assert!(
            self.caches.ropes.contains_key(&document_id),
            "ide: document {document_id:?} not primed for document_rope"
        );
        self.caches.ropes.get(&document_id)?.as_ref()
    }

    fn candidate_document_ids(&self, name: &str) -> Vec<DocumentId> {
        debug_assert_eq!(
            self.caches.ropes.len(),
            self.caches.all_ids.len(),
            "ide: candidate filtering requires every document's rope primed"
        );
        EngineIde::candidate_ids(self.caches, name)
    }

    fn module(&self, document_id: DocumentId) -> Option<&Module> {
        debug_assert!(
            self.caches.modules.contains_key(&document_id),
            "ide: document {document_id:?} not primed for module"
        );
        self.caches
            .modules
            .get(&document_id)
            .map(|module| &**module)
    }

    fn document_naming(&self, document_id: DocumentId) -> Option<&NamesLocal> {
        debug_assert!(
            self.caches.namings.contains_key(&document_id),
            "ide: document {document_id:?} not primed for document_naming"
        );
        self.caches
            .namings
            .get(&document_id)
            .map(|naming| &naming.naming)
    }

    fn package_naming(&self) -> Option<&NamesGlobal> {
        self.caches.package_naming.as_deref()
    }

    fn checked_expression_type(
        &self,
        document_id: DocumentId,
        expression_id: ExpressionId,
    ) -> Option<&CoreType> {
        self.caches
            .checks
            .get(&document_id)?
            .expression_types_by_id
            .get(&expression_id)
    }

    fn variable_constraint(
        &self,
        document_id: DocumentId,
        variable: InferenceVariableId,
    ) -> Constraint {
        self.caches
            .checks
            .get(&document_id)
            .and_then(|check| check.variable_constraints.get(&variable))
            .copied()
            .unwrap_or(Constraint::Unconstrained)
    }

    fn all_document_ids(&self) -> Vec<DocumentId> {
        self.caches.all_ids.clone()
    }

    fn document_kind(&self, document_id: DocumentId) -> Option<DocumentKind> {
        debug_assert!(
            self.caches.document_kinds.contains_key(&document_id),
            "ide: document {document_id:?} not primed for document_kind"
        );
        self.caches.document_kinds.get(&document_id).copied()
    }

    fn stub_namespace(&self, symbol: Symbol) -> Option<Symbol> {
        self.stubs.namespace_of(symbol)
    }

    fn stub_schemes(&self) -> Vec<(Symbol, &TypeScheme)> {
        self.stubs.schemes().collect()
    }

    fn stub_overload_schemes(&self, symbol: Symbol) -> &[TypeScheme] {
        self.stubs.overload_schemes(symbol)
    }

    fn selected_overload(
        &self,
        document_id: DocumentId,
        expression_id: ExpressionId,
    ) -> Option<usize> {
        self.caches
            .checks
            .get(&document_id)?
            .selected_overloads
            .get(&expression_id)
            .copied()
    }
}
