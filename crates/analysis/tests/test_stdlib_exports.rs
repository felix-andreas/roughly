//! Exhaustiveness / name-list check for the shipped standard-library stub corpus.
//!
//! Deterministic and R-free: it diffs the names each `stubs/*.Rtypes` file declares (via the real stub
//! parser) against a checked-in curated snapshot of that namespace's real R exports
//! (`tests/stdlib_exports/<namespace>.txt`). The snapshots are HAND-MAINTAINED, not derived from a live R;
//! signature-level validation (arity/argument names/types) needs a real R installation and stays a future
//! slice (`docs/.../stdlib-stubs.md` §7-9). This is names only.
//!
//! Policy:
//! - The stub corpus must be a **subset** of the real exports: every stubbed name must appear in its
//!   namespace's snapshot. A stubbed name that is not a real export (a typo, or a name from the wrong
//!   package) is a **hard failure** — the corpus would resolve a name R does not actually export.
//! - **Unstubbed** real exports are allowed: the snapshot is intentionally larger than the corpus (we are
//!   not exhaustive). They are counted and printed as a coverage gauge, never a failure.

use std::collections::BTreeSet;

const EXPECTED_EXPORTS: &[(&str, &str)] = &[
    ("base", include_str!("stdlib_exports/base.txt")),
    ("stats", include_str!("stdlib_exports/stats.txt")),
    ("utils", include_str!("stdlib_exports/utils.txt")),
    ("methods", include_str!("stdlib_exports/methods.txt")),
];

fn expected_names(source: &str) -> BTreeSet<String> {
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

// Every stubbed name is a real export of its namespace (hard assert), and every namespace has a matching
// curated snapshot; unstubbed real exports are reported, not failed.
#[test]
fn stub_names_are_a_subset_of_curated_real_exports() {
    let shipped = analysis::stdlib::shipped_stub_names_by_namespace();
    assert_eq!(
        shipped.len(),
        EXPECTED_EXPORTS.len(),
        "every shipped namespace needs a curated export snapshot in tests/stdlib_exports/"
    );

    for (namespace, stubbed_names) in &shipped {
        let source = EXPECTED_EXPORTS
            .iter()
            .find(|(name, _)| name == namespace)
            .unwrap_or_else(|| panic!("no curated export snapshot for namespace `{namespace}`"))
            .1;
        let expected = expected_names(source);
        let stubbed = stubbed_names.iter().cloned().collect::<BTreeSet<_>>();

        // (a) No stubbed name that is not a real export.
        let not_exported = stubbed.difference(&expected).cloned().collect::<Vec<_>>();
        assert!(
            not_exported.is_empty(),
            "these `{namespace}` stub names are not in the curated real-export snapshot \
             (typo, or wrong package?): {not_exported:?}"
        );

        // (b) Coverage gauge: unstubbed real exports are allowed and only reported.
        let unstubbed = expected.difference(&stubbed).count();
        println!(
            "stdlib exhaustiveness [{namespace}]: {} stubbed / {} curated real exports \
             ({} unstubbed not yet covered)",
            stubbed.len(),
            expected.len(),
            unstubbed,
        );
    }
}
