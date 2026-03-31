use {
    crate::{
        Interner,
        diagnostic::{CheckResult, Diagnostic, DocumentDiagnostics, Severity},
        document::Document,
        hir::{Module, ModuleId},
        lower::{LoweringContext, lower_with_diagnostics},
        naming::{NamingResult, resolve_package},
        package::Package,
        package_hir::remap_package_modules,
        typecheck::inference_state_with_builtins,
    },
    std::{
        collections::{BTreeSet, HashMap},
        path::{Path, PathBuf},
    },
};

#[derive(Debug, Default)]
pub struct AnalysisState {
    lowering_context: LoweringContext,
    lowered_documents: HashMap<PathBuf, LoweredDocument>,
    module_ids_by_path: HashMap<PathBuf, ModuleId>,
    next_module_id: u32,
}

impl AnalysisState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn interner(&self) -> &Interner {
        self.lowering_context.interner()
    }

    pub fn interner_mut(&mut self) -> &mut Interner {
        self.lowering_context.interner_mut()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageLoweringResult {
    pub modules: HashMap<PathBuf, Module>,
    pub module_ids: HashMap<PathBuf, ModuleId>,
    pub diagnostics: Vec<DocumentDiagnostics>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageLoweringAndNamingResult {
    pub modules: HashMap<PathBuf, Module>,
    pub module_ids: HashMap<PathBuf, ModuleId>,
    pub naming: NamingResult,
    pub diagnostics: Vec<DocumentDiagnostics>,
}

pub fn check(package: &Package, analysis_state: &mut AnalysisState) -> CheckResult {
    let lowering_result = run_lowering(package, None, analysis_state);
    let package_result = run_naming(&lowering_result, analysis_state);
    run_typecheck(package, &package_result, analysis_state)
}

pub fn run_lowering(
    package: &Package,
    changed_document_paths: Option<&[PathBuf]>,
    analysis_state: &mut AnalysisState,
) -> PackageLoweringResult {
    refresh_lowered_documents(package, changed_document_paths, analysis_state);

    let collected_documents = collect_lowered_documents(analysis_state);
    PackageLoweringResult {
        modules: collected_documents.modules,
        module_ids: collected_documents.module_ids,
        diagnostics: collected_documents.diagnostics,
    }
}

pub fn run_naming(
    lowering_result: &PackageLoweringResult,
    analysis_state: &AnalysisState,
) -> PackageLoweringAndNamingResult {
    if !lowering_result.diagnostics.is_empty() {
        return PackageLoweringAndNamingResult {
            modules: lowering_result.modules.clone(),
            module_ids: lowering_result.module_ids.clone(),
            naming: NamingResult::default(),
            diagnostics: lowering_result.diagnostics.clone(),
        };
    }

    let naming = resolve_package(
        &sorted_lowered_modules(&lowering_result.modules, &lowering_result.module_ids),
        analysis_state.interner(),
    );
    let mut diagnostics = lowering_result.diagnostics.clone();
    diagnostics.extend(naming.diagnostics.clone());

    PackageLoweringAndNamingResult {
        modules: lowering_result.modules.clone(),
        module_ids: lowering_result.module_ids.clone(),
        naming,
        diagnostics,
    }
}

pub fn run_lowering_and_naming(
    package: &Package,
    changed_document_paths: Option<&[PathBuf]>,
    analysis_state: &mut AnalysisState,
) -> PackageLoweringAndNamingResult {
    let lowering_result = run_lowering(package, changed_document_paths, analysis_state);
    run_naming(&lowering_result, analysis_state)
}

pub fn run_typecheck(
    package: &Package,
    package_result: &PackageLoweringAndNamingResult,
    analysis_state: &mut AnalysisState,
) -> CheckResult {
    let naming_diagnostics = flatten_document_diagnostics(&package_result.diagnostics);
    if has_blocking_diagnostics(&naming_diagnostics) {
        return CheckResult {
            diagnostics: naming_diagnostics,
        };
    }

    let remapped_modules = remap_package_modules(&package_result.modules);
    let merged_module = Module::new(
        remapped_modules.arena,
        remapped_modules.definitions,
        remapped_modules.expressions,
    );
    let mut inference_state = inference_state_with_builtins(&mut analysis_state.lowering_context);

    let mut diagnostics = Vec::new();
    if let Err(error) = inference_state.infer_module(&merged_module) {
        diagnostics.push(Diagnostic::from_inference_error(
            &error,
            package.fallback_range(),
            analysis_state.lowering_context.interner(),
        ));
    }

    if diagnostics.is_empty() {
        diagnostics = naming_diagnostics;
    }

    CheckResult { diagnostics }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LoweredDocument {
    module_id: ModuleId,
    module: Module,
    diagnostics: Vec<Diagnostic>,
}

struct CollectedLoweredDocuments {
    modules: HashMap<PathBuf, Module>,
    module_ids: HashMap<PathBuf, ModuleId>,
    diagnostics: Vec<DocumentDiagnostics>,
}

fn refresh_lowered_documents(
    package: &Package,
    changed_document_paths: Option<&[PathBuf]>,
    analysis_state: &mut AnalysisState,
) {
    let package_documents = package.ordered_documents_and_scripts();
    let package_paths = package_documents
        .iter()
        .map(|(path, _)| path.clone())
        .collect::<BTreeSet<_>>();

    analysis_state
        .lowered_documents
        .retain(|path, _| package_paths.contains(path));
    analysis_state
        .module_ids_by_path
        .retain(|path, _| package_paths.contains(path));

    for (path, document) in package_documents {
        if should_refresh_document(
            &path,
            changed_document_paths,
            &analysis_state.lowered_documents,
        ) {
            let module_id = module_id_for_path(&path, analysis_state);
            analysis_state.lowered_documents.insert(
                path.clone(),
                lower_document(module_id, document, &mut analysis_state.lowering_context),
            );
        }
    }
}

fn module_id_for_path(path: &Path, analysis_state: &mut AnalysisState) -> ModuleId {
    if let Some(module_id) = analysis_state.module_ids_by_path.get(path) {
        return *module_id;
    }

    let module_id = ModuleId(analysis_state.next_module_id);
    analysis_state.next_module_id += 1;
    analysis_state
        .module_ids_by_path
        .insert(path.to_path_buf(), module_id);
    module_id
}

fn should_refresh_document(
    path: &Path,
    changed_document_paths: Option<&[PathBuf]>,
    lowered_documents: &HashMap<PathBuf, LoweredDocument>,
) -> bool {
    match changed_document_paths {
        None => true,
        Some(changed_document_paths)
            if changed_document_paths
                .iter()
                .any(|changed_document_path| changed_document_path.as_path() == path) =>
        {
            true
        }
        Some(_) => !lowered_documents.contains_key(path),
    }
}

fn lower_document(
    module_id: ModuleId,
    document: &Document,
    lowering_context: &mut LoweringContext,
) -> LoweredDocument {
    let lowering_result = lower_with_diagnostics(document, lowering_context);
    LoweredDocument {
        module_id,
        module: lowering_result.module,
        diagnostics: lowering_result.diagnostics,
    }
}

fn collect_lowered_documents(analysis_state: &AnalysisState) -> CollectedLoweredDocuments {
    let mut diagnostics = Vec::new();
    let mut modules = HashMap::new();
    let mut module_ids = HashMap::new();

    for (path, lowered_document) in &analysis_state.lowered_documents {
        modules.insert(path.clone(), lowered_document.module.clone());
        module_ids.insert(path.clone(), lowered_document.module_id);
        if !lowered_document.diagnostics.is_empty() {
            diagnostics.push((path.clone(), lowered_document.diagnostics.clone()));
        }
    }

    CollectedLoweredDocuments {
        modules,
        module_ids,
        diagnostics,
    }
}

fn sorted_lowered_modules(
    modules: &HashMap<PathBuf, Module>,
    module_ids: &HashMap<PathBuf, ModuleId>,
) -> Vec<(ModuleId, PathBuf, Module)> {
    let mut sorted_modules = modules
        .iter()
        .filter_map(|(path, module)| {
            module_ids
                .get(path)
                .map(|module_id| (*module_id, path.clone(), module.clone()))
        })
        .collect::<Vec<_>>();
    sorted_modules.sort_by(|(_, left_path, _), (_, right_path, _)| left_path.cmp(right_path));
    sorted_modules
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
