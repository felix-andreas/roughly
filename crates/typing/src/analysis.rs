use {
    crate::{
        Interner,
        diagnostic::{CheckResult, Diagnostic, DocumentDiagnostics, Severity},
        document::{Document, DocumentEditError, DocumentId},
        hir::{DefinitionId, DefinitionItem, ExpressionId, ExpressionKind, HirArena, Module},
        lower::lower_with_shared_interner,
        naming::{LocalNamingResult, NamingResult, resolve_package},
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
            .map_err(|_| AnalysisError::ParseFailed(path.clone()))?;
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

    pub fn document_diagnostics(
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

    pub fn document_phase_diagnostics<'a>(
        &'a self,
        document_id: DocumentId,
        phases: &'a [AnalysisPhase],
    ) -> impl Iterator<Item = &'a Diagnostic> + 'a {
        self.diagnostics
            .get(&document_id)
            .into_iter()
            .flat_map(move |phase_diagnostics| {
                phase_diagnostics
                    .iter()
                    .filter(move |phase_diagnostic| phases.contains(&phase_diagnostic.phase))
                    .map(|phase_diagnostic| &phase_diagnostic.diagnostic)
            })
    }
}

pub fn check(analysis_state: &mut Analysis) -> CheckResult {
    let document_ids = analysis_state.package_document_ids();

    run_lowering(None, analysis_state);
    run_naming(None, analysis_state);
    let naming_diagnostics = analysis_state.collect_phase_diagnostics(
        &document_ids,
        &[AnalysisPhase::Lowering, AnalysisPhase::Naming],
    );
    if has_blocking_diagnostics(&naming_diagnostics) {
        return CheckResult {
            diagnostics: naming_diagnostics,
        };
    }

    let typecheck_result = run_typecheck(None, analysis_state);
    if typecheck_result.diagnostics.is_empty() {
        return CheckResult {
            diagnostics: naming_diagnostics,
        };
    }

    typecheck_result
}

pub fn run_lowering(changed_documents: Option<&[DocumentId]>, analysis_state: &mut Analysis) {
    let document_ids = match changed_documents {
        None => analysis_state.all_document_ids(),
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
}

pub fn run_naming(_changed_documents: Option<&[DocumentId]>, analysis_state: &mut Analysis) {
    let package_document_ids = analysis_state.package_document_ids();
    let all_document_ids = analysis_state.all_document_ids();
    analysis_state.naming = NamingStore::default();
    analysis_state.clear_phase_diagnostics(AnalysisPhase::Naming);

    let package_modules = package_document_ids
        .iter()
        .filter_map(|document_id| {
            analysis_state
                .lowering
                .modules
                .get(document_id)
                .map(|module| (*document_id, module))
        })
        .collect::<Vec<_>>();
    let non_package_modules = all_document_ids
        .iter()
        .filter(|document_id| !package_document_ids.contains(document_id))
        .filter_map(|document_id| {
            analysis_state
                .lowering
                .modules
                .get(document_id)
                .map(|module| (*document_id, module))
        })
        .collect::<Vec<_>>();
    let naming_computation = resolve_package(
        &package_modules,
        &non_package_modules,
        analysis_state.interner(),
    );
    analysis_state.naming.locals = naming_computation.locals;
    analysis_state.naming.package = naming_computation.naming;
    for (document_id, diagnostics) in naming_computation.diagnostics {
        analysis_state.push_phase_diagnostics(document_id, AnalysisPhase::Naming, diagnostics);
    }
}

pub fn run_typecheck(
    _changed_documents: Option<&[DocumentId]>,
    analysis_state: &mut Analysis,
) -> CheckResult {
    let document_ids = analysis_state.package_document_ids();
    analysis_state.clear_phase_diagnostics(AnalysisPhase::Typecheck);

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

    CheckResult { diagnostics }
}

impl Analysis {
    fn all_document_ids(&self) -> Vec<DocumentId> {
        let mut document_ids = self.documents.keys().copied().collect::<Vec<_>>();
        document_ids.sort_by_key(|document_id| document_id.0);
        document_ids
    }

    fn collect_phase_diagnostics(
        &self,
        document_ids: &[DocumentId],
        phases: &[AnalysisPhase],
    ) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();

        for document_id in document_ids {
            diagnostics.extend(
                self.document_phase_diagnostics(*document_id, phases)
                    .cloned(),
            );
        }

        diagnostics
    }

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

struct RemappedModules {
    arena: HirArena,
    definitions: Vec<DefinitionItem>,
    expressions: Vec<ExpressionId>,
}

fn remap_package_modules(modules: &[&Module]) -> RemappedModules {
    let mut arena = HirArena::new();
    let mut definitions = Vec::new();
    let mut expressions = Vec::new();
    let mut next_expression_id = 0u32;
    let mut next_definition_id = 0u32;

    for module in modules {
        let expression_offset = next_expression_id;
        let definition_offset = next_definition_id;

        let remapped_expressions = module
            .arena
            .expressions()
            .iter()
            .cloned()
            .map(|mut expression| {
                expression.id = ExpressionId(expression.id.0 + expression_offset);
                remap_expression_kind(&mut expression.kind, expression_offset);
                expression
            })
            .collect::<Vec<_>>();
        next_expression_id +=
            u32::try_from(remapped_expressions.len()).expect("expression count exceeded u32");
        arena.expressions.extend(remapped_expressions);

        let remapped_definitions = module
            .definitions
            .iter()
            .cloned()
            .map(|definition| {
                DefinitionItem::new(
                    DefinitionId(definition.id.0 + definition_offset),
                    definition.range,
                    definition.definition,
                )
            })
            .collect::<Vec<_>>();
        next_definition_id +=
            u32::try_from(remapped_definitions.len()).expect("definition count exceeded u32");
        definitions.extend(remapped_definitions);

        expressions.extend(
            module
                .expressions
                .iter()
                .map(|expression_id| ExpressionId(expression_id.0 + expression_offset)),
        );
    }

    RemappedModules {
        arena,
        definitions,
        expressions,
    }
}

fn remap_expression_kind(expression_kind: &mut ExpressionKind, expression_offset: u32) {
    match expression_kind {
        ExpressionKind::Block { expressions, .. } => {
            for expression_id in expressions {
                *expression_id = ExpressionId(expression_id.0 + expression_offset);
            }
        }
        ExpressionKind::Assign { value, .. }
        | ExpressionKind::UnaryMinus { value }
        | ExpressionKind::Dollar { value, .. } => {
            *value = ExpressionId(value.0 + expression_offset);
        }
        ExpressionKind::Function { body, .. } | ExpressionKind::Repeat { body } => {
            *body = ExpressionId(body.0 + expression_offset);
        }
        ExpressionKind::While { condition, body } => {
            *condition = ExpressionId(condition.0 + expression_offset);
            *body = ExpressionId(body.0 + expression_offset);
        }
        ExpressionKind::For { sequence, body, .. } => {
            *sequence = ExpressionId(sequence.0 + expression_offset);
            *body = ExpressionId(body.0 + expression_offset);
        }
        ExpressionKind::If {
            condition,
            consequence,
            alternative,
        } => {
            *condition = ExpressionId(condition.0 + expression_offset);
            *consequence = ExpressionId(consequence.0 + expression_offset);
            if let Some(alternative) = alternative {
                *alternative = ExpressionId(alternative.0 + expression_offset);
            }
        }
        ExpressionKind::Call { callee, arguments }
        | ExpressionKind::Subset {
            value: callee,
            arguments,
        }
        | ExpressionKind::Subset2 {
            value: callee,
            arguments,
        } => {
            *callee = ExpressionId(callee.0 + expression_offset);
            for argument in arguments {
                argument.expression = ExpressionId(argument.expression.0 + expression_offset);
            }
        }
        ExpressionKind::Null
        | ExpressionKind::Logical(_)
        | ExpressionKind::Integer(_)
        | ExpressionKind::Double(_)
        | ExpressionKind::Character(_)
        | ExpressionKind::StringLiteralName(_)
        | ExpressionKind::Symbol(_)
        | ExpressionKind::Unsupported => {}
    }
}

#[cfg(test)]
mod tests {
    use {
        super::{Analysis, AnalysisPhase, check, run_lowering, run_naming},
        crate::{
            Severity,
            text::{TextPosition, TextRange},
        },
        std::path::PathBuf,
    };

    #[test]
    fn package_document_ids_exclude_non_package_paths() {
        let mut analysis = Analysis::new(PathBuf::from("/workspace"));
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
    fn edit_document_invalidates_lowering_and_naming_state() {
        let path = PathBuf::from("/workspace/R/main.R");
        let mut analysis = Analysis::new(PathBuf::from("/workspace"));
        let document_id = analysis
            .add_document_from_source(path.clone(), "value <- 1L")
            .expect("document should parse");

        run_lowering(None, &mut analysis);
        run_naming(None, &mut analysis);

        assert!(analysis.lowering.modules.contains_key(&document_id));
        assert!(analysis.naming.locals.contains_key(&document_id));

        analysis
            .edit_document(&path, |document, parser| {
                document.edit_range(
                    parser,
                    TextRange {
                        start: TextPosition {
                            line_index: 0,
                            character_index: 0,
                        },
                        end: TextPosition {
                            line_index: 0,
                            character_index: "value <- 1L".len(),
                        },
                    },
                    "value <-",
                )
            })
            .expect("edit should succeed");

        assert!(!analysis.lowering.modules.contains_key(&document_id));
        assert!(analysis.naming.locals.is_empty());
        assert!(analysis.naming.package.bindings.is_empty());

        run_lowering(None, &mut analysis);
        let diagnostics = analysis
            .document_diagnostics(&[document_id], &[AnalysisPhase::Lowering])
            .into_iter()
            .flat_map(|(_, diagnostics)| diagnostics)
            .collect::<Vec<_>>();

        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error)
        );
    }

    #[test]
    fn delete_document_removes_cached_state() {
        let path = PathBuf::from("/workspace/R/main.R");
        let mut analysis = Analysis::new(PathBuf::from("/workspace"));
        let document_id = analysis
            .add_document_from_source(path.clone(), "value <- 1L")
            .expect("document should parse");

        run_lowering(None, &mut analysis);

        assert!(analysis.document(&path).is_some());
        assert!(analysis.lowering.modules.contains_key(&document_id));

        analysis
            .delete_document(&path)
            .expect("delete should succeed");

        assert!(analysis.document(&path).is_none());
        assert!(analysis.document_id_for_path(&path).is_none());
        assert!(!analysis.lowering.modules.contains_key(&document_id));
        assert!(analysis.package_document_ids().is_empty());
    }

    #[test]
    fn check_runs_naming_even_when_lowering_reports_errors() {
        let path = PathBuf::from("/workspace/R/main.R");
        let mut analysis = Analysis::new(PathBuf::from("/workspace"));
        let document_id = analysis
            .add_document_from_source(path, "value <-")
            .expect("document should parse");

        let result = check(&mut analysis);

        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error)
        );
        assert!(analysis.lowering.modules.contains_key(&document_id));
        assert!(analysis.naming.locals.contains_key(&document_id));
    }
}
