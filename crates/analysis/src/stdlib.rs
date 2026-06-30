use {
    crate::{
        Interner,
        document::Document,
        hir::ExpressionKind,
        interner::Symbol,
        lower::lower_with_shared_interner,
        tree,
        typecheck::{InferenceState, TypeDefinitionEnvironment},
        types::{Annotation, TypeScheme},
    },
    std::{collections::BTreeMap, path::Path},
    tree_sitter::{Parser, Range},
};

// The shipped standard-library stub corpus: one declaration-only R file per namespace. Each file's
// top-level bindings carry `#:` annotations the loader harvests into type schemes; the placeholder bodies
// are never inferred. All shipped namespaces are attached to the base environment, so their names resolve
// as bare globals. See `stubs/base.R` for the format.
const SHIPPED_STUBS: &[&str] = &[
    include_str!("../stubs/base.R"),
    include_str!("../stubs/stats.R"),
    include_str!("../stubs/utils.R"),
    include_str!("../stubs/methods.R"),
];

// An immutable description of the standard library's value bindings: each base name mapped to the
// `TypeScheme` harvested from its `#:` annotation. Built once when an analysis session starts and never
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
        let mut parser = tree::new_parser().expect("stub parser should initialize");
        let mut values = BTreeMap::new();
        for source in SHIPPED_STUBS {
            harvest_stub_source(&mut parser, interner, source, &mut values);
        }
        for source in override_sources {
            harvest_stub_source(&mut parser, interner, source, &mut values);
        }

        debug_assert!(
            !values.is_empty(),
            "shipped stub corpus yielded no schemes; a stub file failed to parse or lower"
        );
        Self { values }
    }

    pub fn contains(&self, symbol: Symbol) -> bool {
        self.values.contains_key(&symbol)
    }

    // Binds every stub scheme as a base global into `inference_state`, so a document's check resolves a
    // bare base name to its stub scheme. The schemes live only in the template environment.
    pub fn seed_into(&self, inference_state: &mut InferenceState) {
        for (symbol, value) in &self.values {
            inference_state.bind_global_scheme(*symbol, value.scheme.clone(), value.range);
        }
    }

    pub fn symbols(&self) -> impl Iterator<Item = Symbol> + '_ {
        self.values.keys().copied()
    }
}

// Reads a project's override stub files from `<root>/stubs/*.R`, in sorted path order, returning their
// source text. A project drops declaration-only `#:` R there to override or extend the shipped
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
                .is_some_and(|extension| extension.eq_ignore_ascii_case("r"))
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .collect()
}

// Parses one declaration-only stub source and harvests each annotated top-level binding's `#:` annotation
// into a scheme keyed by the binding name (a later source overrides an earlier one for the same name). A
// binding without a value-type annotation is skipped; a source that fails to parse or whose annotation
// fails to lower is skipped rather than aborting, so a malformed project override degrades gracefully.
fn harvest_stub_source(
    parser: &mut Parser,
    interner: &mut Interner,
    source: &str,
    values: &mut BTreeMap<Symbol, StubValue>,
) {
    let Ok(document) = Document::parse(parser, source) else {
        return;
    };
    let module = lower_with_shared_interner(&document, interner).module;
    let mut inference_state = InferenceState::new();
    let type_definitions = TypeDefinitionEnvironment::default();
    for expression_id in &module.expressions {
        let expression = module.arena.get(*expression_id);
        let ExpressionKind::Assign { target, .. } = &expression.kind else {
            continue;
        };
        let Some(annotation) = &expression.annotation else {
            continue;
        };
        if !annotation.applies_to_binding() {
            continue;
        }
        let Annotation::Type { surface_type, .. } = annotation.annotation() else {
            continue;
        };
        let Ok(scheme) = inference_state.harvest_annotation_scheme(surface_type, &type_definitions)
        else {
            continue;
        };
        values.insert(
            *target,
            StubValue {
                scheme,
                range: expression.range,
            },
        );
    }
}
