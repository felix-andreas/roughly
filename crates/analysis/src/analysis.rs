use {
    crate::{
        Interner,
        diagnostic::{Diagnostic, Diagnostics, Severity},
        document::{Document, DocumentChange, DocumentId},
        hir::{DefinitionId, DefinitionItem, ExpressionId, ExpressionKind, HirArena, Module},
        lint::{self as lint_phase, NameStyle},
        lower::lower_with_shared_interner,
        naming::{NamesGlobal, NamesLocal, rebuild_package_naming, resolve_document_locally},
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
            let changed = self.replace_document(document_id, document);
            if is_package_path {
                self.non_package_documents.remove(&document_id);
            } else {
                self.non_package_documents.insert(document_id);
            }
            if changed {
                self.bump_document_version(document_id);
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
        self.read_document_from_disk(&path)
            .map(|document| self.add_document(path, document))
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
        self.bump_document_version(document_id);
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
            self.document_path_order_key(*left_document_id)
                .cmp(&self.document_path_order_key(*right_document_id))
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
        self.extend_retained_lint_diagnostics(document_id, &mut diagnostics);
        self.extend_lowering_diagnostics(document_id, &mut diagnostics);
        self.extend_naming_diagnostics(document_id, &mut diagnostics);
        if self.check_config.typing {
            self.extend_typecheck_diagnostics(document_id, &mut diagnostics);
        }
        diagnostics
    }
}

pub fn check(analysis_state: &mut Analysis) -> Diagnostics {
    let document_ids = analysis_state.package_document_ids();

    resolve_package(analysis_state);
    let naming_diagnostics = analysis_state.collect_semantic_diagnostics(&document_ids);
    if has_blocking_diagnostics(&naming_diagnostics) {
        return naming_diagnostics;
    }

    if !analysis_state.check_config.typing {
        return naming_diagnostics;
    }

    let typecheck_diagnostics = typecheck(analysis_state);
    if typecheck_diagnostics.is_empty() {
        return naming_diagnostics;
    }

    typecheck_diagnostics
}

pub fn run_fast(analysis_state: &mut Analysis) {
    lint(analysis_state);
    lower(analysis_state);
}

pub fn run_full(analysis_state: &mut Analysis) {
    lint(analysis_state);
    check(analysis_state);
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

pub fn resolve_document(analysis_state: &mut Analysis) {
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
        let local_naming = resolve_document_locally(*document_id, module);
        analysis_state.document_naming_outputs.insert(
            *document_id,
            DocumentOutput {
                version: document_version,
                output: local_naming,
                diagnostics: Vec::new(),
            },
        );
    }
}

pub fn resolve_package(analysis_state: &mut Analysis) {
    resolve_document(analysis_state);

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

pub fn typecheck(analysis_state: &mut Analysis) -> Diagnostics {
    resolve_package(analysis_state);

    if analysis_state
        .typecheck_output
        .as_ref()
        .is_some_and(|output| output.version == analysis_state.package_version)
    {
        return analysis_state
            .collect_typecheck_diagnostics(&analysis_state.package_document_ids());
    }

    let naming_diagnostics =
        analysis_state.collect_semantic_diagnostics(&analysis_state.package_document_ids());
    if has_blocking_diagnostics(&naming_diagnostics) {
        analysis_state.typecheck_output = Some(PackageOutput {
            version: analysis_state.package_version,
            output: (),
            diagnostics: HashMap::new(),
        });
        return Vec::new();
    }

    let document_ids = analysis_state.package_document_ids();
    let modules = document_ids
        .iter()
        .filter_map(|document_id| analysis_state.module(*document_id))
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

    let typecheck_diagnostics = document_ids
        .first()
        .copied()
        .map(|document_id| HashMap::from([(document_id, diagnostics.clone())]))
        .unwrap_or_default();
    analysis_state.typecheck_output = Some(PackageOutput {
        version: analysis_state.package_version,
        output: (),
        diagnostics: typecheck_diagnostics,
    });

    diagnostics
}

impl Analysis {
    fn is_package_path(&self, path: &Path) -> bool {
        path.starts_with(self.base_path.join("R"))
    }

    fn document_path_order_key(&self, document_id: DocumentId) -> String {
        self.path_for_document_id(document_id)
            .map(|path| self.path_order_key(path))
            .unwrap_or_default()
    }

    fn path_order_key(&self, path: &Path) -> String {
        path.strip_prefix(&self.base_path)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    fn read_document_from_disk(&mut self, path: &Path) -> Result<Document, AnalysisError> {
        let file = File::open(path)
            .map_err(|error| AnalysisError::DocumentRead(path.to_path_buf(), error.to_string()))?;
        let rope = Rope::from_reader(BufReader::new(file))
            .map_err(|error| AnalysisError::DocumentRead(path.to_path_buf(), error.to_string()))?;
        let tree = tree::parse_rope(&mut self.parser, &rope, None)
            .ok_or_else(|| AnalysisError::ParseFailed(path.to_path_buf()))?;
        Ok(Document::new(rope, tree))
    }

    fn all_document_ids(&self) -> Vec<DocumentId> {
        let mut document_ids = self.documents.keys().copied().collect::<Vec<_>>();
        document_ids.sort_by_key(|document_id| document_id.0);
        document_ids
    }

    fn document_version(&self, document_id: DocumentId) -> Option<Version> {
        self.document_versions.get(&document_id).copied()
    }

    fn collect_semantic_diagnostics(&self, document_ids: &[DocumentId]) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();

        for document_id in document_ids {
            self.extend_lowering_diagnostics(*document_id, &mut diagnostics);
            self.extend_naming_diagnostics(*document_id, &mut diagnostics);
        }

        diagnostics
    }

    fn collect_typecheck_diagnostics(&self, document_ids: &[DocumentId]) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();

        for document_id in document_ids {
            self.extend_typecheck_diagnostics(*document_id, &mut diagnostics);
        }

        diagnostics
    }

    fn extend_retained_lint_diagnostics(
        &self,
        document_id: DocumentId,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        if let Some(output) = self.lint_outputs.get(&document_id) {
            diagnostics.extend(output.diagnostics.iter().cloned());
        }
    }

    fn extend_lowering_diagnostics(
        &self,
        document_id: DocumentId,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        if let Some(output) = self.lowering_outputs.get(&document_id) {
            diagnostics.extend(output.diagnostics.iter().cloned());
        }
    }

    fn extend_naming_diagnostics(
        &self,
        document_id: DocumentId,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        if let Some(output) = self.document_naming_outputs.get(&document_id) {
            diagnostics.extend(output.diagnostics.iter().cloned());
        }
        if let Some(output) = &self.package_naming_output
            && let Some(package_diagnostics) = output.diagnostics.get(&document_id)
        {
            diagnostics.extend(package_diagnostics.iter().cloned());
        }
    }

    fn extend_typecheck_diagnostics(
        &self,
        document_id: DocumentId,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        if let Some(output) = &self.typecheck_output
            && let Some(typecheck_diagnostics) = output.diagnostics.get(&document_id)
        {
            diagnostics.extend(typecheck_diagnostics.iter().cloned());
        }
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

    fn bump_document_version(&mut self, document_id: DocumentId) {
        let version = self.bump_version();
        self.document_versions.insert(document_id, version);
    }

    fn bump_package_version(&mut self) {
        self.package_version = self.bump_version();
    }

    fn replace_document(&mut self, document_id: DocumentId, document: Document) -> bool {
        let current_text = self
            .documents
            .get(&document_id)
            .map(|current_document| current_document.rope().to_string());
        let next_text = document.rope().to_string();
        if current_text.as_deref() == Some(next_text.as_str()) {
            return false;
        }
        self.documents.insert(document_id, document);
        true
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
        super::{
            Analysis, CheckConfig, DocumentChange, LintConfig, check, lint, lower, resolve_package,
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
    fn check_runs_naming_even_when_lowering_reports_errors() {
        let path = PathBuf::from("/workspace/R/main.R");
        let mut analysis = Analysis::new(
            PathBuf::from("/workspace"),
            LintConfig::default(),
            CheckConfig::default(),
        );
        let document_id = analysis
            .add_document_from_source(path, "value <-")
            .expect("document should parse");

        let result = check(&mut analysis);

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

        let initial_diagnostics = super::typecheck(&mut analysis);
        assert!(!initial_diagnostics.is_empty());

        let sentinel =
            Diagnostic::type_error(initial_diagnostics[0].range, "cached typecheck sentinel");
        analysis
            .typecheck_output
            .as_mut()
            .expect("typecheck output should exist")
            .diagnostics = HashMap::from([(document_id, vec![sentinel.clone()])]);

        let cached_diagnostics = super::typecheck(&mut analysis);

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
