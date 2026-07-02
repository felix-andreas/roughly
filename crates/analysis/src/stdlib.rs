use {
    crate::{
        Interner,
        interner::Symbol,
        stub::parse_stub_declarations,
        typecheck::{InferenceState, TypeDefinitionEnvironment},
        types::TypeScheme,
    },
    std::{
        collections::{BTreeMap, BTreeSet},
        path::Path,
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
}

#[derive(Debug, Clone)]
struct StubValue {
    // The name's ordered overload set: one scheme per declaration of the name within its source, in
    // declaration order. Most names declare exactly one. The first scheme is the primary signature
    // (bound as the plain global, shown on hover/completion); a call to a multi-scheme name probes
    // the set in order and commits the first scheme that accepts the arguments.
    schemes: Vec<TypeScheme>,
    range: Range,
    // The R namespace the stub was declared in (`base`, `stats`, …), or `None` for a project override.
    // Shown on hover so a stdlib name reads as coming from its package; it does not affect resolution.
    namespace: Option<&'static str>,
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

    // Loads the shipped stubs, then folds project-supplied override sources over them: a binding an
    // override defines replaces the shipped binding of the same name, so a project can correct or extend
    // the standard-library signatures it sees (a more precise return type, or a name the shipped corpus
    // omits). Every stub name is interned through the caller's interner, so a user reference and the stub
    // share one symbol id.
    pub fn load_with_overrides(interner: &mut Interner, override_sources: &[String]) -> Self {
        let mut values = BTreeMap::new();
        for &(namespace, source) in SHIPPED_STUBS {
            harvest_stub_source(interner, source, Some(namespace), &mut values);
        }
        for source in override_sources {
            harvest_stub_source(interner, source, None, &mut values);
        }

        debug_assert!(
            !values.is_empty(),
            "shipped stub corpus yielded no schemes; a stub file failed to parse"
        );
        Self { values }
    }

    // Whether `namespace` is one the shipped corpus models (`base`, `stats`, …). An unknown
    // namespace is a diagnostic at `pkg::name` sites; a known one gates the export check below.
    pub fn is_known_namespace(&self, namespace: &str) -> bool {
        SHIPPED_STUBS
            .iter()
            .any(|(shipped_namespace, _)| *shipped_namespace == namespace)
    }

    // Whether the corpus declares `symbol` in `namespace`. The corpus folds flat (one winner per
    // name), so a name declared by a different shipped namespace does not count for this one.
    pub fn namespace_exports(&self, namespace: &str, symbol: Symbol) -> bool {
        self.values
            .get(&symbol)
            .and_then(|value| value.namespace)
            .is_some_and(|shipped_namespace| shipped_namespace == namespace)
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

    // The R namespace a stub name was declared in (`base`, `stats`, …), for display on hover. `None`
    // when the symbol is not a stub, or when it comes from a project override (no shipped namespace).
    pub fn namespace_of(&self, symbol: Symbol) -> Option<&'static str> {
        self.values.get(&symbol)?.namespace
    }
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
            let (declarations, _errors) = parse_stub_declarations(source, &mut interner);
            let mut names = declarations
                .iter()
                .filter_map(|declaration| interner.resolve(declaration.name))
                .map(str::to_owned)
                .collect::<Vec<_>>();
            names.sort();
            names.dedup();
            (namespace, names)
        })
        .collect()
}

// Reads a project's override stub files from `<root>/stubs/*.Rtypes`, in sorted path order, returning their
// source text. A project drops declaration-only stub files there to override or extend the shipped
// standard-library stubs. A missing directory or an unreadable file is silently skipped: overrides are
// optional, and a malformed override must never block analysis.
pub fn discover_project_stub_sources(root: &Path) -> Vec<String> {
    let stubs_directory = root.join("stubs");
    let Ok(entries) = std::fs::read_dir(&stubs_directory) else {
        return Vec::new();
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
    paths
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .collect()
}

// Parses one stub source and harvests each declaration's type expression into a scheme keyed by the
// declared name. Declaring the same name more than once *within one source* builds an ordered
// overload set; a later *source* (a project override) still replaces the whole set for that name, so
// an override that wants overloads declares all of them itself. A declaration whose type expression
// fails to lower into a scheme is skipped, and lines that fail to parse are dropped by the
// declaration parser, so a malformed project override degrades gracefully rather than aborting analysis.
fn harvest_stub_source(
    interner: &mut Interner,
    source: &str,
    namespace: Option<&'static str>,
    values: &mut BTreeMap<Symbol, StubValue>,
) {
    let (declarations, _errors) = parse_stub_declarations(source, interner);
    let mut inference_state = InferenceState::new();
    let type_definitions = TypeDefinitionEnvironment::default();
    let mut declared_in_this_source = BTreeSet::new();
    for declaration in declarations {
        let Ok(scheme) =
            inference_state.harvest_annotation_scheme(&declaration.surface_type, &type_definitions)
        else {
            continue;
        };
        if declared_in_this_source.contains(&declaration.name) {
            if let Some(value) = values.get_mut(&declaration.name) {
                value.schemes.push(scheme);
            }
            continue;
        }
        declared_in_this_source.insert(declaration.name);
        // A project override of a shipped name replaces the scheme but keeps the shipped
        // namespace tag: the override refines the type of `stats::sd`, it does not move `sd`
        // out of `stats` — dropping the tag would make `stats::sd` warn "not exported".
        let namespace = namespace.or_else(|| {
            values
                .get(&declaration.name)
                .and_then(|existing| existing.namespace)
        });
        values.insert(
            declaration.name,
            StubValue {
                schemes: vec![scheme],
                range: declaration.range,
                namespace,
            },
        );
    }
}
