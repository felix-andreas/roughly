use {
    crate::{
        document::Document,
        hir::ExpressionKind,
        interner::Symbol,
        lower::lower_with_shared_interner,
        tree,
        typecheck::{InferenceState, TypeDefinitionEnvironment},
        types::{Annotation, TypeScheme},
        Interner,
    },
    std::collections::BTreeMap,
    tree_sitter::Range,
};

// The embedded stdlib stub corpus. It is ordinary, parseable R whose bindings carry `#:` annotations;
// the loader harvests each annotation into a `TypeScheme` and never infers the placeholder bodies.
const EMBEDDED_BASE_STUB: &str = include_str!("stdlib_base.R");

// An immutable, high-durability description of the standard library's value bindings: each base value
// or function name mapped to the `TypeScheme` harvested from its `#:` annotation. Built once at
// `Analysis::new` and never invalidated by user edits.
//
// HARD ISOLATION (LT2): a `StubLibrary` is a *base-environment* input only. Its symbols are seeded
// into the per-document inference template (so a bare base name resolves to its stub scheme), but they
// must never appear in `global_bindings`, the package interface table, or the materialized type index —
// a user binding over a base name is a shadow, not a package definition, and a stub is never a package
// global. The seeding happens only in `typecheck`'s template, structurally outside package naming.
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
    // An empty library, used as the zero-cost baseline in the isolation benchmark.
    pub fn empty() -> Self {
        Self::default()
    }

    // Parses the embedded corpus once, lowering it through the ordinary pipeline against the shared
    // interner so every stub name is interned exactly as user references to it would be, then
    // harvests each binding annotation into a scheme without inferring the placeholder bodies.
    pub fn load(interner: &mut Interner) -> Self {
        let mut parser = tree::new_parser().expect("stub parser should initialize");
        let document =
            Document::parse(&mut parser, EMBEDDED_BASE_STUB).expect("embedded base stub should parse");
        let module = lower_with_shared_interner(&document, interner).module;

        let mut inference_state = InferenceState::new();
        let type_definitions = TypeDefinitionEnvironment::default();
        let mut values = BTreeMap::new();
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
            let scheme = inference_state
                .harvest_annotation_scheme(surface_type, &type_definitions)
                .expect("embedded base stub annotation should lower to a scheme");
            values.insert(
                *target,
                StubValue {
                    scheme,
                    range: expression.range,
                },
            );
        }

        debug_assert!(
            !values.is_empty(),
            "embedded base stub yielded no schemes; the corpus failed to parse or lower"
        );
        Self { values }
    }

    pub fn contains(&self, symbol: Symbol) -> bool {
        self.values.contains_key(&symbol)
    }

    // Binds every stub scheme as a base global into `inference_state`. Called when building the
    // per-document inference template, so a document's check resolves a bare base name to its stub
    // scheme. The schemes live only in the template environment, never in `global_bindings`.
    pub fn seed_into(&self, inference_state: &mut InferenceState) {
        for (symbol, value) in &self.values {
            inference_state.bind_global_scheme(*symbol, value.scheme.clone(), value.range);
        }
    }

    pub fn symbols(&self) -> impl Iterator<Item = Symbol> + '_ {
        self.values.keys().copied()
    }
}
