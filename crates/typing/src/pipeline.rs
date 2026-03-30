use {
    crate::{
        Interner,
        diagnostic::{CheckResult, Diagnostic, DocumentDiagnostics, Severity},
        document::Document,
        hir::Module,
        lower::{LoweringContext, lower_with_diagnostics},
        naming::{NamingResult, resolve_package},
        package::Package,
        package_hir::{remap_package_modules, sorted_modules},
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
pub struct PackageLoweringAndNamingResult {
    pub modules: HashMap<PathBuf, Module>,
    pub naming: NamingResult,
    pub diagnostics: Vec<DocumentDiagnostics>,
}

pub fn check(package: &Package, analysis_state: &mut AnalysisState) -> CheckResult {
    let package_result = run_lowering_and_naming(package, None, analysis_state);
    run_typecheck(package, &package_result, analysis_state)
}

pub fn run_lowering_and_naming(
    package: &Package,
    changed_document_paths: Option<&[PathBuf]>,
    analysis_state: &mut AnalysisState,
) -> PackageLoweringAndNamingResult {
    refresh_lowered_documents(package, changed_document_paths, analysis_state);

    let (modules, mut diagnostics) = collect_lowered_documents(analysis_state);
    if !diagnostics.is_empty() {
        return PackageLoweringAndNamingResult {
            modules,
            naming: NamingResult::default(),
            diagnostics,
        };
    }

    let modules = remap_package_modules(&modules).modules;
    let naming = resolve_package(&sorted_modules(&modules), analysis_state.interner());
    diagnostics.extend(naming.diagnostics.clone());

    PackageLoweringAndNamingResult {
        modules,
        naming,
        diagnostics,
    }
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
    module: Module,
    diagnostics: Vec<Diagnostic>,
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

    for (path, document) in package_documents {
        if should_refresh_document(
            &path,
            changed_document_paths,
            &analysis_state.lowered_documents,
        ) {
            analysis_state.lowered_documents.insert(
                path.clone(),
                lower_document(document, &mut analysis_state.lowering_context),
            );
        }
    }
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

fn lower_document(document: &Document, lowering_context: &mut LoweringContext) -> LoweredDocument {
    let lowering_result = lower_with_diagnostics(document, lowering_context);
    LoweredDocument {
        module: lowering_result.module,
        diagnostics: lowering_result.diagnostics,
    }
}

fn collect_lowered_documents(
    analysis_state: &AnalysisState,
) -> (HashMap<PathBuf, Module>, Vec<DocumentDiagnostics>) {
    let mut diagnostics = Vec::new();
    let mut modules = HashMap::new();

    for (path, lowered_document) in &analysis_state.lowered_documents {
        modules.insert(path.clone(), lowered_document.module.clone());
        if !lowered_document.diagnostics.is_empty() {
            diagnostics.push((path.clone(), lowered_document.diagnostics.clone()));
        }
    }

    (modules, diagnostics)
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
