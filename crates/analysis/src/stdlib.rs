use {
    crate::{
        Interner,
        interner::Symbol,
        stub::parse_stub_declarations,
        typecheck::{InferenceState, TypeDefinitionEnvironment},
        types::TypeScheme,
    },
    std::{collections::BTreeMap, path::Path},
    tree_sitter::Range,
};

// The shipped standard-library stub corpus: one declaration-only `.Rti` file per namespace. Each file is
// a flat list of `name : <type-expr>` declarations the loader harvests into type schemes. All shipped
// namespaces are attached to the base environment, so their names resolve as bare globals. See
// `stubs/base.Rti` for the format.
const SHIPPED_STUBS: &[&str] = &[
    include_str!("../stubs/base.Rti"),
    include_str!("../stubs/stats.Rti"),
    include_str!("../stubs/utils.Rti"),
    include_str!("../stubs/methods.Rti"),
];

// The extension of a stub file: R type information, declaration-only. The name mirrors R's own `.Rd`
// documentation convention without colliding with it.
pub const STUB_EXTENSION: &str = "Rti";

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
    scheme: TypeScheme,
    range: Range,
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
        for source in SHIPPED_STUBS {
            harvest_stub_source(interner, source, &mut values);
        }
        for source in override_sources {
            harvest_stub_source(interner, source, &mut values);
        }

        debug_assert!(
            !values.is_empty(),
            "shipped stub corpus yielded no schemes; a stub file failed to parse"
        );
        Self { values }
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
            let imported = inference_state.import_scheme(&value.scheme);
            inference_state.bind_global_scheme(*symbol, imported, value.range);
        }
    }

    pub fn symbols(&self) -> impl Iterator<Item = Symbol> + '_ {
        self.values.keys().copied()
    }
}

// Reads a project's override stub files from `<root>/stubs/*.Rti`, in sorted path order, returning their
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
// declared name (a later source overrides an earlier one for the same name, and — until overload sets
// exist — so does a later declaration of the same name within one source). A declaration whose type
// expression fails to lower into a scheme is skipped, and lines that fail to parse are dropped by the
// declaration parser, so a malformed project override degrades gracefully rather than aborting analysis.
fn harvest_stub_source(
    interner: &mut Interner,
    source: &str,
    values: &mut BTreeMap<Symbol, StubValue>,
) {
    let (declarations, _errors) = parse_stub_declarations(source, interner);
    let mut inference_state = InferenceState::new();
    let type_definitions = TypeDefinitionEnvironment::default();
    for declaration in declarations {
        let Ok(scheme) =
            inference_state.harvest_annotation_scheme(&declaration.surface_type, &type_definitions)
        else {
            continue;
        };
        values.insert(
            declaration.name,
            StubValue {
                scheme,
                range: declaration.range,
            },
        );
    }
}
