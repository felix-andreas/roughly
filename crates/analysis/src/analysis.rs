use {
    crate::{
        Interner,
        diagnostic::Diagnostic,
        document::{Document, DocumentChange, DocumentId},
        hir::{ExpressionId, Module},
        lint::{self as lint_phase, NameStyle},
        lower::lower_with_shared_interner,
        naming::{
            DocumentKind, NamesGlobal, NamesLocal, rebuild_package_naming,
            resolve_document_locally,
        },
        tree,
        type_syntax::render_surface_type,
        typecheck::{
            ExportedValue, TypeDefinitionEnvironment, inference_state_with_builtins_in_interner,
        },
        types::{CoreType, TypeScheme},
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
    document_interface_outputs: HashMap<DocumentId, InterfaceOutput>,
    document_typecheck_outputs: HashMap<DocumentId, TypecheckDocumentOutput>,
    // The package version at which `typecheck` last completed. Since the package version bumps on
    // every document or config change, an unchanged version means every typecheck output is already
    // current, so a repeated call (e.g. successive hover or inlay-hint requests) returns at once.
    last_typecheck_package_version: Option<Version>,
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
    // Surface unused-local-binding warnings.
    pub unused: bool,
    // Surface type-error diagnostics. The typecheck phase still runs on demand for typing IDE
    // features (hover types, inlay hints, signature help) regardless of this flag; it only controls
    // whether type errors are reported.
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
struct InterfaceOutput {
    version: Version,
    type_definitions_fingerprint: String,
    // Fingerprint of the package-global schemes this document references. A document's exported
    // interface is recomputed when its own version, the type definitions, or any scheme it depends
    // on changes, so an edit only re-derives the affected documents.
    dependency_fingerprint: String,
    exports: Vec<ExportedValue>,
    fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TypecheckDocumentOutput {
    version: Version,
    type_definitions_fingerprint: String,
    // Fingerprint of exactly the package-global schemes this document references, rendered against
    // the converged interface table. The round-2 check is recomputed only when its own version, the
    // type definitions, or one of the schemes it actually depends on changes, so a value-interface
    // change rechecks only the documents that reference the changed name.
    dependency_fingerprint: String,
    diagnostics: Vec<Diagnostic>,
    expression_types: HashMap<ExpressionId, CoreType>,
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
            document_interface_outputs: HashMap::new(),
            document_typecheck_outputs: HashMap::new(),
            last_typecheck_package_version: None,
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

    pub fn base_path(&self) -> &Path {
        &self.base_path
    }

    // Whether type-error diagnostics are surfaced. The typecheck phase still runs on demand for
    // typing IDE features regardless of this flag; it only gates publishing type errors.
    pub fn type_errors_enabled(&self) -> bool {
        self.check_config.typing
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
            if self.check_config.unused {
                for binding_id in &output.output.unused_bindings {
                    if let Some(binding) = output.output.bindings.get(binding_id) {
                        let name = self.interner().resolve(binding.symbol).unwrap_or("<unknown>");
                        diagnostics.push(Diagnostic::naming_warning(
                            binding.range,
                            format!("`{name}` is assigned but never used."),
                        ));
                    }
                }
            }
        }
        if let Some(output) = &self.package_naming_output
            && let Some(package_diagnostics) = output.diagnostics.get(&document_id)
        {
            diagnostics.extend(package_diagnostics.iter().cloned());
        }
        if self.check_config.typing
            && let Some(output) = self.document_typecheck_outputs.get(&document_id)
        {
            diagnostics.extend(output.diagnostics.iter().cloned());
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
}

// Returns the documents whose typecheck output changed, so callers can republish exactly
// those diagnostics after a package-visible edit.
pub fn run_full(analysis_state: &mut Analysis) -> Vec<DocumentId> {
    lint(analysis_state);

    if analysis_state.check_config.typing {
        typecheck(analysis_state)
    } else {
        resolve_package(analysis_state);
        Vec::new()
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
        let document_kind = if analysis_state.non_package_documents.contains(document_id) {
            DocumentKind::Script
        } else {
            DocumentKind::Package
        };
        let local_naming =
            resolve_document_locally(*document_id, module, analysis_state.interner(), document_kind);
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

// Fingerprint of exactly the package-global schemes a document references, used as part of its
// interface cache key so a document recomputes only when a dependency's scheme changes.
fn render_dependency_fingerprint(
    referenced: &std::collections::BTreeSet<crate::Symbol>,
    table: &std::collections::BTreeMap<crate::Symbol, ExportedValue>,
    interner: &Interner,
) -> String {
    referenced
        .iter()
        .filter_map(|symbol| table.get(symbol).map(|export| (symbol, export)))
        .map(|(symbol, export)| {
            format!(
                "{}: {}",
                interner.resolve(*symbol).unwrap_or("<unknown>"),
                crate::diagnostic::render_type_scheme(interner, &export.type_scheme)
            )
        })
        .collect::<Vec<_>>()
        .join(";")
}

// Typechecking is incremental at document grain. Round 1 computes each package document's
// exported value schemes in isolation (cross-file references check as `Unknown`), cached by
// document version plus the package type-definition fingerprint. Round 2 checks every
// document against the package interface table built from round 1, cached by document
// version, the type-definition fingerprint, plus that document's own dependency fingerprint
// (the schemes it references, rendered against the converged table), so a value-interface change
// rechecks only the documents that reference the changed name. Returns the documents whose
// typecheck output was recomputed, so callers can republish exactly those diagnostics.
pub fn typecheck(analysis_state: &mut Analysis) -> Vec<DocumentId> {
    // Nothing has changed since the last completed typecheck, so every cached output is current and
    // no document needs rechecking. This keeps repeated IDE requests on an unchanged package cheap.
    if analysis_state.last_typecheck_package_version == Some(analysis_state.package_version) {
        return Vec::new();
    }

    resolve_package(analysis_state);

    let package_document_ids = analysis_state.package_document_ids();
    let all_document_ids = analysis_state.all_document_ids();
    let template_state = inference_state_with_builtins_in_interner(analysis_state.interner_mut());
    let fallback_range = analysis_state.fallback_range();

    let (type_definitions, type_definitions_fingerprint) = {
        let package_modules = package_document_ids
            .iter()
            .map(|document_id| {
                analysis_state.module(*document_id).unwrap_or_else(|| {
                    panic!("missing lowered module for typecheck {document_id:?}")
                })
            })
            .collect::<Vec<_>>();
        let type_definitions =
            TypeDefinitionEnvironment::from_modules(package_modules.iter().copied());
        let mut rendered_definitions = Vec::new();
        for module in &package_modules {
            for definition_item in &module.definitions {
                let definition = &definition_item.definition;
                let name = analysis_state
                    .interner()
                    .resolve(definition.name)
                    .unwrap_or("<unknown>");
                let parameters = definition
                    .type_parameters
                    .iter()
                    .map(|parameter| {
                        analysis_state
                            .interner()
                            .resolve(*parameter)
                            .unwrap_or("<unknown>")
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                rendered_definitions.push(format!(
                    "{:?} {name}<{parameters}> = {}",
                    definition.kind,
                    render_surface_type(&definition.surface_type, analysis_state.interner())
                ));
            }
        }
        (type_definitions, rendered_definitions.join(";"))
    };

    let package_naming = analysis_state
        .package_naming_output
        .as_ref()
        .unwrap_or_else(|| panic!("missing package naming output for typecheck"))
        .output
        .clone();

    // Package interface fixed-point. Each round builds the current package-global table, recomputes
    // every document whose version, the type definitions, or a referenced scheme changed, then
    // rebuilds the table from the fresh exports. Because a document's interface is checked against
    // that table, re-exports and forward references resolve within and across files; the
    // dependency-fingerprint cache keeps an edit from re-deriving documents it cannot affect, and
    // the round cap stops genuine cycles (which keep `Unknown`).
    const MAX_PACKAGE_INTERFACE_ROUNDS: usize = 32;
    for _round in 0..MAX_PACKAGE_INTERFACE_ROUNDS {
        let table = build_package_interface_table(
            &package_naming,
            &analysis_state.document_interface_outputs,
            fallback_range,
        );

        let mut fresh_interfaces = Vec::new();
        for document_id in &package_document_ids {
            let Some(document_version) = analysis_state.document_version(*document_id) else {
                continue;
            };
            let local_naming = analysis_state
                .document_naming(*document_id)
                .unwrap_or_else(|| panic!("missing local naming for typecheck {document_id:?}"));
            let referenced = local_naming
                .non_locals
                .values()
                .copied()
                .collect::<std::collections::BTreeSet<_>>();
            let dependency_fingerprint =
                render_dependency_fingerprint(&referenced, &table, analysis_state.interner());

            if analysis_state
                .document_interface_outputs
                .get(document_id)
                .is_some_and(|output| {
                    output.version == document_version
                        && output.type_definitions_fingerprint == type_definitions_fingerprint
                        && output.dependency_fingerprint == dependency_fingerprint
                })
            {
                continue;
            }

            let module = analysis_state.module(*document_id).unwrap_or_else(|| {
                panic!("missing lowered module for typecheck {document_id:?}")
            });
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
            fresh_interfaces.push((
                *document_id,
                InterfaceOutput {
                    version: document_version,
                    type_definitions_fingerprint: type_definitions_fingerprint.clone(),
                    dependency_fingerprint,
                    exports,
                    fingerprint,
                },
            ));
        }

        if fresh_interfaces.is_empty() {
            break;
        }
        for (document_id, output) in fresh_interfaces {
            analysis_state
                .document_interface_outputs
                .insert(document_id, output);
        }
    }

    // The converged package interface table. It both keys each document's round-2 cache (via the
    // per-document dependency fingerprint rendered against it) and supplies the schemes bound when a
    // document actually runs inference.
    let final_table = build_package_interface_table(
        &package_naming,
        &analysis_state.document_interface_outputs,
        fallback_range,
    );

    let mut recomputed_document_ids = Vec::new();
    let mut fresh_outputs = Vec::new();
    for document_id in &all_document_ids {
        let Some(document_version) = analysis_state.document_version(*document_id) else {
            continue;
        };
        let local_naming = analysis_state
            .document_naming(*document_id)
            .unwrap_or_else(|| panic!("missing local naming for typecheck {document_id:?}"));
        // Only the names this document actually references need to be in scope; binding the
        // whole package interface would make every check linear in package size. The same set keys
        // the cache: a document rechecks only when its version or one of these schemes changes.
        let referenced_symbols = local_naming
            .non_locals
            .values()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        let per_doc_dependency_fingerprint = render_dependency_fingerprint(
            &referenced_symbols,
            &final_table,
            analysis_state.interner(),
        );

        if analysis_state
            .document_typecheck_outputs
            .get(document_id)
            .is_some_and(|output| {
                output.version == document_version
                    && output.type_definitions_fingerprint == type_definitions_fingerprint
                    && output.dependency_fingerprint == per_doc_dependency_fingerprint
            })
        {
            continue;
        }

        let module = analysis_state
            .module(*document_id)
            .unwrap_or_else(|| panic!("missing lowered module for typecheck {document_id:?}"));
        // Script-local type declarations are visible only inside the script itself and
        // shadow package definitions of the same name.
        let document_type_definitions =
            if analysis_state.non_package_documents.contains(document_id) {
                let package_modules = package_document_ids.iter().filter_map(|package_document_id| {
                    analysis_state.module(*package_document_id)
                });
                TypeDefinitionEnvironment::from_modules(package_modules.chain([module]))
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
        // Round 2 is the authoritative per-document check, so it records expression types for
        // hover and inlay hints.
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
        let expression_types = module_check
            .expression_types_by_id
            .into_iter()
            .collect::<HashMap<_, _>>();
        fresh_outputs.push((
            *document_id,
            TypecheckDocumentOutput {
                version: document_version,
                type_definitions_fingerprint: type_definitions_fingerprint.clone(),
                dependency_fingerprint: per_doc_dependency_fingerprint,
                diagnostics,
                expression_types,
            },
        ));
        recomputed_document_ids.push(*document_id);
    }
    for (document_id, output) in fresh_outputs {
        analysis_state
            .document_typecheck_outputs
            .insert(document_id, output);
    }

    analysis_state.last_typecheck_package_version = Some(analysis_state.package_version);
    recomputed_document_ids
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
        self.lint_outputs.remove(&document_id);
        self.lowering_outputs.remove(&document_id);
        self.document_naming_outputs.remove(&document_id);
        self.document_interface_outputs.remove(&document_id);
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
            Diagnostic, Severity,
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
            analysis.document_typecheck_outputs.contains_key(&document_id),
            "the typecheck phase must still run and retain output for IDE features"
        );

        // typing on: the type error is surfaced as a diagnostic.
        let mut analysis = Analysis::new(
            PathBuf::from("/workspace"),
            LintConfig::default(),
            CheckConfig {
                unused: false,
                typing: true,
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
    fn typecheck_reuses_cached_document_output_when_nothing_changed() {
        let mut analysis = Analysis::new(
            PathBuf::from("/workspace"),
            LintConfig::default(),
            CheckConfig::default(),
        );
        let document_id = analysis
            .add_document_from_source(PathBuf::from("/workspace/R/main.R"), "1L + \"text\"")
            .expect("document should parse");

        let first_run = super::typecheck(&mut analysis);
        assert_eq!(first_run, vec![document_id]);
        let initial_diagnostics = analysis
            .document_typecheck_outputs
            .get(&document_id)
            .map(|output| output.diagnostics.clone())
            .unwrap_or_default();
        assert!(!initial_diagnostics.is_empty());

        let sentinel =
            Diagnostic::type_error(initial_diagnostics[0].range, "cached typecheck sentinel");
        analysis
            .document_typecheck_outputs
            .get_mut(&document_id)
            .expect("typecheck output should exist")
            .diagnostics = vec![sentinel.clone()];

        let second_run = super::typecheck(&mut analysis);
        assert!(second_run.is_empty());
        let cached_diagnostics = analysis
            .document_typecheck_outputs
            .get(&document_id)
            .map(|output| output.diagnostics.clone())
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

        assert!(
            hover
                .contents
                .iter()
                .any(|block| block.contains("Local variable, defined at `R/main.R:1:19`")),
            "{:?}",
            hover.contents
        );
        assert!(
            hover.debug.iter().any(|section| section.title == "Lowering"
                && section.body.contains("Symbol(parameter)"))
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

        assert!(
            hover
                .contents
                .iter()
                .any(|block| block.contains("Package global, defined at `R/a.R:1:1`")),
            "{:?}",
            hover.contents
        );
        assert!(
            hover.debug.iter().any(|section| section.title == "Naming"
                && section.body.contains("package resolution: binding `value` at R/a.R:1:1"))
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
