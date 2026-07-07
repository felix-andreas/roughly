use {
    crate::{
        Interner,
        interner::Symbol,
        stub::{parse_stub_declarations, parse_stub_file},
        typecheck::{InferenceState, TypeDefinitionEnvironment},
        types::TypeScheme,
    },
    std::{
        collections::{BTreeMap, BTreeSet},
        path::{Path, PathBuf},
    },
    tree_sitter::Range,
};

// The shipped standard-library stub corpus: one declaration-only `.Rtypes` file per namespace, paired
// with the R namespace it declares. Each file is a flat list of `name : <type-expr>` declarations the
// loader harvests into type schemes. All shipped namespaces fold into the base environment, so their
// names resolve as bare globals; the origin tag is retained only for display on hover. See
// `stubs/base.Rtypes` for the format.
const SHIPPED_STUBS: &[(&str, &str)] = &[
    ("base", include_str!("../stubs/base.Rtypes")),
    ("stats", include_str!("../stubs/stats.Rtypes")),
    ("utils", include_str!("../stubs/utils.Rtypes")),
    ("methods", include_str!("../stubs/methods.Rtypes")),
    ("graphics", include_str!("../stubs/graphics.Rtypes")),
    ("grDevices", include_str!("../stubs/grDevices.Rtypes")),
];

// The extension of a stub file: R type information, declaration-only. The name mirrors R's own `.Rd`
// documentation convention without colliding with it.
pub const STUB_EXTENSION: &str = "Rtypes";

// An immutable description of the standard library's value bindings: each base name mapped to the
// `TypeScheme` harvested from its declaration. Built once when an analysis session starts and never
// invalidated by user edits.
//
// A stub is a base-environment binding only. Its schemes are seeded into the per-document inference
// template, so a bare base name resolves to its stub scheme, but a stub never becomes a package
// definition, a package global, or an interface export: a user binding over a base name shadows the stub,
// it does not redefine it.
#[derive(Debug, Clone, Default)]
pub struct StubLibrary {
    values: BTreeMap<Symbol, StubValue>,
    // Opaque nominal types the corpus declares (`@type data.frame`), keyed to their declaration
    // range. Seeded under every type-definition environment so a stub (or user annotation) can
    // name them; a project's own `@type`/`@alias` of the same name shadows the stub type.
    type_declarations: BTreeMap<Symbol, Range>,
    // The names each namespace declares, across every source (shipped and project). This is what
    // validates `pkg::name`: it is independent of the flat per-name winner fold in `values`,
    // because a project override winning a name's *type* must not un-export it from its shipped
    // namespace, and one name may be declared by several namespaces.
    exports_by_namespace: BTreeMap<Symbol, BTreeSet<Symbol>>,
}

#[derive(Debug, Clone)]
struct StubValue {
    // The name's ordered overload set: one scheme per declaration of the name within its source, in
    // declaration order. Most names declare exactly one. The first scheme is the primary signature
    // (bound as the plain global, shown on hover/completion); a call to a multi-scheme name probes
    // the set in order and commits the first scheme that accepts the arguments.
    schemes: Vec<TypeScheme>,
    range: Range,
    // The namespace of the winning declaration (`base`, `stats`, or a project stub's file stem),
    // shown on hover so the name reads as coming from its package. It does not affect resolution,
    // and `pkg::name` validation uses `exports_by_namespace`, not this tag.
    namespace: Option<Symbol>,
}

impl StubLibrary {
    // An empty library, used as the zero-cost baseline in benchmarks.
    pub fn empty() -> Self {
        Self::default()
    }

    // Loads the shipped standard-library stubs.
    pub fn load(interner: &mut Interner) -> Self {
        Self::load_with_overrides(interner, &[])
    }

    // Loads the shipped stubs, then folds project-supplied stub files over them: a binding a project
    // file defines replaces the shipped binding of the same name, so a project can correct or extend
    // the standard-library signatures it sees — or type a third-party package outright. A project
    // stub file's name declares its namespace (`stubs/dplyr.Rtypes` declares `dplyr`), so
    // `dplyr::mutate` validates and types like a shipped namespace read. Every stub name is interned
    // through the caller's interner, so a user reference and the stub share one symbol id.
    pub fn load_with_overrides(interner: &mut Interner, overrides: &[ProjectStubSource]) -> Self {
        // Types first, across every source: a value declaration may reference a type a later file
        // (or the same file, further down) declares, so harvesting runs against the full set.
        let mut type_declarations = BTreeMap::new();
        let shipped = SHIPPED_STUBS
            .iter()
            .map(|&(namespace, source)| (Some(interner.intern(namespace)), source))
            .collect::<Vec<_>>();
        let project = overrides
            .iter()
            .map(|stub_source| {
                let namespace = stub_source
                    .path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .filter(|stem| !stem.is_empty())
                    .map(|stem| interner.intern(stem));
                (namespace, stub_source.source.as_str())
            })
            .collect::<Vec<_>>();
        let sources = shipped.into_iter().chain(project).collect::<Vec<_>>();
        for &(_, source) in &sources {
            let stub_file = parse_stub_file(source, interner);
            for type_declaration in stub_file.types {
                type_declarations
                    .entry(type_declaration.name)
                    .or_insert(type_declaration.range);
            }
        }
        let mut type_definitions = TypeDefinitionEnvironment::default();
        for symbol in type_declarations.keys() {
            type_definitions.seed_opaque_type(*symbol);
        }

        let mut values = BTreeMap::new();
        let mut exports_by_namespace: BTreeMap<Symbol, BTreeSet<Symbol>> = BTreeMap::new();
        for (namespace, source) in sources {
            let declared =
                harvest_stub_source(interner, source, namespace, &type_definitions, &mut values);
            if let Some(namespace) = namespace {
                exports_by_namespace
                    .entry(namespace)
                    .or_default()
                    .extend(declared);
            }
        }

        debug_assert!(
            !values.is_empty(),
            "shipped stub corpus yielded no schemes; a stub file failed to parse"
        );
        Self {
            values,
            type_declarations,
            exports_by_namespace,
        }
    }

    // Seeds the corpus's opaque nominal types into a type-definition environment. Call after the
    // environment is built from modules: seeding never overwrites, so user declarations win.
    pub fn seed_type_definitions(&self, type_definitions: &mut TypeDefinitionEnvironment) {
        for symbol in self.type_declarations.keys() {
            type_definitions.seed_opaque_type(*symbol);
        }
    }

    // Whether `namespace` is one the corpus models: a shipped namespace (`base`, `stats`, …) or a
    // project stub file's namespace (`stubs/dplyr.Rtypes` → `dplyr`). An unknown namespace is a
    // diagnostic at `pkg::name` sites; a known one gates the export check below.
    pub fn is_known_namespace(&self, namespace: Symbol) -> bool {
        self.exports_by_namespace.contains_key(&namespace)
    }

    // Whether `namespace` declares `symbol` in any of its sources. Declaration-level, not
    // winner-level: a project override winning `sd`'s type does not un-export `sd` from `stats`.
    pub fn namespace_exports(&self, namespace: Symbol, symbol: Symbol) -> bool {
        self.exports_by_namespace
            .get(&namespace)
            .is_some_and(|names| names.contains(&symbol))
    }

    pub fn contains(&self, symbol: Symbol) -> bool {
        self.values.contains_key(&symbol)
    }

    // Binds every stub scheme as a base global into `inference_state`, so a document's check resolves a
    // bare base name to its stub scheme. The schemes live only in the template environment.
    //
    // Each scheme is *imported* first: it was harvested in a throwaway inference state, so a generic
    // scheme's quantified (and body) variables carry that state's variable ids. Importing re-binds them
    // to fresh ids owned by `inference_state`; without it a call to a generic stub instantiates against
    // variable ids the consuming state never registered, failing with "unknown inference variable". A
    // monomorphic scheme has no variables, so importing is a no-op for it.
    pub fn seed_into(&self, inference_state: &mut InferenceState) {
        for (symbol, value) in &self.values {
            let imported = value
                .schemes
                .iter()
                .map(|scheme| inference_state.import_scheme(scheme))
                .collect::<Vec<_>>();
            let Some(first) = imported.first() else {
                continue;
            };
            inference_state.bind_global_scheme(*symbol, first.clone(), value.range);
            if imported.len() > 1 {
                inference_state.bind_overload_set(*symbol, imported);
            }
        }
    }

    pub fn symbols(&self) -> impl Iterator<Item = Symbol> + '_ {
        self.values.keys().copied()
    }

    // One `(symbol, scheme)` pair per stub name, using the primary (first-declared) scheme of an
    // overloaded name — the display surfaces (completion detail, corpus audits) want one signature.
    pub fn schemes(&self) -> impl Iterator<Item = (Symbol, &TypeScheme)> + '_ {
        self.values
            .iter()
            .filter_map(|(symbol, value)| value.schemes.first().map(|scheme| (*symbol, scheme)))
    }

    // The namespace of a stub name's winning declaration (`base`, `stats`, a project stub's file
    // stem), for display on hover. `None` when the symbol is not a stub.
    pub fn namespace_of(&self, symbol: Symbol) -> Option<Symbol> {
        self.values.get(&symbol)?.namespace
    }

    // The full declared overload set of a stub name, in declaration order — empty for a non-stub
    // symbol. Signature help lists every candidate of a multi-scheme name; single-scheme names
    // return their one scheme.
    pub fn overload_schemes(&self, symbol: Symbol) -> &[TypeScheme] {
        self.values
            .get(&symbol)
            .map(|value| value.schemes.as_slice())
            .unwrap_or(&[])
    }
}

// The type-definition environment an open `.Rtypes` editor buffer harvests against: the shipped
// corpus's opaque `@type` declarations plus the buffer's own. Without this, a valid override
// declaration returning a shipped nominal (`data.frame`) would falsely report as unloadable.
pub fn stub_editing_type_definitions(
    interner: &mut Interner,
    buffer_source: &str,
) -> TypeDefinitionEnvironment {
    let mut type_definitions = TypeDefinitionEnvironment::default();
    for &(_, source) in SHIPPED_STUBS {
        seed_stub_source_types(interner, source, &mut type_definitions);
    }
    seed_stub_source_types(interner, buffer_source, &mut type_definitions);
    type_definitions
}

// One declaration the stub loader drops from a source, located by its (zero-based) line: a line
// that fails to parse, or a declaration that parses but fails to harvest into a scheme (an
// unresolvable type name, for example). Every surface that reports stub feedback — the editor's
// `.Rtypes` buffer diagnostics and `roughly check`'s override report — renders from this one list,
// so the wording stays identical.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StubProblem {
    pub line: usize,
    pub message: String,
}

// Everything the loader drops from one stub source against `type_definitions`, ordered by line.
pub fn stub_source_problems(
    interner: &mut Interner,
    source: &str,
    type_definitions: &TypeDefinitionEnvironment,
) -> Vec<StubProblem> {
    let (declarations, errors) = parse_stub_declarations(source, interner);
    let mut problems: Vec<StubProblem> = errors
        .into_iter()
        .map(|error| StubProblem {
            line: error.line,
            message: error.message,
        })
        .collect();
    let mut inference_state = InferenceState::new();
    for declaration in &declarations {
        let Err(error) =
            inference_state.harvest_annotation_scheme(&declaration.surface_type, type_definitions)
        else {
            continue;
        };
        let rendered = crate::diagnostic::Diagnostic::from_inference_error(
            &error,
            declaration.range,
            interner,
        );
        problems.push(StubProblem {
            line: declaration.range.start_point.row,
            message: format!("this declaration does not load: {}", rendered.message),
        });
    }
    problems.sort_by_key(|problem| problem.line);
    problems
}

// The names each shipped namespace declares, keyed by namespace in `SHIPPED_STUBS` order. Names are
// extracted with the real stub parser (not a second scanner), so this stays in step with what the loader
// harvests. A name repeated within one file appears once. Used by the exhaustiveness check that diffs the
// declared corpus against a curated list of each namespace's real R exports.
pub fn shipped_stub_names_by_namespace() -> Vec<(&'static str, Vec<String>)> {
    let mut interner = Interner::default();
    SHIPPED_STUBS
        .iter()
        .map(|&(namespace, source)| {
            let mut names = stub_source_declared_names(source, &mut interner);
            names.sort();
            names.dedup();
            (namespace, names)
        })
        .collect()
}

// The full export table — each namespace mapped to its declared names — across the shipped corpus
// plus the given project override stubs, folded exactly as the loader folds them (a project file's
// namespace is its file stem, matching `load_with_overrides`). Interner-free and string-keyed, so
// the NAMESPACE validator can run against a live editor buffer without touching the shared
// interner. Built once per session (set-once ambient state), not per validation.
pub fn stub_names_by_namespace(
    project_sources: &[ProjectStubSource],
) -> BTreeMap<String, BTreeSet<String>> {
    let mut interner = Interner::default();
    let mut exports: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let shipped = SHIPPED_STUBS
        .iter()
        .map(|&(namespace, source)| (namespace.to_owned(), source));
    let project = project_sources.iter().filter_map(|stub_source| {
        let namespace = stub_source
            .path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| !stem.is_empty())?;
        Some((namespace.to_owned(), stub_source.source.as_str()))
    });
    for (namespace, source) in shipped.chain(project) {
        exports
            .entry(namespace)
            .or_default()
            .extend(stub_source_declared_names(source, &mut interner));
    }
    exports
}

// The names one stub source declares, resolved to owned strings against `interner`. Uses the real
// stub declaration parser so it cannot drift from what the loader harvests.
fn stub_source_declared_names(source: &str, interner: &mut Interner) -> Vec<String> {
    let (declarations, _errors) = parse_stub_declarations(source, interner);
    declarations
        .iter()
        .filter_map(|declaration| interner.resolve(declaration.name))
        .map(str::to_owned)
        .collect()
}

// One readable project override stub file, in `<root>/stubs/*.Rtypes`.
#[derive(Debug, Clone)]
pub struct ProjectStubSource {
    pub path: PathBuf,
    pub source: String,
}

// A project's override stub files: the readable ones (in sorted path order — the order the loader
// folds them) plus the paths that could not be read, with the read error. Overrides are optional
// and a broken one must never block analysis, so consumers load `sources` and decide how loudly to
// report `unreadable` (`roughly check` reports it as an I/O failure; the server logs it).
#[derive(Debug, Clone, Default)]
pub struct ProjectStubs {
    pub sources: Vec<ProjectStubSource>,
    pub unreadable: Vec<(PathBuf, String)>,
}

// Reads a project's override stub files from `<root>/stubs/*.Rtypes`. A project drops
// declaration-only stub files there to override or extend the shipped standard-library stubs. A
// missing directory means no overrides.
pub fn discover_project_stubs(root: &Path) -> ProjectStubs {
    let stubs_directory = root.join("stubs");
    let Ok(entries) = std::fs::read_dir(&stubs_directory) else {
        return ProjectStubs::default();
    };
    let mut paths = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case(STUB_EXTENSION))
        })
        .collect::<Vec<_>>();
    paths.sort();
    let mut project_stubs = ProjectStubs::default();
    for path in paths {
        match std::fs::read_to_string(&path) {
            Ok(source) => project_stubs
                .sources
                .push(ProjectStubSource { path, source }),
            Err(error) => project_stubs.unreadable.push((path, error.to_string())),
        }
    }
    project_stubs
}

// The per-file problem lists for a set of override sources, parallel to `sources`. Harvest runs
// exactly as `load_with_overrides` runs it — opaque types are collected across the shipped corpus
// and every override first — so a declaration referencing a type another override file declares
// does not falsely report as unloadable.
pub fn stub_override_problems(sources: &[ProjectStubSource]) -> Vec<Vec<StubProblem>> {
    let mut interner = Interner::new();
    let mut type_definitions = TypeDefinitionEnvironment::default();
    for &(_, source) in SHIPPED_STUBS {
        seed_stub_source_types(&mut interner, source, &mut type_definitions);
    }
    for stub_source in sources {
        seed_stub_source_types(&mut interner, &stub_source.source, &mut type_definitions);
    }
    sources
        .iter()
        .map(|stub_source| {
            stub_source_problems(&mut interner, &stub_source.source, &type_definitions)
        })
        .collect()
}

fn seed_stub_source_types(
    interner: &mut Interner,
    source: &str,
    type_definitions: &mut TypeDefinitionEnvironment,
) {
    for type_declaration in parse_stub_file(source, interner).types {
        type_definitions.seed_opaque_type(type_declaration.name);
    }
}

// Parses one stub source and harvests each declaration's type expression into a scheme keyed by the
// declared name, returning every name the source declares (parse-level, so `pkg::name` validation
// covers a declaration even when its scheme fails to load). Declaring the same name more than once
// *within one source* builds an ordered overload set; a later *source* (a project stub) still
// replaces the whole set for that name, so an override that wants overloads declares all of them
// itself. A declaration whose type expression fails to lower into a scheme is skipped, and lines
// that fail to parse are dropped by the declaration parser, so a malformed project stub degrades
// gracefully rather than aborting analysis.
fn harvest_stub_source(
    interner: &mut Interner,
    source: &str,
    namespace: Option<Symbol>,
    type_definitions: &TypeDefinitionEnvironment,
    values: &mut BTreeMap<Symbol, StubValue>,
) -> BTreeSet<Symbol> {
    let (declarations, _errors) = parse_stub_declarations(source, interner);
    let mut inference_state = InferenceState::new();
    let mut declared_in_this_source = BTreeSet::new();
    let mut harvested_in_this_source = BTreeSet::new();
    for declaration in declarations {
        declared_in_this_source.insert(declaration.name);
        let Ok(scheme) =
            inference_state.harvest_annotation_scheme(&declaration.surface_type, type_definitions)
        else {
            continue;
        };
        if harvested_in_this_source.contains(&declaration.name) {
            if let Some(value) = values.get_mut(&declaration.name) {
                value.schemes.push(scheme);
            }
            continue;
        }
        harvested_in_this_source.insert(declaration.name);
        values.insert(
            declaration.name,
            StubValue {
                schemes: vec![scheme],
                range: declaration.range,
                namespace,
            },
        );
    }
    declared_in_this_source
}
