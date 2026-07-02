//! Exhaustiveness / name-list checks for the shipped standard-library stub corpus.
//!
//! Deterministic and R-free: it diffs the names each `stubs/*.Rtypes` file declares (via the real stub
//! parser) against a checked-in snapshot of that namespace's real R exports
//! (`tests/stdlib_exports/<namespace>.txt`). The snapshots are best-effort static lists written from R
//! knowledge, aiming to be genuinely close to `getNamespaceExports(<namespace>)` — including operators
//! and names no stub will ever cover — so the printed coverage is an honest gauge, not a
//! self-referential one; regenerating them from a live R (the stubtest slice) replaces them.
//! Signature-level validation (arity/argument names/types) needs a real R installation and stays a
//! future slice (`docs/.../stdlib-stubs.md` §7-9). This is names only.
//!
//! Policy:
//! - The stub corpus must be a **subset** of the real exports: every stubbed name must appear in its
//!   namespace's snapshot. A stubbed name that is not a real export (a typo, or a name from the wrong
//!   package) is a **hard failure** — the corpus would resolve a name R does not actually export.
//! - **Unstubbed** real exports are allowed: the snapshot is intentionally much larger than the corpus.
//!   Coverage percentages are printed as a gauge, never asserted.
//! - The stub loader currently drops malformed declaration lines and failed harvests silently
//!   (`stdlib.rs`), so this suite additionally asserts the shipped sources parse and harvest cleanly —
//!   otherwise a typo'd stub line would vanish without any test noticing.

use {
    analysis::{
        Interner,
        stub::parse_stub_declarations,
        typecheck::{InferenceState, TypeDefinitionEnvironment},
    },
    std::collections::BTreeSet,
};

// Every shipped (or shipping-pending) stub file paired with its namespace snapshot. `graphics` and
// `grDevices` stub files exist but are not yet wired into `SHIPPED_STUBS` (`stdlib.rs` is owned by the
// loader); the checks here run from the files directly so the corpus is validated either way, and the
// wiring consistency test below tolerates — but reports — the not-yet-wired state.
const STUB_CORPUS: &[(&str, &str, &str)] = &[
    (
        "base",
        include_str!("../stubs/base.Rtypes"),
        include_str!("stdlib_exports/base.txt"),
    ),
    (
        "stats",
        include_str!("../stubs/stats.Rtypes"),
        include_str!("stdlib_exports/stats.txt"),
    ),
    (
        "utils",
        include_str!("../stubs/utils.Rtypes"),
        include_str!("stdlib_exports/utils.txt"),
    ),
    (
        "methods",
        include_str!("../stubs/methods.Rtypes"),
        include_str!("stdlib_exports/methods.txt"),
    ),
    (
        "graphics",
        include_str!("../stubs/graphics.Rtypes"),
        include_str!("stdlib_exports/graphics.txt"),
    ),
    (
        "grDevices",
        include_str!("../stubs/grDevices.Rtypes"),
        include_str!("stdlib_exports/grDevices.txt"),
    ),
];

fn snapshot_names(source: &str) -> BTreeSet<String> {
    source
        .lines()
        .map(|line| match line.find('#') {
            Some(index) => &line[..index],
            None => line,
        })
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

fn declared_names(source: &str, namespace: &str) -> BTreeSet<String> {
    let mut interner = Interner::default();
    let (declarations, errors) = parse_stub_declarations(source, &mut interner);
    assert!(
        errors.is_empty(),
        "`{namespace}.Rtypes` has malformed declaration lines the loader would silently drop: {errors:?}"
    );
    declarations
        .iter()
        .filter_map(|declaration| interner.resolve(declaration.name))
        .map(str::to_owned)
        .collect()
}

// Every stubbed name is a real export of its namespace (hard assert); unstubbed real exports are only
// counted. Coverage percentages are printed (`--nocapture`), never asserted.
#[test]
fn stub_names_are_a_subset_of_real_export_snapshots() {
    for &(namespace, stub_source, snapshot_source) in STUB_CORPUS {
        let stubbed = declared_names(stub_source, namespace);
        let expected = snapshot_names(snapshot_source);
        assert!(
            !stubbed.is_empty(),
            "`{namespace}.Rtypes` declares no names at all"
        );

        let not_exported = stubbed.difference(&expected).cloned().collect::<Vec<_>>();
        assert!(
            not_exported.is_empty(),
            "these `{namespace}` stub names are not in the real-export snapshot \
             (typo, or wrong package?): {not_exported:?}"
        );

        let percent = 100.0 * stubbed.len() as f64 / expected.len() as f64;
        println!(
            "stdlib coverage [{namespace}]: {} stubbed / {} snapshot exports ({percent:.1}%)",
            stubbed.len(),
            expected.len(),
        );
    }
}

// Every declaration in every shipped stub file must harvest into a scheme: the loader skips failed
// harvests silently, so a declaration that parses but cannot lower (e.g. referencing a type name the
// empty definition environment cannot resolve) would otherwise vanish from the corpus unnoticed.
#[test]
fn stub_declarations_harvest_cleanly() {
    for &(namespace, stub_source, _) in STUB_CORPUS {
        let mut interner = Interner::default();
        let (declarations, _errors) = parse_stub_declarations(stub_source, &mut interner);
        let mut inference_state = InferenceState::new();
        let type_definitions = TypeDefinitionEnvironment::default();
        for declaration in &declarations {
            let name = interner.resolve(declaration.name).unwrap_or("<unknown>");
            assert!(
                inference_state
                    .harvest_annotation_scheme(&declaration.surface_type, &type_definitions)
                    .is_ok(),
                "`{namespace}.Rtypes` declaration `{name}` fails to harvest into a scheme; \
                 the loader would silently drop it"
            );
        }
    }
}

// The namespaces the loader actually ships must line up with the corpus files: each wired namespace
// needs a snapshot, and its loader-visible names must equal the file's declarations (drift between
// `SHIPPED_STUBS` and `stubs/` would otherwise go unseen). Corpus files not yet wired are reported —
// `graphics`/`grDevices` await their two-line `SHIPPED_STUBS` entries.
#[test]
fn wired_namespaces_match_corpus_files() {
    let shipped = analysis::stdlib::shipped_stub_names_by_namespace();
    for (namespace, shipped_names) in &shipped {
        let (_, stub_source, _) = STUB_CORPUS
            .iter()
            .find(|(name, _, _)| name == namespace)
            .unwrap_or_else(|| panic!("wired namespace `{namespace}` has no corpus entry here"));
        let from_file = declared_names(stub_source, namespace);
        let from_loader = shipped_names.iter().cloned().collect::<BTreeSet<_>>();
        assert_eq!(
            from_loader, from_file,
            "loader-visible `{namespace}` names differ from its `.Rtypes` file"
        );
    }

    for &(namespace, _, _) in STUB_CORPUS {
        if !shipped.iter().any(|(name, _)| *name == namespace) {
            println!(
                "stdlib wiring [{namespace}]: corpus file exists but is NOT wired into SHIPPED_STUBS yet"
            );
        }
    }
}
