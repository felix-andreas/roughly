use {
    crate::{
        Interner,
        diagnostic::Diagnostic,
        document::{Document, DocumentChange, DocumentId},
        hir::Module,
        lint::{self as lint_phase, NameStyle},
        lower::lower_with_shared_interner,
        naming::{
            NamesGlobal, NamesLocal, find_exported_binding, rebuild_package_naming,
            resolve_document_locally,
        },
        tree,
        typecheck::inference_state_with_builtins_in_interner,
    },
    ropey::Rope,
    std::{
        collections::{HashMap, HashSet},
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
    package_naming_output: Option<PackageOutput<NamesGlobal>>,
    typecheck_output: Option<PackageOutput<()>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalysisError {
    DocumentNotFound(PathBuf),
    ParseFailed(PathBuf),
    DocumentRead(PathBuf, String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct LintConfig {
    pub naming_style: Option<NameStyle>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct CheckConfig {
    pub unused: bool,
    pub typing: bool,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackageOutput<T> {
    version: Version,
    output: T,
    diagnostics: HashMap<DocumentId, Vec<Diagnostic>>,
}

impl Analysis {
    pub fn new(base_path: PathBuf, lint_config: LintConfig, check_config: CheckConfig) -> Self {
        Self {
            base_path,
            lint_config,
            check_config,
            parser: tree::new_parser().expect("typing parser should initialize"),
            interner: Interner::new(),
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
            package_naming_output: None,
            typecheck_output: None,
        }
    }

    pub fn set_configs(&mut self, lint_config: LintConfig, check_config: CheckConfig) {
        self.lint_config = lint_config;
        self.check_config = check_config;
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
        document.edit(&mut self.parser, changes);
        let version = self.bump_version();
        self.document_versions.insert(document_id, version);
        self.bump_package_version();
        self.invalidate_document(document_id);
        Ok(())
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
            let left_path = self
                .path_for_document_id(*left_document_id)
                .map(|path| {
                    path.strip_prefix(&self.base_path)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .replace('\\', "/")
                })
                .unwrap_or_default();
            let right_path = self
                .path_for_document_id(*right_document_id)
                .map(|path| {
                    path.strip_prefix(&self.base_path)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .replace('\\', "/")
                })
                .unwrap_or_default();
            left_path.cmp(&right_path)
        });
        document_ids
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
        }
        if let Some(output) = &self.package_naming_output
            && let Some(package_diagnostics) = output.diagnostics.get(&document_id)
        {
            diagnostics.extend(package_diagnostics.iter().cloned());
        }
        if self.check_config.typing {
            if let Some(output) = &self.typecheck_output
                && let Some(typecheck_diagnostics) = output.diagnostics.get(&document_id)
            {
                diagnostics.extend(typecheck_diagnostics.iter().cloned());
            }
        }
        diagnostics
    }
}

pub fn run_full(analysis_state: &mut Analysis) {
    lint(analysis_state);

    if analysis_state.check_config.typing {
        typecheck(analysis_state);
    } else {
        resolve_package(analysis_state);
    }
}

pub fn run_fast(analysis_state: &mut Analysis) {
    lint(analysis_state);
    lower(analysis_state);
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
        let local_naming =
            resolve_document_locally(*document_id, module, analysis_state.interner());
        analysis_state.document_naming_outputs.insert(
            *document_id,
            DocumentOutput {
                version: document_version,
                output: local_naming.naming,
                diagnostics: local_naming.diagnostics,
            },
        );
    }

    if analysis_state
        .package_naming_output
        .as_ref()
        .is_some_and(|output| output.version == analysis_state.package_version)
    {
        return;
    }

    let document_ids = analysis_state.package_document_ids();
    let all_document_ids = analysis_state.all_document_ids();
    let package_modules = document_ids
        .iter()
        .filter_map(|document_id| {
            analysis_state
                .module(*document_id)
                .map(|module| (*document_id, module))
        })
        .collect::<Vec<_>>();
    let extra_modules = all_document_ids
        .iter()
        .filter(|document_id| !document_ids.contains(document_id))
        .filter_map(|document_id| {
            analysis_state
                .module(*document_id)
                .map(|module| (*document_id, module))
        })
        .collect::<Vec<_>>();
    let naming_locals = analysis_state
        .document_naming_outputs
        .iter()
        .map(|(document_id, output)| (*document_id, output.output.clone()))
        .collect::<HashMap<_, _>>();
    let naming_computation = rebuild_package_naming(
        &package_modules,
        &extra_modules,
        &naming_locals,
        analysis_state.interner(),
    );
    analysis_state.package_naming_output = Some(PackageOutput {
        version: analysis_state.package_version,
        output: naming_computation.naming,
        diagnostics: naming_computation.diagnostics,
    });
}

pub fn typecheck(analysis_state: &mut Analysis) {
    resolve_package(analysis_state);

    if analysis_state
        .typecheck_output
        .as_ref()
        .is_some_and(|output| output.version == analysis_state.package_version)
    {
        return;
    }

    let document_ids = analysis_state.package_document_ids();
    let mut inference_state =
        inference_state_with_builtins_in_interner(analysis_state.interner_mut());
    let package_naming = analysis_state
        .package_naming_output
        .as_ref()
        .unwrap_or_else(|| panic!("missing package naming output for typecheck"));
    for (symbol, document_id) in &package_naming.output.global_bindings {
        let local_naming = analysis_state
            .document_naming_outputs
            .get(document_id)
            .unwrap_or_else(|| {
                panic!("missing local naming output for package winner {document_id:?}")
            });
        let module = analysis_state
            .module(*document_id)
            .unwrap_or_else(|| panic!("missing lowered module for package winner {document_id:?}"));
        let binding_id = find_exported_binding(module, &local_naming.output, *symbol)
            .unwrap_or_else(|| {
                panic!("missing exported binding for package winner {document_id:?}:{symbol:?}")
            });
        let binding = local_naming
            .output
            .bindings
            .get(&binding_id)
            .unwrap_or_else(|| {
                panic!("missing binding info for package winner {document_id:?}:{binding_id:?}")
            });
        let variable = inference_state.fresh_variable();
        inference_state.bind_global_name(
            *symbol,
            crate::types::CoreType::Variable(variable),
            binding.range,
        );
    }

    let mut diagnostics = HashMap::new();
    for document_id in document_ids {
        let module = analysis_state
            .module(document_id)
            .unwrap_or_else(|| panic!("missing lowered module for typecheck {document_id:?}"));
        let local_naming = analysis_state
            .document_naming_outputs
            .get(&document_id)
            .unwrap_or_else(|| panic!("missing local naming output for typecheck {document_id:?}"));
        if let Err(error) = inference_state.infer_module_with_naming(
            document_id,
            module,
            &local_naming.output,
            &package_naming.output,
        ) {
            diagnostics.insert(
                document_id,
                vec![Diagnostic::from_inference_error(
                    &error,
                    analysis_state.fallback_range(),
                    analysis_state.interner(),
                )],
            );
            break;
        }
    }
    analysis_state.typecheck_output = Some(PackageOutput {
        version: analysis_state.package_version,
        output: (),
        diagnostics,
    });
}

impl Analysis {
    fn is_package_path(&self, path: &Path) -> bool {
        path.starts_with(self.base_path.join("R"))
    }

    fn all_document_ids(&self) -> Vec<DocumentId> {
        let mut document_ids = self.documents.keys().copied().collect::<Vec<_>>();
        document_ids.sort_by_key(|document_id| document_id.0);
        document_ids
    }

    fn document_version(&self, document_id: DocumentId) -> Option<Version> {
        self.document_versions.get(&document_id).copied()
    }

    fn invalidate_document(&mut self, document_id: DocumentId) {
        self.lint_outputs.remove(&document_id);
        self.lowering_outputs.remove(&document_id);
        self.document_naming_outputs.remove(&document_id);
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
            Diagnostic, Severity,
            ide::HoverPhase,
            lint::NameStyle,
            text::{TextPosition, TextRange},
        },
        std::{
            collections::HashMap,
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
            .add_document_from_source(path.clone(), "missing")
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
                            character_index: "missing".len(),
                        },
                    },
                    text: "missing(".to_owned(),
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
    fn typecheck_uses_cached_output_when_package_version_is_unchanged() {
        let mut analysis = Analysis::new(
            PathBuf::from("/workspace"),
            LintConfig::default(),
            CheckConfig::default(),
        );
        let document_id = analysis
            .add_document_from_source(PathBuf::from("/workspace/R/main.R"), "1L + \"text\"")
            .expect("document should parse");

        super::typecheck(&mut analysis);
        let initial_diagnostics = analysis
            .typecheck_output
            .as_ref()
            .and_then(|output| output.diagnostics.get(&document_id))
            .cloned()
            .unwrap_or_default();
        assert!(!initial_diagnostics.is_empty());

        let sentinel =
            Diagnostic::type_error(initial_diagnostics[0].range, "cached typecheck sentinel");
        analysis
            .typecheck_output
            .as_mut()
            .expect("typecheck output should exist")
            .diagnostics = HashMap::from([(document_id, vec![sentinel.clone()])]);

        super::typecheck(&mut analysis);
        let cached_diagnostics = analysis
            .typecheck_output
            .as_ref()
            .and_then(|output| output.diagnostics.get(&document_id))
            .cloned()
            .unwrap_or_default();

        assert_eq!(cached_diagnostics, vec![sentinel]);
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

        assert_eq!(hover.sections[0].phase, HoverPhase::Lowering);
        assert_eq!(hover.sections[0].value, "Symbol(parameter)");
        assert_eq!(hover.sections[1].phase, HoverPhase::Naming);
        assert!(
            hover.sections[1]
                .value
                .contains("local resolution: binding `parameter` at R/main.R:1:19")
        );
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

        assert_eq!(hover.sections[0].value, "Symbol(value)");
        assert!(
            hover.sections[1]
                .value
                .contains("local resolution: unresolved `value`")
        );
        assert!(
            hover.sections[1]
                .value
                .contains("package resolution: binding `value` at R/a.R:1:1")
        );
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
        if let Err(error) = fs::remove_dir_all(workspace_path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                panic!("failed to remove test workspace: {error}");
            }
        }
    }
}
