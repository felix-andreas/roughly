use {
    crate::{
        Interner, Symbol,
        diagnostic::Diagnostic,
        document::{Document, DocumentChange, DocumentId},
        hir::{ExpressionId, ExpressionKind, Module},
        lint::{self as lint_phase, NameStyle},
        lower::lower_with_shared_interner,
        naming::{
            DocumentKind, NamesGlobal, NamesLocal, TypeInfo, build_type_index,
            rebuild_package_naming, resolve_document_locally,
        },
        stdlib::StubLibrary,
        tree,
        typecheck::{
            ExportedValue, StrictOriginKind, StrictUnknownOrigin, TypeDefinitionEnvironment,
            inference_state_with_builtins_in_interner,
        },
        types::{Constraint, CoreType, InferenceVariableId, TypeScheme},
    },
    ropey::Rope,
    std::{
        collections::{BTreeMap, BTreeSet, HashMap, HashSet},
        fs::File,
        io::BufReader,
        path::{Path, PathBuf},
    },
    tree_sitter::Parser,
};

pub struct Analysis {
    base_path: PathBuf,
    lint_config: LintConfig,
    check_config: CheckConfig,
    parser: Parser,
    interner: Interner,
    // The standard-library stub corpus, loaded once here and never invalidated by user edits. Its
    // schemes are seeded into the per-document inference template (see `typecheck`); its symbols are a
    // base-environment input only, never entering `global_bindings` or the materialized type index (LT2).
    stub_library: StubLibrary,
    next_document_id: u32,
    next_version: Version,
    package_version: Version,
    documents: HashMap<DocumentId, Document>,
    document_versions: HashMap<DocumentId, Version>,
    document_ids_by_path: HashMap<PathBuf, DocumentId>,
    document_paths: HashMap<DocumentId, PathBuf>,
    non_package_documents: HashSet<DocumentId>,
    lint_outputs: HashMap<DocumentId, LintOutput>,
    lowering_outputs: HashMap<DocumentId, DocumentOutput<Module>>,
    document_naming_outputs: HashMap<DocumentId, DocumentOutput<NamesLocal>>,
    // Materialized package type index: each uniquely-defined type name's `TypeInfo`, plus the set of
    // names defined more than once (kept out of the resolved index, diagnosed as duplicates). Rebuilt
    // from scratch by `build_type_index` on each `resolve_package`, the type-side analog of
    // `global_bindings`; materialized for O(1) lookup during type-reference resolution.
    package_type_index: BTreeMap<Symbol, TypeInfo>,
    duplicate_type_names: BTreeSet<Symbol>,
    package_naming_output: Option<PackageOutput<NamesGlobal>>,
    document_typecheck_outputs: HashMap<DocumentId, TypecheckDocumentOutput>,
}

// Why a document was rechecked by `typecheck`. A document is attributed to its proximate cause: a
// document whose own version changed is a `BodyEdit` even if it also references a changed global,
// because its own edit, not the interface, is what forced the recheck. `InterfaceChange` carries the
// changed package-globals the document references that triggered the recheck.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalysisError {
    DocumentNotFound(PathBuf),
    ParseFailed(PathBuf),
    DocumentRead(PathBuf, String),
    InvalidEditRange(PathBuf),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct LintConfig {
    pub naming_style: Option<NameStyle>,
    pub assignment_operator: LintLevel,
    pub boolean_shorthand: LintLevel,
    pub missing_comma: LintLevel,
    pub trailing_comma: LintLevel,
}

// A lint's configured level: keep its default severity, force a severity, or disable it. The
// `[lint]` table keys each level by the lint's stable code (`assignment-operator = "off"`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LintLevel {
    #[default]
    Default,
    Off,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct CheckConfig {
    // Surface unused-local-binding warnings.
    pub unused: bool,
    // Surface type-error diagnostics. The typecheck phase still runs on demand for typing IDE
    // features (hover types, inlay hints, signature help) regardless of this flag; it only controls
    // whether type errors are reported.
    pub typing: bool,
    // Surface strict-mode diagnostics: report each site that originates a genuine `Unknown` type
    // (an unsupported construct or a reference to a binding with no known type). Like `typing`, the
    // typecheck phase computes these regardless; this flag only controls whether they are published.
    // See the strict-mode section of the typing reference.
    pub strict: bool,
}

type Version = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
struct DocumentOutput<T> {
    version: Version,
    output: T,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LintOutput {
    version: Version,
    config: LintConfig,
    diagnostics: Vec<Diagnostic>,
}

// Within-run scratch for the package-interface fixed-point: one document's exported schemes plus a
// fingerprint of them. Held only in a local map inside `typecheck` (never persisted on `Analysis`), so
// each from-scratch run starts the fixed-point empty.
#[derive(Debug, Clone, PartialEq, Eq)]
struct InterfaceOutput {
    exports: Vec<ExportedValue>,
    // Fingerprint of this document's exported schemes; the interface fixed-point converges when a round
    // changes no document's fingerprint.
    fingerprint: String,
}

// One document's authoritative typecheck result, the output `run_full` leaves on `Analysis` for callers
// (`document_diagnostics`, `checked_expression_type`) to read.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TypecheckDocumentOutput {
    diagnostics: Vec<Diagnostic>,
    // Strict-mode `Unknown`-origin diagnostics, computed alongside the type errors from the same inputs.
    // Published only when `[check] strict` is on, exactly as `diagnostics` is published only when
    // `[check] typing` is on.
    strict_diagnostics: Vec<Diagnostic>,
    expression_types: HashMap<ExpressionId, CoreType>,
    variable_constraints: BTreeMap<InferenceVariableId, Constraint>,
    selected_overloads: BTreeMap<ExpressionId, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackageOutput<T> {
    version: Version,
    output: T,
    diagnostics: HashMap<DocumentId, Vec<Diagnostic>>,
}

impl Analysis {
    pub fn new(base_path: PathBuf, lint_config: LintConfig, check_config: CheckConfig) -> Self {
        // A project may ship its own `.Rtypes` stubs under `<base_path>/stubs/` that override or extend the
        // shipped corpus. They are discovered and folded in once here; the assembled library is a
        // set-once base-environment input. Unreadable override files are skipped — reporting them is
        // the caller's concern (`roughly check` reports them; here they must never block analysis).
        let overrides = crate::stdlib::discover_project_stubs(&base_path).sources;
        Self::new_with_stub_library(base_path, lint_config, check_config, move |interner| {
            StubLibrary::load_with_overrides(interner, &overrides)
        })
    }

    // Builds an `Analysis` whose base environment comes from the stub library the `load_stubs` closure
    // produces against the interner this constructs — so the stub symbols are always interned in the
    // interner that stores them (no cross-interner mismatch is representable). `new` passes
    // `StubLibrary::load`; the LT2 zero-cost benchmark passes `|_| StubLibrary::empty()` to measure the
    // per-edit cost the stubs add.
    pub fn new_with_stub_library(
        base_path: PathBuf,
        lint_config: LintConfig,
        check_config: CheckConfig,
        load_stubs: impl FnOnce(&mut Interner) -> StubLibrary,
    ) -> Self {
        let mut interner = Interner::new();
        let stub_library = load_stubs(&mut interner);
        Self::build(base_path, lint_config, check_config, interner, stub_library)
    }

    fn build(
        base_path: PathBuf,
        lint_config: LintConfig,
        check_config: CheckConfig,
        interner: Interner,
        stub_library: StubLibrary,
    ) -> Self {
        Self {
            base_path,
            lint_config,
            check_config,
            parser: tree::new_parser().expect("typing parser should initialize"),
            interner,
            stub_library,
            next_document_id: 0,
            next_version: 1,
            package_version: 0,
            documents: HashMap::new(),
            document_versions: HashMap::new(),
            document_ids_by_path: HashMap::new(),
            document_paths: HashMap::new(),
            non_package_documents: HashSet::new(),
            lint_outputs: HashMap::new(),
            lowering_outputs: HashMap::new(),
            document_naming_outputs: HashMap::new(),
            package_type_index: BTreeMap::new(),
            duplicate_type_names: BTreeSet::new(),
            package_naming_output: None,
            document_typecheck_outputs: HashMap::new(),
        }
    }

    pub fn set_configs(&mut self, lint_config: LintConfig, check_config: CheckConfig) {
        // A check-config change (for example toggling typing) invalidates the version-keyed
        // semantic caches, so bump the package version to force the next request to recompute.
        if self.check_config != check_config {
            self.bump_package_version();
        }
        self.lint_config = lint_config;
        self.check_config = check_config;
    }

    pub fn interner(&self) -> &Interner {
        &self.interner
    }

    pub fn interner_mut(&mut self) -> &mut Interner {
        &mut self.interner
    }

    // The namespace a symbol's stub was declared in (shipped package or project stub file), for
    // showing the name's origin package on hover. `None` when the symbol is not a stub.
    pub fn stub_namespace(&self, symbol: Symbol) -> Option<Symbol> {
        self.stub_library.namespace_of(symbol)
    }

    pub fn stub_schemes(&self) -> Vec<(Symbol, &TypeScheme)> {
        self.stub_library.schemes().collect()
    }

    pub fn document_kind(&self, document_id: DocumentId) -> Option<DocumentKind> {
        if !self.documents.contains_key(&document_id) {
            return None;
        }
        Some(if self.non_package_documents.contains(&document_id) {
            DocumentKind::Script
        } else {
            DocumentKind::Package
        })
    }

    pub fn base_path(&self) -> &Path {
        &self.base_path
    }

    // Whether type-error diagnostics are surfaced. The typecheck phase still runs on demand for
    // typing IDE features regardless of this flag; it only gates publishing type errors.
    pub fn type_errors_enabled(&self) -> bool {
        self.check_config.typing
    }

    // Whether strict-mode `Unknown`-origin diagnostics are surfaced. Like `type_errors_enabled`, the
    // typecheck phase computes the strict diagnostics regardless; this only gates publishing them.
    pub fn strict_enabled(&self) -> bool {
        self.check_config.strict
    }

    pub fn add_document(&mut self, path: PathBuf, document: Document) -> DocumentId {
        let is_package_path = self.is_package_path(&path);

        if let Some(document_id) = self.document_ids_by_path.get(&path).copied() {
            let current_text = self
                .documents
                .get(&document_id)
                .map(|current_document| current_document.rope().to_string());
            let next_text = document.rope().to_string();
            let changed = current_text.as_deref() != Some(next_text.as_str());
            if changed {
                self.documents.insert(document_id, document);
            }
            if is_package_path {
                self.non_package_documents.remove(&document_id);
            } else {
                self.non_package_documents.insert(document_id);
            }
            if changed {
                let version = self.bump_version();
                self.document_versions.insert(document_id, version);
                self.bump_package_version();
                self.invalidate_document(document_id);
            }
            return document_id;
        }

        let document_id = DocumentId(self.next_document_id);
        self.next_document_id += 1;
        let version = self.bump_version();
        self.document_ids_by_path.insert(path.clone(), document_id);
        self.document_paths.insert(document_id, path);
        self.documents.insert(document_id, document);
        self.document_versions.insert(document_id, version);
        if is_package_path {
            self.non_package_documents.remove(&document_id);
        } else {
            self.non_package_documents.insert(document_id);
        }
        self.bump_package_version();
        self.invalidate_document(document_id);
        document_id
    }

    pub fn add_document_from_source(
        &mut self,
        path: PathBuf,
        source: &str,
    ) -> Result<DocumentId, AnalysisError> {
        let document = Document::parse(&mut self.parser, source)
            .map_err(|_| AnalysisError::ParseFailed(path.clone()))?;
        Ok(self.add_document(path, document))
    }

    pub fn add_document_from_disk(&mut self, path: PathBuf) -> Result<DocumentId, AnalysisError> {
        let file = File::open(&path)
            .map_err(|error| AnalysisError::DocumentRead(path.clone(), error.to_string()))?;
        let rope = Rope::from_reader(BufReader::new(file))
            .map_err(|error| AnalysisError::DocumentRead(path.clone(), error.to_string()))?;
        let tree = tree::parse_rope(&mut self.parser, &rope, None)
            .ok_or_else(|| AnalysisError::ParseFailed(path.clone()))?;
        Ok(self.add_document(path, Document::new(rope, tree)))
    }

    pub fn edit_document(
        &mut self,
        path: &Path,
        changes: &[DocumentChange],
    ) -> Result<(), AnalysisError> {
        let document_id = self
            .document_ids_by_path
            .get(path)
            .copied()
            .ok_or_else(|| AnalysisError::DocumentNotFound(path.to_path_buf()))?;
        let document = self
            .documents
            .get_mut(&document_id)
            .expect("document should exist");
        // A malformed change range is rejected without mutating the rope, but earlier changes in the
        // batch may already be applied (and the tree is always reparsed to match), so the document is
        // still coherent and its caches must be invalidated regardless before the error propagates.
        let edit_result = document.edit(&mut self.parser, changes);
        let version = self.bump_version();
        self.document_versions.insert(document_id, version);
        self.bump_package_version();
        self.invalidate_document(document_id);
        edit_result.map_err(|_| AnalysisError::InvalidEditRange(path.to_path_buf()))
    }

    pub fn delete_document(&mut self, path: &Path) -> Result<(), AnalysisError> {
        let document_id = self
            .document_ids_by_path
            .remove(path)
            .ok_or_else(|| AnalysisError::DocumentNotFound(path.to_path_buf()))?;
        self.documents.remove(&document_id);
        self.document_versions.remove(&document_id);
        self.document_paths.remove(&document_id);
        self.non_package_documents.remove(&document_id);
        self.invalidate_document(document_id);
        self.bump_package_version();
        Ok(())
    }

    pub fn document(&self, path: &Path) -> Option<&Document> {
        self.document_ids_by_path
            .get(path)
            .and_then(|document_id| self.documents.get(document_id))
    }

    pub fn document_by_id(&self, document_id: DocumentId) -> Option<&Document> {
        self.documents.get(&document_id)
    }

    pub fn document_id_for_path(&self, path: &Path) -> Option<DocumentId> {
        self.document_ids_by_path.get(path).copied()
    }

    pub fn path_for_document_id(&self, document_id: DocumentId) -> Option<&Path> {
        self.document_paths.get(&document_id).map(PathBuf::as_path)
    }

    pub fn module(&self, document_id: DocumentId) -> Option<&Module> {
        self.lowering_outputs
            .get(&document_id)
            .map(|output| &output.output)
    }

    pub fn document_naming(&self, document_id: DocumentId) -> Option<&NamesLocal> {
        self.document_naming_outputs
            .get(&document_id)
            .map(|output| &output.output)
    }

    pub fn package_naming(&self) -> Option<&NamesGlobal> {
        self.package_naming_output
            .as_ref()
            .map(|output| &output.output)
    }

    pub fn package_document_ids(&self) -> Vec<DocumentId> {
        let mut document_ids = self
            .documents
            .keys()
            .copied()
            .filter(|document_id| !self.non_package_documents.contains(document_id))
            .collect::<Vec<_>>();
        document_ids.sort_by(|left_document_id, right_document_id| {
            self.package_path_key(*left_document_id)
                .cmp(&self.package_path_key(*right_document_id))
        });
        document_ids
    }

    // The package-relative, slash-normalized path used to order package documents. This is the single
    // source of truth for package document order: `package_document_ids` sorts by it, and last-writer-wins
    // winner selection for a duplicated definition takes the path-last definer by it. Winner selection
    // must use this, never `DocumentId` numeric order.
    fn package_path_key(&self, document_id: DocumentId) -> String {
        self.path_for_document_id(document_id)
            .map(|path| {
                path.strip_prefix(&self.base_path)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .unwrap_or_default()
    }

    pub fn lint_diagnostics(&self, document_id: DocumentId) -> Vec<Diagnostic> {
        self.lint_outputs
            .get(&document_id)
            .map(|output| output.diagnostics.clone())
            .unwrap_or_default()
    }

    pub fn document_diagnostics(&self, document_id: DocumentId) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        if let Some(output) = self.lint_outputs.get(&document_id) {
            diagnostics.extend(output.diagnostics.iter().cloned());
        }
        if let Some(output) = self.lowering_outputs.get(&document_id) {
            diagnostics.extend(output.diagnostics.iter().cloned());
        }
        if let Some(output) = self.document_naming_outputs.get(&document_id) {
            diagnostics.extend(output.diagnostics.iter().cloned());
            if self.check_config.unused {
                diagnostics.extend(crate::naming::unused_diagnostics(
                    &output.output,
                    self.interner(),
                ));
            }
        }
        if let Some(output) = &self.package_naming_output
            && let Some(package_diagnostics) = output.diagnostics.get(&document_id)
        {
            diagnostics.extend(package_diagnostics.iter().cloned());
        }
        // A file's own `#: @strict` directive overrides the configured default.
        let strict_enabled = self
            .lowering_outputs
            .get(&document_id)
            .and_then(|lowering| lowering.output.strict_override)
            .unwrap_or(self.check_config.strict);
        if let Some(output) = self.document_typecheck_outputs.get(&document_id) {
            if self.check_config.typing {
                diagnostics.extend(output.diagnostics.iter().cloned());
            }
            if strict_enabled {
                diagnostics.extend(output.strict_diagnostics.iter().cloned());
            }
        }
        if strict_enabled {
            for diagnostic in &mut diagnostics {
                diagnostic.escalate_unresolved_to_error();
            }
        }
        if let Some(document) = self.document_by_id(document_id) {
            diagnostics =
                crate::diagnostic::apply_suppressions(diagnostics, &document.rope().to_string());
        }
        diagnostics
    }

    // The checked type of an expression, available once `typecheck` has run for the document.
    // Used by hover and inlay hints to show inferred types.
    pub fn checked_expression_type(
        &self,
        document_id: DocumentId,
        expression_id: ExpressionId,
    ) -> Option<&CoreType> {
        self.document_typecheck_outputs
            .get(&document_id)?
            .expression_types
            .get(&expression_id)
    }

    // The constraint carried by a still-unbound inference variable in a stored expression type,
    // for display-time generalization (`<T: numeric>` on hover).
    pub fn variable_constraint(
        &self,
        document_id: DocumentId,
        variable: InferenceVariableId,
    ) -> Constraint {
        self.document_typecheck_outputs
            .get(&document_id)
            .and_then(|output| output.variable_constraints.get(&variable))
            .copied()
            .unwrap_or(Constraint::Unconstrained)
    }

    // The declared-set index of the overload a call committed, keyed by the callee expression;
    // `None` when the callee did not resolve to a stub overload set (or the call never matched).
    pub fn selected_overload(
        &self,
        document_id: DocumentId,
        expression_id: ExpressionId,
    ) -> Option<usize> {
        self.document_typecheck_outputs
            .get(&document_id)?
            .selected_overloads
            .get(&expression_id)
            .copied()
    }

    // The full declared overload set of a stub name, in declaration order; empty for non-stubs.
    pub fn stub_overload_schemes(&self, symbol: Symbol) -> &[TypeScheme] {
        self.stub_library.overload_schemes(symbol)
    }
}

// Returns the documents whose typecheck output changed, so callers can republish exactly
// those diagnostics after a package-visible edit.
pub fn run_full(analysis_state: &mut Analysis) -> Vec<DocumentId> {
    lint(analysis_state);

    if analysis_state.check_config.typing || analysis_state.check_config.strict {
        typecheck(analysis_state)
    } else {
        resolve_package(analysis_state);
        Vec::new()
    }
}

pub fn lint(analysis_state: &mut Analysis) {
    let document_ids = analysis_state.all_document_ids();
    let config = analysis_state.lint_config;

    for document_id in &document_ids {
        let Some(document_version) = analysis_state.document_version(*document_id) else {
            continue;
        };
        if analysis_state
            .lint_outputs
            .get(document_id)
            .is_some_and(|output| output.version == document_version && output.config == config)
        {
            continue;
        }

        let Some(document) = analysis_state.document_by_id(*document_id) else {
            continue;
        };
        analysis_state.lint_outputs.insert(
            *document_id,
            LintOutput {
                version: document_version,
                config,
                diagnostics: lint_phase::analyze(document, config),
            },
        );
    }
}

pub fn lower(analysis_state: &mut Analysis) {
    let document_ids = analysis_state.all_document_ids();

    for document_id in &document_ids {
        let Some(document_version) = analysis_state.document_version(*document_id) else {
            continue;
        };
        if analysis_state
            .lowering_outputs
            .get(document_id)
            .is_some_and(|output| output.version == document_version)
        {
            continue;
        }

        let Some(document) = analysis_state.documents.get(document_id).cloned() else {
            continue;
        };
        let lowering_result = lower_with_shared_interner(&document, analysis_state.interner_mut());
        analysis_state.lowering_outputs.insert(
            *document_id,
            DocumentOutput {
                version: document_version,
                output: lowering_result.module,
                diagnostics: lowering_result.diagnostics,
            },
        );
    }
}

pub fn resolve_package(analysis_state: &mut Analysis) {
    lower(analysis_state);

    let document_ids = analysis_state.all_document_ids();
    for document_id in &document_ids {
        let Some(document_version) = analysis_state.document_version(*document_id) else {
            continue;
        };
        if analysis_state
            .document_naming_outputs
            .get(document_id)
            .is_some_and(|output| output.version == document_version)
        {
            continue;
        }

        let module = analysis_state.module(*document_id).unwrap_or_else(|| {
            panic!("missing lowered module for document naming {document_id:?}")
        });
        let document_kind = if analysis_state.non_package_documents.contains(document_id) {
            DocumentKind::Script
        } else {
            DocumentKind::Package
        };
        let local_naming = resolve_document_locally(
            *document_id,
            module,
            analysis_state.interner(),
            document_kind,
        );
        analysis_state.document_naming_outputs.insert(
            *document_id,
            DocumentOutput {
                version: document_version,
                output: local_naming.naming,
                diagnostics: local_naming.diagnostics,
            },
        );
    }

    // From-scratch package naming + materialized type index, rebuilt unconditionally: `analysis` is the
    // from-scratch oracle/CLI checker (the engine owns incrementality), so there is no cross-call cache
    // to consult and no incremental mirror to maintain or assert against.
    let package_document_ids = analysis_state.package_document_ids();
    let package_modules = package_document_ids
        .iter()
        .filter_map(|document_id| {
            analysis_state
                .module(*document_id)
                .map(|module| (*document_id, module))
        })
        .collect::<Vec<_>>();
    let extra_modules = analysis_state
        .all_document_ids()
        .into_iter()
        .filter(|document_id| !package_document_ids.contains(document_id))
        .filter_map(|document_id| {
            analysis_state
                .module(document_id)
                .map(|module| (document_id, module))
        })
        .collect::<Vec<_>>();
    let naming_locals = analysis_state
        .document_naming_outputs
        .iter()
        .map(|(document_id, output)| (*document_id, output.output.clone()))
        .collect::<HashMap<_, _>>();
    // The package-naming diagnostics narrow a type-reference error to the offending name by re-lexing
    // the document, so the rebuild needs each document's text. Rope clones share structure, so this is cheap.
    let ropes = analysis_state
        .all_document_ids()
        .into_iter()
        .filter_map(|document_id| {
            analysis_state
                .document_by_id(document_id)
                .map(|document| (document_id, document.rope().clone()))
        })
        .collect::<HashMap<_, _>>();

    let (package_type_index, duplicate_type_names) = build_type_index(&package_modules);
    let computation = rebuild_package_naming(
        &package_modules,
        &extra_modules,
        &naming_locals,
        &ropes,
        analysis_state.interner(),
        &analysis_state.stub_library,
    );
    // The module borrows are released before mutating the owning state.
    drop(package_modules);
    drop(extra_modules);

    let package_version = analysis_state.package_version;
    analysis_state.package_type_index = package_type_index;
    analysis_state.duplicate_type_names = duplicate_type_names;
    analysis_state.package_naming_output = Some(PackageOutput {
        version: package_version,
        output: computation.naming,
        diagnostics: computation.diagnostics,
    });
}

fn render_interface_fingerprint(exports: &[ExportedValue], interner: &Interner) -> String {
    exports
        .iter()
        .map(|export| {
            format!(
                "{}: {}",
                interner.resolve(export.symbol).unwrap_or("<unknown>"),
                crate::diagnostic::render_type_scheme(interner, &export.type_scheme)
            )
        })
        .collect::<Vec<_>>()
        .join(";")
}

// The package-global interface table: the winning document's exported scheme for each package-global
// name. Names whose winning document has not produced an export yet resolve to `Unknown`, which is
// how the fixed-point starts before re-exports and forward references fill in.
fn build_package_interface_table(
    package_naming: &NamesGlobal,
    interface_outputs: &HashMap<DocumentId, InterfaceOutput>,
    fallback_range: tree_sitter::Range,
) -> std::collections::BTreeMap<crate::Symbol, ExportedValue> {
    let mut table = std::collections::BTreeMap::new();
    for (symbol, winner_document_id) in &package_naming.global_bindings {
        let export = interface_outputs
            .get(winner_document_id)
            .and_then(|interface| {
                interface
                    .exports
                    .iter()
                    .find(|export| export.symbol == *symbol)
                    .cloned()
            })
            .unwrap_or_else(|| ExportedValue {
                symbol: *symbol,
                type_scheme: TypeScheme::monomorphic(CoreType::Unknown),
                range: fallback_range,
            });
        table.insert(*symbol, export);
    }
    table
}

// From-scratch whole-package typecheck: the one-shot CLI path and the differential oracle the
// incremental engine is checked against. Round 1 computes each package document's exported value
// schemes in isolation (cross-file references check as `Unknown`); the interface fixed-point then
// converges the package table; finally every document is checked against the converged table.
// Nothing here is cached across calls — per-document output maps are within-run scratch (never
// call `typecheck` twice on one `Analysis`). Returns the documents whose typecheck output was
// computed, so callers can publish exactly those diagnostics.
pub fn typecheck(analysis_state: &mut Analysis) -> Vec<DocumentId> {
    resolve_package(analysis_state);

    let package_document_ids = analysis_state.package_document_ids();
    let all_document_ids = analysis_state.all_document_ids();
    let mut template_state =
        inference_state_with_builtins_in_interner(analysis_state.interner_mut());
    // Seed the stdlib stub schemes as base globals alongside the builtins, so a document cloned from
    // this template resolves a bare base name (`length`, `T`, `pi`, ...) to its stub scheme. These live
    // only in the template environment, never in `global_bindings` or the interface table.
    analysis_state.stub_library.seed_into(&mut template_state);
    let fallback_range = analysis_state.fallback_range();

    let type_definitions = {
        let package_modules = package_document_ids
            .iter()
            .map(|document_id| {
                analysis_state.module(*document_id).unwrap_or_else(|| {
                    panic!("missing lowered module for typecheck {document_id:?}")
                })
            })
            .collect::<Vec<_>>();
        let mut type_definitions =
            TypeDefinitionEnvironment::from_modules(package_modules.iter().copied());
        analysis_state
            .stub_library
            .seed_type_definitions(&mut type_definitions);
        type_definitions
    };

    let package_naming = analysis_state
        .package_naming_output
        .as_ref()
        .unwrap_or_else(|| panic!("missing package naming output for typecheck"))
        .output
        .clone();

    let package_document_set = package_document_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();

    // Per-document exported interface — scratch for the fixed-point only. Local (never a persisted
    // `Analysis` field), so each from-scratch `typecheck` starts the iteration from an empty table.
    let mut document_interface_outputs: HashMap<DocumentId, InterfaceOutput> = HashMap::new();

    // Package interface fixed-point. Every package document is re-inferred each round against the current
    // interface table; the loop converges when a round changes no document's exported interface. Each
    // round's table is the previous round's output, so an acyclic re-export/forward-reference chain
    // progresses monotonically (each name's exported scheme transitions at most once, `Unknown` →
    // concrete), reaching the fixed point in at most `#package-globals + 1` rounds. A genuine re-export
    // cycle (`a <- b`, `b <- a`) is non-monotone — its members' schemes swap every round — so the
    // oscillation guard pins such a name to `Unknown`, collapsing the cycle and restoring monotonicity.
    // The `+ 8` is slack so the convergence `debug_assert` cannot false-fire on legitimate input.
    let max_package_interface_rounds = package_naming.global_bindings.len().saturating_add(8);
    let mut pinned_unknown = BTreeSet::<Symbol>::new();
    let mut symbol_value_history = HashMap::<Symbol, Vec<String>>::new();
    let mut converged = false;
    for _round in 0..max_package_interface_rounds {
        let mut table = build_package_interface_table(
            &package_naming,
            &document_interface_outputs,
            fallback_range,
        );
        for symbol in &pinned_unknown {
            if let Some(export) = table.get_mut(symbol) {
                export.type_scheme = TypeScheme::monomorphic(CoreType::Unknown);
            }
        }

        // Oscillation guard: a name whose table rendering returns to a value it already had in an earlier
        // round while differing from the previous round is swapping on a re-export cycle, not progressing
        // monotonically; pin it to the conservative `Unknown`. An acyclic name's rendering only ever
        // transitions away from `Unknown` and never returns, so it is never pinned.
        let mut newly_pinned = Vec::new();
        for symbol in package_naming.global_bindings.keys() {
            if pinned_unknown.contains(symbol) {
                continue;
            }
            let Some(export) = table.get(symbol) else {
                continue;
            };
            let rendered = crate::diagnostic::render_type_scheme(
                analysis_state.interner(),
                &export.type_scheme,
            );
            let history = symbol_value_history.entry(*symbol).or_default();
            if history.last().is_some_and(|last| *last != rendered) && history.contains(&rendered) {
                newly_pinned.push(*symbol);
            } else {
                history.push(rendered);
            }
        }
        let pinned_this_round = !newly_pinned.is_empty();
        for symbol in newly_pinned {
            pinned_unknown.insert(symbol);
            if let Some(export) = table.get_mut(&symbol) {
                export.type_scheme = TypeScheme::monomorphic(CoreType::Unknown);
            }
        }

        let mut any_interface_changed = false;
        let mut fresh_interfaces = Vec::new();
        for document_id in &package_document_set {
            let local_naming = analysis_state
                .document_naming(*document_id)
                .unwrap_or_else(|| panic!("missing local naming for typecheck {document_id:?}"));
            let referenced = local_naming
                .non_locals
                .values()
                .copied()
                .collect::<BTreeSet<_>>();
            let previous_fingerprint = document_interface_outputs
                .get(document_id)
                .map(|output| output.fingerprint.clone());

            let module = analysis_state
                .module(*document_id)
                .unwrap_or_else(|| panic!("missing lowered module for typecheck {document_id:?}"));
            let mut inference_state = template_state.clone();
            for symbol in &referenced {
                if let Some(export) = table.get(symbol) {
                    let imported_scheme = inference_state.import_scheme(&export.type_scheme);
                    inference_state.bind_global_scheme(*symbol, imported_scheme, export.range);
                }
            }
            let _ = inference_state.check_module_with_naming(
                *document_id,
                module,
                local_naming,
                &package_naming,
                &type_definitions,
            );
            let exports = inference_state.exported_value_schemes(module, local_naming);
            let fingerprint = render_interface_fingerprint(&exports, analysis_state.interner());
            if previous_fingerprint.as_deref() != Some(fingerprint.as_str()) {
                any_interface_changed = true;
            }
            fresh_interfaces.push((
                *document_id,
                InterfaceOutput {
                    exports,
                    fingerprint,
                },
            ));
        }
        for (document_id, output) in fresh_interfaces {
            document_interface_outputs.insert(document_id, output);
        }

        if !any_interface_changed && !pinned_this_round {
            converged = true;
            break;
        }
    }
    // The bound is sized so every legitimate package converges, so non-convergence is a genuine
    // fixed-point defect (e.g. a non-monotone exported scheme), not a deep chain: fail loudly in
    // debug/test builds.
    debug_assert!(
        converged,
        "package interface fixed-point did not converge within {max_package_interface_rounds} rounds; \
         this indicates a fixed-point defect (e.g. a non-monotone exported scheme), not a deep chain"
    );

    // The converged package interface table. It supplies the schemes bound when each document runs its
    // authoritative round-2 check; names pinned during the fixed-point are forced to `Unknown` here too.
    let mut final_table =
        build_package_interface_table(&package_naming, &document_interface_outputs, fallback_range);
    for symbol in &pinned_unknown {
        if let Some(export) = final_table.get_mut(symbol) {
            export.type_scheme = TypeScheme::monomorphic(CoreType::Unknown);
        }
    }

    // Round 2: the authoritative per-document check over every document, recording expression types for
    // hover and inlay hints.
    let mut checked_document_ids = Vec::new();
    let mut fresh_outputs = Vec::new();
    for document_id in &all_document_ids {
        let local_naming = analysis_state
            .document_naming(*document_id)
            .unwrap_or_else(|| panic!("missing local naming for typecheck {document_id:?}"));
        let referenced_symbols = local_naming
            .non_locals
            .values()
            .copied()
            .collect::<BTreeSet<_>>();
        let module = analysis_state
            .module(*document_id)
            .unwrap_or_else(|| panic!("missing lowered module for typecheck {document_id:?}"));
        // Script-local type declarations are visible only inside the script itself and shadow package
        // definitions of the same name.
        let document_type_definitions =
            if analysis_state.non_package_documents.contains(document_id) {
                let package_modules = package_document_ids
                    .iter()
                    .filter_map(|package_document_id| analysis_state.module(*package_document_id));
                let mut script_type_definitions =
                    TypeDefinitionEnvironment::from_modules(package_modules.chain([module]));
                analysis_state
                    .stub_library
                    .seed_type_definitions(&mut script_type_definitions);
                script_type_definitions
            } else {
                type_definitions.clone()
            };
        let mut inference_state = template_state.clone();
        for symbol in &referenced_symbols {
            if let Some(export) = final_table.get(symbol) {
                let imported_scheme = inference_state.import_scheme(&export.type_scheme);
                inference_state.bind_global_scheme(*symbol, imported_scheme, export.range);
            }
        }
        inference_state.enable_expression_type_recording();
        let module_check = inference_state.check_module_with_naming(
            *document_id,
            module,
            local_naming,
            &package_naming,
            &document_type_definitions,
        );
        let diagnostics = module_check
            .errors
            .iter()
            .map(|error| {
                Diagnostic::from_inference_error(error, fallback_range, analysis_state.interner())
            })
            .collect();
        let strict_diagnostics = strict_origin_diagnostics(
            module,
            &module_check.strict_origins,
            analysis_state.interner(),
        );
        let expression_types = module_check
            .expression_types_by_id
            .into_iter()
            .collect::<HashMap<_, _>>();
        fresh_outputs.push((
            *document_id,
            TypecheckDocumentOutput {
                diagnostics,
                strict_diagnostics,
                expression_types,
                variable_constraints: module_check.variable_constraints,
                selected_overloads: module_check.selected_overloads,
            },
        ));
        checked_document_ids.push(*document_id);
    }
    for (document_id, output) in fresh_outputs {
        analysis_state
            .document_typecheck_outputs
            .insert(document_id, output);
    }

    checked_document_ids
}

// Renders each strict `Unknown` origin into a diagnostic. An origin that is the value of an
// assignment is phrased against the bound name (the actionable fix is to annotate that binding);
// any other origin is phrased against the expression itself.
pub fn strict_origin_diagnostics(
    module: &Module,
    strict_origins: &[StrictUnknownOrigin],
    interner: &Interner,
) -> Vec<Diagnostic> {
    if strict_origins.is_empty() {
        return Vec::new();
    }
    let mut assignment_targets = HashMap::new();
    for expression in module.arena.expressions() {
        if let ExpressionKind::Assign { value, .. } = &expression.kind
            && let Some(target) = expression.kind.assignment_variable()
        {
            assignment_targets.insert(*value, target);
        }
    }
    strict_origins
        .iter()
        .map(|origin| {
            // A loop-widened origin is about the named variable, never about the expression the
            // loop happens to be the value of, so it skips the assignment-value phrasing.
            let assignment_target = match &origin.kind {
                StrictOriginKind::LoopWidened(_) => None,
                _ => assignment_targets.get(&origin.expression_id),
            };
            let message = if let Some(target) = assignment_target {
                let name = interner.resolve(*target).unwrap_or("<unknown>");
                format!("strict mode: could not determine the type of `{name}`; add a type annotation")
            } else {
                match &origin.kind {
                    StrictOriginKind::UnsupportedConstruct => {
                        "strict mode: this expression has an undetermined type (`Unknown`)".to_owned()
                    }
                    StrictOriginKind::UndeterminedReference(symbol) => {
                        let name = interner.resolve(*symbol).unwrap_or("<unknown>");
                        format!(
                            "strict mode: could not determine the type of `{name}`; it has no known type"
                        )
                    }
                    StrictOriginKind::LoopWidened(symbol) => {
                        let name = interner.resolve(*symbol).unwrap_or("<unknown>");
                        format!(
                            "strict mode: could not determine the type of `{name}`; its type does not stabilize across loop iterations — add a type annotation"
                        )
                    }
                }
            };
            Diagnostic::strict(origin.range, message)
        })
        .collect()
}

impl Analysis {
    fn is_package_path(&self, path: &Path) -> bool {
        path.starts_with(self.base_path.join("R"))
    }

    pub fn all_document_ids(&self) -> Vec<DocumentId> {
        let mut document_ids = self.documents.keys().copied().collect::<Vec<_>>();
        document_ids.sort_by_key(|document_id| document_id.0);
        document_ids
    }

    fn document_version(&self, document_id: DocumentId) -> Option<Version> {
        self.document_versions.get(&document_id).copied()
    }

    fn invalidate_document(&mut self, document_id: DocumentId) {
        // Drops the document's version-keyed phase caches so the next `run_full` re-derives it from
        // scratch. Every add/edit/delete funnels through here.
        self.lint_outputs.remove(&document_id);
        self.lowering_outputs.remove(&document_id);
        self.document_naming_outputs.remove(&document_id);
        self.document_typecheck_outputs.remove(&document_id);
    }

    fn bump_version(&mut self) -> Version {
        let version = self.next_version;
        self.next_version += 1;
        version
    }

    fn bump_package_version(&mut self) {
        self.package_version = self.bump_version();
    }

    fn fallback_range(&self) -> tree_sitter::Range {
        for document_id in self.package_document_ids() {
            let Some(document) = self.documents.get(&document_id) else {
                continue;
            };
            let line = document
                .rope()
                .get_line(0)
                .map(|line| line.to_string())
                .unwrap_or_default();
            return tree_sitter::Range {
                start_byte: 0,
                end_byte: line.len(),
                start_point: tree_sitter::Point { row: 0, column: 0 },
                end_point: tree_sitter::Point {
                    row: 0,
                    column: line.len(),
                },
            };
        }

        tree_sitter::Range {
            start_byte: 0,
            end_byte: 0,
            start_point: tree_sitter::Point { row: 0, column: 0 },
            end_point: tree_sitter::Point { row: 0, column: 0 },
        }
    }
}

#[cfg(test)]
mod tests {
    use {
        super::{
            Analysis, CheckConfig, DocumentChange, LintConfig, lint, lower, resolve_package,
            run_full,
        },
        crate::{
            Severity,
            lint::NameStyle,
            text::{TextPosition, TextRange},
        },
        std::{
            fs,
            path::{Path, PathBuf},
            sync::atomic::{AtomicU64, Ordering},
            time::{SystemTime, UNIX_EPOCH},
        },
    };

    #[test]
    fn package_document_ids_exclude_non_package_paths() {
        let mut analysis = Analysis::new(
            PathBuf::from("/workspace"),
            LintConfig::default(),
            CheckConfig::default(),
        );
        let package_document_id = analysis
            .add_document_from_source(PathBuf::from("/workspace/R/main.R"), "value <- 1L")
            .expect("package document should parse");
        analysis
            .add_document_from_source(
                PathBuf::from("/workspace/tests/test-value.R"),
                "helper <- 1L",
            )
            .expect("non-package document should parse");

        assert_eq!(analysis.package_document_ids(), vec![package_document_id]);
    }

    #[test]
    fn package_document_ids_follow_package_path_order() {
        let mut analysis = Analysis::new(
            PathBuf::from("/workspace"),
            LintConfig::default(),
            CheckConfig::default(),
        );
        let first_document_id = analysis
            .add_document_from_source(PathBuf::from("/workspace/R/zeta.R"), "zeta <- 1L")
            .expect("package document should parse");
        let second_document_id = analysis
            .add_document_from_source(PathBuf::from("/workspace/R/alpha.R"), "alpha <- 1L")
            .expect("package document should parse");

        assert_eq!(
            analysis.package_document_ids(),
            vec![second_document_id, first_document_id]
        );
    }

    #[test]
    fn edit_document_invalidates_lowering_and_naming_state() {
        let path = PathBuf::from("/workspace/R/main.R");
        let mut analysis = Analysis::new(
            PathBuf::from("/workspace"),
            LintConfig::default(),
            CheckConfig::default(),
        );
        let document_id = analysis
            .add_document_from_source(path.clone(), "value <- 1L")
            .expect("document should parse");

        lower(&mut analysis);
        resolve_package(&mut analysis);

        assert!(analysis.module(document_id).is_some());
        assert!(analysis.document_naming(document_id).is_some());

        analysis
            .edit_document(
                &path,
                &[DocumentChange {
                    range: TextRange {
                        start: TextPosition {
                            line_index: 0,
                            character_index: 0,
                        },
                        end: TextPosition {
                            line_index: 0,
                            character_index: "value <- 1L".len(),
                        },
                    },
                    text: "value <-".to_owned(),
                }],
            )
            .expect("edit should succeed");

        assert!(analysis.module(document_id).is_none());
        assert!(analysis.document_naming(document_id).is_none());

        lower(&mut analysis);
        let diagnostics = analysis.document_diagnostics(document_id);

        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error)
        );
    }

    #[test]
    fn delete_document_removes_cached_state() {
        let path = PathBuf::from("/workspace/R/main.R");
        let mut analysis = Analysis::new(
            PathBuf::from("/workspace"),
            LintConfig::default(),
            CheckConfig::default(),
        );
        let document_id = analysis
            .add_document_from_source(path.clone(), "value <- 1L")
            .expect("document should parse");

        lower(&mut analysis);

        assert!(analysis.document(&path).is_some());
        assert!(analysis.module(document_id).is_some());

        analysis
            .delete_document(&path)
            .expect("delete should succeed");

        assert!(analysis.document(&path).is_none());
        assert!(analysis.document_id_for_path(&path).is_none());
        assert!(analysis.module(document_id).is_none());
        assert!(analysis.package_document_ids().is_empty());
    }

    #[test]
    fn run_full_runs_naming_even_when_lowering_reports_errors() {
        let path = PathBuf::from("/workspace/R/main.R");
        let mut analysis = Analysis::new(
            PathBuf::from("/workspace"),
            LintConfig::default(),
            CheckConfig::default(),
        );
        let document_id = analysis
            .add_document_from_source(path, "value <-")
            .expect("document should parse");

        run_full(&mut analysis);
        let result = analysis.document_diagnostics(document_id);

        assert!(
            result
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error)
        );
        assert!(analysis.module(document_id).is_some());
        assert!(analysis.document_naming(document_id).is_some());
    }

    #[test]
    fn lint_is_retained_in_analysis_and_reruns_when_config_changes() {
        let path = PathBuf::from("/workspace/R/main.R");
        let mut analysis = Analysis::new(
            PathBuf::from("/workspace"),
            LintConfig {
                naming_style: Some(NameStyle::Snake),
                ..LintConfig::default()
            },
            CheckConfig::default(),
        );
        let document_id = analysis
            .add_document_from_source(path.clone(), "value <- function(snake_name) snake_name")
            .expect("document should parse");

        lint(&mut analysis);
        assert!(analysis.lint_diagnostics(document_id).is_empty());

        analysis.set_configs(
            LintConfig {
                naming_style: Some(NameStyle::Camel),
                ..LintConfig::default()
            },
            CheckConfig::default(),
        );
        lint(&mut analysis);
        assert!(
            analysis
                .lint_diagnostics(document_id)
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Warning)
        );
    }

    #[test]
    fn package_diagnostics_remain_retained_until_package_resolution_reruns() {
        let path = PathBuf::from("/workspace/R/main.R");
        let mut analysis = Analysis::new(
            PathBuf::from("/workspace"),
            LintConfig::default(),
            CheckConfig::default(),
        );
        let document_id = analysis
            .add_document_from_source(path.clone(), "zzz_unknown")
            .expect("document should parse");

        resolve_package(&mut analysis);
        let retained_naming = analysis.document_diagnostics(document_id);
        assert!(
            retained_naming
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Warning)
        );

        analysis
            .edit_document(
                &path,
                &[DocumentChange {
                    range: TextRange {
                        start: TextPosition {
                            line_index: 0,
                            character_index: 0,
                        },
                        end: TextPosition {
                            line_index: 0,
                            character_index: "zzz_unknown".len(),
                        },
                    },
                    text: "zzz_unknown(".to_owned(),
                }],
            )
            .expect("edit should succeed");
        lower(&mut analysis);

        let retained_diagnostics = analysis.document_diagnostics(document_id);
        assert!(
            retained_diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error)
        );
        assert!(
            retained_diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Warning)
        );
    }

    #[test]
    fn type_errors_are_gated_on_config_but_typed_info_is_retained() {
        // typing off (default): the typecheck phase still runs and retains its output (so IDE
        // features can read checked types), but the type error is not surfaced as a diagnostic.
        let mut analysis = Analysis::new(
            PathBuf::from("/workspace"),
            LintConfig::default(),
            CheckConfig::default(),
        );
        let document_id = analysis
            .add_document_from_source(PathBuf::from("/workspace/R/main.R"), "1L + \"text\"")
            .expect("document should parse");
        super::typecheck(&mut analysis);
        assert!(
            analysis.document_diagnostics(document_id).is_empty(),
            "type errors must be suppressed when `[check] typing` is off"
        );
        assert!(
            analysis
                .document_typecheck_outputs
                .contains_key(&document_id),
            "the typecheck phase must still run and retain output for IDE features"
        );

        // typing on: the type error is surfaced as a diagnostic.
        let mut analysis = Analysis::new(
            PathBuf::from("/workspace"),
            LintConfig::default(),
            CheckConfig {
                unused: false,
                typing: true,
                strict: false,
            },
        );
        let document_id = analysis
            .add_document_from_source(PathBuf::from("/workspace/R/main.R"), "1L + \"text\"")
            .expect("document should parse");
        super::typecheck(&mut analysis);
        assert!(
            !analysis.document_diagnostics(document_id).is_empty(),
            "type errors must be surfaced when `[check] typing` is on"
        );
    }

    #[test]
    fn reload_document_from_disk_replaces_loaded_package_file() {
        let workspace_path = unique_temp_workspace_path();
        let package_root = workspace_path.join("R");
        let document_path = package_root.join("main.R");
        fs::create_dir_all(&package_root).expect("workspace package root should be created");
        fs::write(&document_path, "value <- 1L\n").expect("document should be written");

        let mut analysis = Analysis::new(
            workspace_path.clone(),
            LintConfig::default(),
            CheckConfig::default(),
        );
        analysis
            .add_document_from_source(document_path.clone(), "value <- 1L\n")
            .expect("document should parse");

        fs::write(&document_path, "value <- 2L\n").expect("document should be updated");
        analysis
            .add_document_from_disk(document_path.clone())
            .expect("reload should succeed");
        assert_eq!(
            analysis
                .document(&document_path)
                .expect("document should be present")
                .rope()
                .to_string(),
            "value <- 2L\n"
        );

        remove_workspace_path(&workspace_path);
    }

    #[test]
    fn project_stub_overrides_shipped_stub_of_the_same_name() {
        // The shipped corpus types `nchar` as `fn(x: character) -> integer`, so annotating its result
        // as `character` is a type error. A project stub that redeclares `nchar` to return `character`
        // must win, making the same document type-check clean — proving project overrides are wired
        // through construction end to end.
        let typing = CheckConfig {
            typing: true,
            ..CheckConfig::default()
        };
        let source = "#: character\nresult <- nchar(\"hi\")\n";
        let document_relative = "R/main.R";

        let shipped_workspace = unique_temp_workspace_path();
        fs::create_dir_all(shipped_workspace.join("R")).expect("package root");
        let mut shipped = Analysis::new(shipped_workspace.clone(), LintConfig::default(), typing);
        let shipped_document = shipped
            .add_document_from_source(shipped_workspace.join(document_relative), source)
            .expect("document should parse");
        run_full(&mut shipped);
        assert!(
            !shipped.document_diagnostics(shipped_document).is_empty(),
            "shipped `nchar` returns integer, so the character annotation must be a type error"
        );
        remove_workspace_path(&shipped_workspace);

        let override_workspace = unique_temp_workspace_path();
        fs::create_dir_all(override_workspace.join("R")).expect("package root");
        fs::create_dir_all(override_workspace.join("stubs")).expect("stubs dir");
        fs::write(
            override_workspace.join("stubs/overrides.Rtypes"),
            "nchar : fn(x: character) -> character\n",
        )
        .expect("override stub should be written");
        let mut overridden =
            Analysis::new(override_workspace.clone(), LintConfig::default(), typing);
        let override_document = overridden
            .add_document_from_source(override_workspace.join(document_relative), source)
            .expect("document should parse");
        run_full(&mut overridden);
        assert!(
            overridden
                .document_diagnostics(override_document)
                .is_empty(),
            "project stub redeclaring `nchar` to return character must win, so the annotation holds"
        );
        remove_workspace_path(&override_workspace);
    }

    #[test]
    fn delete_document_if_loaded_removes_missing_package_file() {
        let workspace_path = unique_temp_workspace_path();
        let package_root = workspace_path.join("R");
        let document_path = package_root.join("main.R");
        fs::create_dir_all(&package_root).expect("workspace package root should be created");
        fs::write(&document_path, "value <- 1L\n").expect("document should be written");

        let mut analysis = Analysis::new(
            workspace_path.clone(),
            LintConfig::default(),
            CheckConfig::default(),
        );
        analysis
            .add_document_from_source(document_path.clone(), "value <- 1L\n")
            .expect("document should parse");

        fs::remove_file(&document_path).expect("document should be removed");
        analysis
            .delete_document(&document_path)
            .expect("delete should succeed");
        assert!(analysis.document(&document_path).is_none());

        remove_workspace_path(&workspace_path);
    }

    #[test]
    fn hover_reports_lowering_and_local_naming_for_symbol_use() {
        let path = PathBuf::from("/workspace/R/main.R");
        let source = "value <- function(parameter) parameter";
        let hover_character = source
            .rfind("parameter")
            .expect("hover target should exist");
        let mut analysis = Analysis::new(
            PathBuf::from("/workspace"),
            LintConfig::default(),
            CheckConfig {
                unused: false,
                typing: true,
                strict: false,
            },
        );
        analysis
            .add_document_from_source(path.clone(), source)
            .expect("document should parse");

        let hover = crate::ide::hover(
            &mut analysis,
            &path,
            TextPosition {
                line_index: 0,
                character_index: hover_character,
            },
        )
        .expect("hover target should exist");

        assert!(
            hover
                .contents
                .iter()
                .any(|block| block.contains("Local variable, defined at `R/main.R:1:19`")),
            "{:?}",
            hover.contents
        );
        assert!(hover.debug.iter().any(
            |section| section.title == "Lowering" && section.body.contains("Symbol(parameter)")
        ));
    }

    #[test]
    fn hover_reports_package_resolution_for_cross_file_symbol_use() {
        let definition_path = PathBuf::from("/workspace/R/a.R");
        let use_path = PathBuf::from("/workspace/R/b.R");
        let source = "result <- value";
        let hover_character = source.rfind("value").expect("hover target should exist");
        let mut analysis = Analysis::new(
            PathBuf::from("/workspace"),
            LintConfig::default(),
            CheckConfig::default(),
        );
        analysis
            .add_document_from_source(definition_path, "value <- 1L")
            .expect("definition document should parse");
        analysis
            .add_document_from_source(use_path.clone(), source)
            .expect("use document should parse");

        let hover = crate::ide::hover(
            &mut analysis,
            &use_path,
            TextPosition {
                line_index: 0,
                character_index: hover_character,
            },
        )
        .expect("hover target should exist");

        assert!(
            hover
                .contents
                .iter()
                .any(|block| block.contains("Package global, defined at `R/a.R:1:1`")),
            "{:?}",
            hover.contents
        );
        assert!(hover.debug.iter().any(|section| {
            section.title == "Naming"
                && section
                    .body
                    .contains("package resolution: binding `value` at R/a.R:1:1")
        }));
    }

    fn unique_temp_workspace_path() -> PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);

        let unique_suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "typing-analysis-test-{}-{}",
            unique_suffix,
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn remove_workspace_path(workspace_path: &Path) {
        if let Err(error) = fs::remove_dir_all(workspace_path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            panic!("failed to remove test workspace: {error}");
        }
    }
}
