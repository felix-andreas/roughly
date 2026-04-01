use {
    crate::{
        Interner,
        diagnostic::{CheckResult, Diagnostic, DocumentDiagnostics, Severity},
        document::{Document, DocumentEditError, DocumentId},
        hir::Module,
        lower::lower_with_shared_interner,
        naming::{LocalNamingResult, NamingResult, resolve_package},
        package_hir::remap_package_modules,
        tree,
        typecheck::inference_state_with_builtins_in_interner,
    },
    std::{
        collections::{HashMap, HashSet},
        path::{Path, PathBuf},
    },
    tree_sitter::Parser,
};

pub struct Analysis {
    base_path: PathBuf,
    parser: Parser,
    interner: Interner,
    next_document_id: u32,
    documents: HashMap<DocumentId, Document>,
    document_ids_by_path: HashMap<PathBuf, DocumentId>,
    document_paths: HashMap<DocumentId, PathBuf>,
    non_package_documents: HashSet<DocumentId>,
    pub lowering: LoweringStore,
    pub naming: NamingStore,
    pub typecheck: TypecheckStore,
    diagnostics: HashMap<DocumentId, Vec<PhaseDiagnostic>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalysisError {
    DocumentNotFound(PathBuf),
    ParseFailed(PathBuf),
    DocumentEdit(DocumentEditError),
}

#[derive(Debug, Default)]
pub struct LoweringStore {
    pub modules: HashMap<DocumentId, Module>,
}

#[derive(Debug, Default)]
pub struct NamingStore {
    pub locals: HashMap<DocumentId, LocalNamingResult>,
    pub package: NamingResult,
}

#[derive(Debug, Default)]
pub struct TypecheckStore {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoweringResult {
    pub document_ids: Vec<DocumentId>,
    pub diagnostics: Vec<DocumentDiagnostics>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamingRunResult {
    pub naming: NamingResult,
    pub diagnostics: Vec<DocumentDiagnostics>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseDiagnostic {
    pub phase: AnalysisPhase,
    pub diagnostic: Diagnostic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisPhase {
    Lowering,
    Naming,
    Typecheck,
}

impl Analysis {
    pub fn new(base_path: PathBuf) -> Self {
        Self {
            base_path,
            parser: tree::new_parser().expect("typing parser should initialize"),
            interner: Interner::new(),
            next_document_id: 0,
            documents: HashMap::new(),
            document_ids_by_path: HashMap::new(),
            document_paths: HashMap::new(),
            non_package_documents: HashSet::new(),
            lowering: LoweringStore::default(),
            naming: NamingStore::default(),
            typecheck: TypecheckStore::default(),
            diagnostics: HashMap::new(),
        }
    }

    pub fn interner(&self) -> &Interner {
        &self.interner
    }

    pub fn interner_mut(&mut self) -> &mut Interner {
        &mut self.interner
    }

    pub fn base_path(&self) -> &Path {
        &self.base_path
    }

    pub fn add_document(&mut self, path: PathBuf, document: Document) -> DocumentId {
        if let Some(document_id) = self.document_ids_by_path.get(&path).copied() {
            self.documents.insert(document_id, document);
            if path.starts_with(self.base_path.join("R")) {
                self.non_package_documents.remove(&document_id);
            } else {
                self.non_package_documents.insert(document_id);
            }
            self.invalidate_document(document_id);
            return document_id;
        }

        let document_id = DocumentId(self.next_document_id);
        self.next_document_id += 1;
        self.document_ids_by_path.insert(path.clone(), document_id);
        self.document_paths.insert(document_id, path);
        self.documents.insert(document_id, document);
        let path = self
            .document_paths
            .get(&document_id)
            .expect("document path should exist");
        if path.starts_with(self.base_path.join("R")) {
            self.non_package_documents.remove(&document_id);
        } else {
            self.non_package_documents.insert(document_id);
        }
        self.invalidate_package_semantics();
        document_id
    }

    pub fn add_document_from_source(
        &mut self,
        path: PathBuf,
        source: &str,
    ) -> Result<DocumentId, AnalysisError> {
        let document = Document::parse(&mut self.parser, source)
            .ok_or_else(|| AnalysisError::ParseFailed(path.clone()))?;
        Ok(self.add_document(path, document))
    }

    pub fn edit_document(
        &mut self,
        path: &Path,
        edit: impl FnOnce(&mut Document, &mut Parser) -> Result<(), DocumentEditError>,
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
        edit(document, &mut self.parser).map_err(AnalysisError::DocumentEdit)?;
        self.invalidate_document(document_id);
        Ok(())
    }

    pub fn delete_document(&mut self, path: &Path) -> Result<(), AnalysisError> {
        let document_id = self
            .document_ids_by_path
            .remove(path)
            .ok_or_else(|| AnalysisError::DocumentNotFound(path.to_path_buf()))?;
        self.documents.remove(&document_id);
        self.document_paths.remove(&document_id);
        self.non_package_documents.remove(&document_id);
        self.invalidate_document(document_id);
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
        self.lowering.modules.get(&document_id)
    }

    pub fn package_document_ids(&self) -> Vec<DocumentId> {
        let mut document_ids = self
            .documents
            .keys()
            .copied()
            .filter(|document_id| !self.non_package_documents.contains(document_id))
            .collect::<Vec<_>>();
        document_ids.sort_by_key(|document_id| document_id.0);
        document_ids
    }
}

pub fn check(analysis_state: &mut Analysis) -> CheckResult {
    run_lowering(None, analysis_state);
    run_naming(None, analysis_state);
    run_typecheck(None, analysis_state)
}

pub fn run_lowering_and_naming(
    changed_documents: Option<&[DocumentId]>,
    analysis_state: &mut Analysis,
) -> NamingRunResult {
    run_lowering(changed_documents, analysis_state);
    run_naming(None, analysis_state)
}

pub fn run_lowering(
    changed_documents: Option<&[DocumentId]>,
    analysis_state: &mut Analysis,
) -> LoweringResult {
    let document_ids = match changed_documents {
        None => analysis_state.package_document_ids(),
        Some(changed_documents) => {
            let mut document_ids = changed_documents
                .iter()
                .copied()
                .filter(|document_id| analysis_state.documents.contains_key(document_id))
                .collect::<Vec<_>>();
            document_ids.sort_by_key(|document_id| document_id.0);
            document_ids
        }
    };

    for document_id in &document_ids {
        analysis_state.clear_document_phase_diagnostics(*document_id, AnalysisPhase::Lowering);
        let Some(document) = analysis_state.documents.get(document_id).cloned() else {
            continue;
        };
        let lowering_result = lower_with_shared_interner(&document, analysis_state.interner_mut());
        analysis_state
            .lowering
            .modules
            .insert(*document_id, lowering_result.module);
        analysis_state.push_phase_diagnostics(
            *document_id,
            AnalysisPhase::Lowering,
            lowering_result.diagnostics,
        );
    }

    LoweringResult {
        document_ids: document_ids.clone(),
        diagnostics: analysis_state.document_diagnostics(&document_ids, &[AnalysisPhase::Lowering]),
    }
}

pub fn run_naming(
    _changed_documents: Option<&[DocumentId]>,
    analysis_state: &mut Analysis,
) -> NamingRunResult {
    let document_ids = analysis_state.package_document_ids();
    analysis_state.naming = NamingStore::default();
    analysis_state.clear_phase_diagnostics(AnalysisPhase::Naming);

    let lowering_diagnostics =
        analysis_state.document_diagnostics(&document_ids, &[AnalysisPhase::Lowering]);
    if has_blocking_diagnostics(&flatten_document_diagnostics(&lowering_diagnostics)) {
        return NamingRunResult {
            naming: NamingResult::default(),
            diagnostics: lowering_diagnostics,
        };
    }

    let modules = document_ids
        .iter()
        .filter_map(|document_id| {
            analysis_state
                .lowering
                .modules
                .get(document_id)
                .map(|module| (*document_id, module))
        })
        .collect::<Vec<_>>();
    let naming_computation = resolve_package(&modules, analysis_state.interner());
    analysis_state.naming.locals = naming_computation.locals;
    analysis_state.naming.package = naming_computation.naming;
    for (document_id, diagnostics) in naming_computation.diagnostics {
        analysis_state.push_phase_diagnostics(document_id, AnalysisPhase::Naming, diagnostics);
    }

    NamingRunResult {
        naming: analysis_state.naming.package.clone(),
        diagnostics: analysis_state.document_diagnostics(
            &document_ids,
            &[AnalysisPhase::Lowering, AnalysisPhase::Naming],
        ),
    }
}

pub fn run_typecheck(
    _changed_documents: Option<&[DocumentId]>,
    analysis_state: &mut Analysis,
) -> CheckResult {
    let document_ids = analysis_state.package_document_ids();
    analysis_state.clear_phase_diagnostics(AnalysisPhase::Typecheck);

    let naming_diagnostics = flatten_document_diagnostics(&analysis_state.document_diagnostics(
        &document_ids,
        &[AnalysisPhase::Lowering, AnalysisPhase::Naming],
    ));
    if has_blocking_diagnostics(&naming_diagnostics) {
        return CheckResult {
            diagnostics: naming_diagnostics,
        };
    }

    let modules = document_ids
        .iter()
        .filter_map(|document_id| analysis_state.lowering.modules.get(document_id))
        .collect::<Vec<_>>();
    let remapped_modules = remap_package_modules(&modules);
    let merged_module = Module::new(
        remapped_modules.arena,
        remapped_modules.definitions,
        remapped_modules.expressions,
    );
    let mut inference_state =
        inference_state_with_builtins_in_interner(analysis_state.interner_mut());

    let mut diagnostics = Vec::new();
    if let Err(error) = inference_state.infer_module(&merged_module) {
        diagnostics.push(Diagnostic::from_inference_error(
            &error,
            analysis_state.fallback_range(),
            analysis_state.interner(),
        ));
    }

    if let Some(first_document_id) = document_ids.first().copied() {
        analysis_state.push_phase_diagnostics(
            first_document_id,
            AnalysisPhase::Typecheck,
            diagnostics.clone(),
        );
    }

    if diagnostics.is_empty() {
        diagnostics = naming_diagnostics;
    }

    CheckResult { diagnostics }
}

impl Analysis {
    fn invalidate_document(&mut self, document_id: DocumentId) {
        self.lowering.modules.remove(&document_id);
        self.diagnostics.remove(&document_id);
        self.invalidate_package_semantics();
    }

    fn invalidate_package_semantics(&mut self) {
        self.naming = NamingStore::default();
        self.typecheck = TypecheckStore::default();
        self.clear_phase_diagnostics(AnalysisPhase::Naming);
        self.clear_phase_diagnostics(AnalysisPhase::Typecheck);
    }

    fn clear_phase_diagnostics(&mut self, phase: AnalysisPhase) {
        self.diagnostics.retain(|_, diagnostics| {
            diagnostics.retain(|phase_diagnostic| phase_diagnostic.phase != phase);
            !diagnostics.is_empty()
        });
    }

    fn clear_document_phase_diagnostics(&mut self, document_id: DocumentId, phase: AnalysisPhase) {
        let should_remove = if let Some(diagnostics) = self.diagnostics.get_mut(&document_id) {
            diagnostics.retain(|phase_diagnostic| phase_diagnostic.phase != phase);
            diagnostics.is_empty()
        } else {
            false
        };
        if should_remove {
            self.diagnostics.remove(&document_id);
        }
    }

    fn push_phase_diagnostics(
        &mut self,
        document_id: DocumentId,
        phase: AnalysisPhase,
        diagnostics: Vec<Diagnostic>,
    ) {
        if diagnostics.is_empty() {
            return;
        }
        self.diagnostics.entry(document_id).or_default().extend(
            diagnostics
                .into_iter()
                .map(|diagnostic| PhaseDiagnostic { phase, diagnostic }),
        );
    }

    fn document_diagnostics(
        &self,
        document_ids: &[DocumentId],
        phases: &[AnalysisPhase],
    ) -> Vec<DocumentDiagnostics> {
        let mut diagnostics = Vec::new();

        for document_id in document_ids {
            let Some(path) = self.path_for_document_id(*document_id) else {
                continue;
            };
            let path_diagnostics = self
                .diagnostics
                .get(document_id)
                .map(|phase_diagnostics| {
                    phase_diagnostics
                        .iter()
                        .filter(|phase_diagnostic| phases.contains(&phase_diagnostic.phase))
                        .map(|phase_diagnostic| phase_diagnostic.diagnostic.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if !path_diagnostics.is_empty() {
                diagnostics.push((path.to_path_buf(), path_diagnostics));
            }
        }

        diagnostics
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

fn has_blocking_diagnostics(diagnostics: &[Diagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
}

fn flatten_document_diagnostics(document_diagnostics: &[DocumentDiagnostics]) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    for (_, path_diagnostics) in document_diagnostics {
        diagnostics.extend(path_diagnostics.clone());
    }

    diagnostics
}
