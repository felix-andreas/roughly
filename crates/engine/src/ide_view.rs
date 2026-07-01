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
        queries::{FileId, Key, ParsedDocument, RoughlyQueries},
    },
    analysis::{
        document::{Document, DocumentId},
        hir::{ExpressionId, Module},
        ide::{
            CompletionResult, HoverInfo, IdeDatabase, InlayHint, Location, RenameResult,
            SignatureHelp, generic,
        },
        interner::{Interner, Symbol},
        naming::{DocumentNamingComputation, NamesGlobal, NamesLocal},
        text::{TextPosition, TextRange},
        typecheck::ModuleCheck,
        types::CoreType,
    },
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
        let database = EngineIdeRef::new(&caches, &interner, self.paths);
        generic::hover(&database, path, position)
    }

    pub fn inlay_hints(&self, path: &Path, viewport: Option<TextRange>) -> Vec<InlayHint> {
        let Some(target) = self.paths.id(path) else {
            return Vec::new();
        };
        let caches = self.prime_typed(target);
        let interner = self.engine.group().interner_ref();
        let database = EngineIdeRef::new(&caches, &interner, self.paths);
        generic::inlay_hints(&database, path, viewport)
    }

    pub fn signature_help(&self, path: &Path, position: TextPosition) -> Option<SignatureHelp> {
        let target = self.paths.id(path)?;
        let caches = self.prime_typed(target);
        let interner = self.engine.group().interner_ref();
        let database = EngineIdeRef::new(&caches, &interner, self.paths);
        generic::signature_help(&database, path, position)
    }

    pub fn completion(&self, path: &Path, position: TextPosition) -> Option<CompletionResult> {
        let target = self.paths.id(path)?;
        let caches = self.prime_completion(target);
        let interner = self.engine.group().interner_ref();
        let database = EngineIdeRef::new(&caches, &interner, self.paths);
        generic::completion(&database, path, position)
    }

    pub fn definition(&self, path: &Path, position: TextPosition) -> Option<Vec<Location>> {
        let target = self.paths.id(path)?;
        let caches = self.prime_definition(target, position);
        let interner = self.engine.group().interner_ref();
        let database = EngineIdeRef::new(&caches, &interner, self.paths);
        generic::definition(&database, path, position)
    }

    pub fn references(
        &self,
        path: &Path,
        position: TextPosition,
        include_declaration: bool,
    ) -> Option<Vec<Location>> {
        let _target = self.paths.id(path)?;
        let caches = self.prime_all_files();
        let interner = self.engine.group().interner_ref();
        let database = EngineIdeRef::new(&caches, &interner, self.paths);
        generic::references(&database, path, position, include_declaration)
    }

    pub fn rename(
        &self,
        path: &Path,
        position: TextPosition,
        new_name: &str,
    ) -> Option<RenameResult> {
        let _target = self.paths.id(path)?;
        let caches = self.prime_all_files();
        let interner = self.engine.group().interner_ref();
        let database = EngineIdeRef::new(&caches, &interner, self.paths);
        generic::rename(&database, path, position, new_name)
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
        self.prime_module(&mut caches, target);
        self.prime_naming(&mut caches, target);
        self.prime_check(&mut caches, target);
        let package_naming = self.synthesize_package_naming();
        for export in self.referenced_export_files(target, &package_naming) {
            self.prime_module(&mut caches, export);
            self.prime_naming(&mut caches, export);
        }
        caches.package_naming = Some(package_naming);
        caches
    }

    // inlay_hints / signature_help: the target's module + checked types. No naming, no cross-file facts.
    fn prime_typed(&self, target: DocumentId) -> Caches {
        let mut caches = self.empty_caches();
        self.prime_module(&mut caches, target);
        self.prime_check(&mut caches, target);
        caches
    }

    // completion: the target's parse (local bindings + context come from its tree), plus module+naming of
    // every package file (the global loop computes each matching global's kind from its export file). No
    // `Typecheck` at all.
    fn prime_completion(&self, target: DocumentId) -> Caches {
        let mut caches = self.empty_caches();
        self.prime_parse(&mut caches, target);
        let package_naming = self.synthesize_package_naming();
        for export in package_naming
            .global_bindings
            .values()
            .copied()
            .collect::<BTreeSet<_>>()
        {
            self.prime_module(&mut caches, export);
            self.prime_naming(&mut caches, export);
        }
        caches.package_naming = Some(package_naming);
        caches
    }

    // definition: identifier path primes the target + the export files of its references; the S4 path (a
    // string-literal class/generic name) instead scans every file's tree, so it primes all parses. The S4
    // discriminator reads only the target's tree+rope+point, so it needs no interner.
    fn prime_definition(&self, target: DocumentId, position: TextPosition) -> Caches {
        let mut caches = self.empty_caches();
        self.prime_parse(&mut caches, target);
        self.prime_module(&mut caches, target);
        self.prime_naming(&mut caches, target);
        let package_naming = self.synthesize_package_naming();
        for export in self.referenced_export_files(target, &package_naming) {
            self.prime_parse(&mut caches, export);
            self.prime_module(&mut caches, export);
            self.prime_naming(&mut caches, export);
        }
        if self.target_is_s4(target, position) {
            for file in caches.all_ids.clone() {
                self.prime_parse(&mut caches, file);
            }
        }
        caches.package_naming = Some(package_naming);
        caches
    }

    // references / rename (whole-project scans): every file's parse + module + naming, plus the package index. The
    // occurrence scan text-prefilters per identifier, but the fact scope is the whole project.
    fn prime_all_files(&self) -> Caches {
        let mut caches = self.empty_caches();
        for file in caches.all_ids.clone() {
            self.prime_parse(&mut caches, file);
            self.prime_module(&mut caches, file);
            self.prime_naming(&mut caches, file);
        }
        caches.package_naming = Some(self.synthesize_package_naming());
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

    fn prime_parse(&self, caches: &mut Caches, document_id: DocumentId) {
        caches.parses.entry(document_id).or_insert_with(|| {
            self.engine
                .fetch::<ParsedDocument>(Key::Parse(document_id.0))
        });
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

    fn project_ids(&self) -> Vec<DocumentId> {
        self.engine
            .fetch::<Vec<FileId>>(Key::ProjectFiles)
            .iter()
            .map(|file| DocumentId(*file))
            .collect()
    }

    // The package-global binding map, synthesized from `PackageSymbolIndex` (name → winning file) into the
    // `NamesGlobal` shape `analysis::ide` expects. Held owned in `Caches` so its `&NamesGlobal` is tied to
    // the cache borrow exactly like `Analysis::package_naming`'s.
    fn synthesize_package_naming(&self) -> NamesGlobal {
        let index = self
            .engine
            .fetch::<BTreeMap<Symbol, FileId>>(Key::PackageSymbolIndex);
        NamesGlobal {
            global_bindings: index
                .iter()
                .map(|(symbol, file)| (*symbol, DocumentId(*file)))
                .collect(),
        }
    }

    // The export files of the package globals the target references — interner-free (reads the target's
    // recorded `non_locals` symbols and the synthesized index). Deduped.
    fn referenced_export_files(
        &self,
        target: DocumentId,
        package_naming: &NamesGlobal,
    ) -> BTreeSet<DocumentId> {
        let naming = self
            .engine
            .fetch::<DocumentNamingComputation>(Key::LocalNaming(target.0));
        naming
            .naming
            .non_locals
            .values()
            .filter_map(|symbol| package_naming.global_bindings.get(symbol).copied())
            .collect()
    }

    // Whether the cursor sits on an S4 string-literal symbol (class/generic name), which the identifier
    // naming analysis never sees and `analysis::ide` resolves structurally over *every* file's tree.
    fn target_is_s4(&self, target: DocumentId, position: TextPosition) -> bool {
        let parsed = self.engine.fetch::<ParsedDocument>(Key::Parse(target.0));
        generic::cursor_is_s4_symbol(&parsed.0, position)
    }
}

/// The per-call fact cache: owned `Rc` snapshots of every memo the feature reads, plus the synthesized
/// package binding map and the project file set. Its references back the `&T` the [`IdeDatabase`] impl
/// returns, exactly as `Analysis`'s retained maps do.
#[derive(Default)]
struct Caches {
    parses: BTreeMap<DocumentId, Shared<ParsedDocument>>,
    modules: BTreeMap<DocumentId, Shared<Module>>,
    namings: BTreeMap<DocumentId, Shared<DocumentNamingComputation>>,
    checks: BTreeMap<DocumentId, Shared<ModuleCheck>>,
    package_naming: Option<NamesGlobal>,
    all_ids: Vec<DocumentId>,
}

/// The borrowing view handed to `analysis::ide::generic::*`. Holds borrows of two disjoint stack locals —
/// the primed [`Caches`] and the `Ref<Interner>` — so it is not self-referential and triggers no `fetch`.
struct EngineIdeRef<'a> {
    caches: &'a Caches,
    interner: &'a Interner,
    paths: &'a PathTable,
}

impl<'a> EngineIdeRef<'a> {
    // `interner` is passed as `&Ref<Interner>`, which deref-coerces to `&Interner` at this argument site;
    // the orchestrator holds the `Ref` on its stack frame for the duration of the generic call.
    fn new(caches: &'a Caches, interner: &'a Interner, paths: &'a PathTable) -> EngineIdeRef<'a> {
        EngineIdeRef {
            caches,
            interner,
            paths,
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
        self.caches.parses.get(&document_id).map(|parsed| &parsed.0)
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
        self.caches.package_naming.as_ref()
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

    fn all_document_ids(&self) -> Vec<DocumentId> {
        self.caches.all_ids.clone()
    }
}
