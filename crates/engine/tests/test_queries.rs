// Granularity tests for the R query group (`DESIGN.md` §3). They drive the
// real `analysis` phase chain through the generic engine and assert *exact* body-execution counts on the
// group, the same instrumentation style as the engine smoke tests.
//
// The graph under test:
//
//   SourceText(f) -> Parse(f) -> Lower(f) -> LocalNaming(f) -> ExportedNames(f) --+
//   DocumentKind(f) ------------------------------------------------------------+ |
//   ProjectFiles ---------------------------------------> PackageSymbolIndex <----+
//                                                              |  (names only)
//                                                              v
//                                                       DefiningItem(symbol)  (firewall)
//                                                              v
//                                                       GlobalScheme(symbol)
//                                                              v
//   Config ------------------------------------------>   Typecheck(f) -> Diagnostics(f)
//
// Files: a.R defines `x`, b.R references `x`, c.R references nothing external.

use {
    engine::{
        Engine,
        queries::{Config, FileId, Key, RoughlyQueries, source_input},
    },
    std::path::PathBuf,
};

const A: FileId = 0; // defines `x`
const B: FileId = 1; // references `x`
const C: FileId = 2; // references nothing external

fn file_name(file: FileId) -> PathBuf {
    PathBuf::from(format!("/pkg/R/file_{file}.R"))
}

fn setup(sources: &[(FileId, &str)]) -> Engine<RoughlyQueries> {
    let mut engine = Engine::new(RoughlyQueries::new());
    let files: Vec<FileId> = sources.iter().map(|(file, _)| *file).collect();
    engine.set_input(Key::ProjectFiles, files);
    engine.set_input(Key::Config, Config::default());
    for (file, source) in sources {
        engine.set_input(Key::SourceText(*file), source_input(source));
        engine.set_input(
            Key::DocumentKind(*file),
            analysis::naming::DocumentKind::Package,
        );
        engine.set_input(Key::FileName(*file), file_name(*file));
    }
    engine
}

fn realize_all(engine: &Engine<RoughlyQueries>) {
    for file in [A, B, C] {
        let _ = engine.fetch::<engine::queries::FileDiagnostics>(Key::Diagnostics(file));
    }
}

// The whole `parse -> ... -> diagnostics` chain runs on real R input and each body runs exactly once.
#[test]
fn full_chain_runs_once_on_real_r() {
    let engine = setup(&[(A, "x <- function() 1"), (B, "z <- x()"), (C, "w <- 5")]);
    realize_all(&engine);

    let group = engine.group();
    assert_eq!(group.parse_runs(A), 1);
    assert_eq!(group.lower_runs(A), 1);
    assert_eq!(group.local_naming_runs(A), 1);
    assert_eq!(group.exported_names_runs(A), 1);
    assert_eq!(
        group.package_symbol_index_runs(),
        1,
        "the one all-files fold runs once"
    );
    assert_eq!(group.typecheck_runs(B), 1);
    assert_eq!(group.diagnostics_runs(B), 1);
    // b references x, so its typecheck pulled x's global scheme through the per-symbol firewall.
    let x = group.intern("x");
    assert_eq!(
        group.global_scheme_runs(x),
        1,
        "x's scheme computed once for its referrer"
    );
}

// ACCEPTANCE CONDITION 1 — the headline. Editing a function *body* (not its exported name) must trigger
// ZERO `PackageSymbolIndex` body folds, because `ExportedNames(a)` recomputes to an equal names-only value
// and cuts off. The referrer `b` re-typechecks ONLY because `x`'s `GlobalScheme` changed; an unrelated `c`
// does not re-typecheck at all.
#[test]
fn body_edit_does_not_refold_index_and_confines_to_referrers() {
    let engine = setup(&[(A, "x <- function() 1"), (B, "z <- x()"), (C, "w <- 5")]);
    realize_all(&engine);

    let x = engine.group().intern("x");
    let index_before = engine.group().package_symbol_index_runs();
    let exported_names_a_before = engine.group().exported_names_runs(A);
    let global_scheme_x_before = engine.group().global_scheme_runs(x);
    let typecheck_b_before = engine.group().typecheck_runs(B);
    let typecheck_c_before = engine.group().typecheck_runs(C);

    // Body-only edit: same exported name `x`, but its return type goes integer -> string.
    let mut engine = engine;
    engine.set_input(Key::SourceText(A), source_input("x <- function() \"two\""));
    realize_all(&engine);
    let group = engine.group();

    // The names-only cutoff: ExportedNames(a) recomputed (its inputs changed) ...
    assert_eq!(
        group.exported_names_runs(A),
        exported_names_a_before + 1,
        "ExportedNames(a) recomputes on a body edit"
    );
    // ... but produced an EQUAL value, so the lone all-files fold did NOT run. This is the headline win.
    assert_eq!(
        group.package_symbol_index_runs(),
        index_before,
        "body edit triggers ZERO PackageSymbolIndex folds (ExportedNames cut off)"
    );

    // x's scheme changed (integer -> string), so its one referrer re-typechecks ...
    assert_eq!(
        group.global_scheme_runs(x),
        global_scheme_x_before + 1,
        "x's GlobalScheme recomputes (its winning file's lower changed)"
    );
    assert_eq!(
        group.typecheck_runs(B),
        typecheck_b_before + 1,
        "the referrer b re-typechecks because x's scheme changed"
    );
    // ... and the unrelated file, which references no symbol whose scheme changed, does not.
    assert_eq!(
        group.typecheck_runs(C),
        typecheck_c_before,
        "c references nothing x-related, so it does not re-typecheck"
    );
}

// A body edit whose derived schemes are UNCHANGED must not re-typecheck the referrer either: the
// per-file `ExportedSchemes` recomputes (one whole-file inference), its value-eq cutoff stops
// propagation, and `GlobalScheme` does not even re-run. Editing the body to another integer keeps
// `x : () -> integer`.
#[test]
fn body_edit_with_unchanged_scheme_cuts_off_at_exported_schemes() {
    let engine = setup(&[(A, "x <- function() 1"), (B, "z <- x()"), (C, "w <- 5")]);
    realize_all(&engine);

    let x = engine.group().intern("x");
    let exported_schemes_a_before = engine.group().exported_schemes_runs(A);
    let global_scheme_x_before = engine.group().global_scheme_runs(x);
    let typecheck_b_before = engine.group().typecheck_runs(B);

    let mut engine = engine;
    engine.set_input(Key::SourceText(A), source_input("x <- function() 2")); // still () -> integer
    realize_all(&engine);
    let group = engine.group();

    assert_eq!(
        group.exported_schemes_runs(A),
        exported_schemes_a_before + 1,
        "the file's exports recompute once (lower changed)"
    );
    assert_eq!(
        group.global_scheme_runs(x),
        global_scheme_x_before,
        "the export set is value-unchanged, so GlobalScheme(x) validates without re-running"
    );
    assert_eq!(
        group.typecheck_runs(B),
        typecheck_b_before,
        "and the referrer does not re-typecheck"
    );
}

// STRUCTURAL EDIT — adding a top-level binding to a.R DOES re-fold `PackageSymbolIndex` once, but the
// `DefiningItem` firewall plus `GlobalScheme` value-eq confine propagation: `x`'s winner is unchanged and
// its scheme is unchanged, so the referrer `b` does not re-typecheck.
#[test]
fn structural_edit_refolds_index_once_but_firewall_confines_propagation() {
    let engine = setup(&[(A, "x <- function() 1"), (B, "z <- x()"), (C, "w <- 5")]);
    realize_all(&engine);

    let index_before = engine.group().package_symbol_index_runs();
    let typecheck_b_before = engine.group().typecheck_runs(B);
    let typecheck_c_before = engine.group().typecheck_runs(C);

    // Add a new top-level binding `y` to a.R: the exported-name set changes {x} -> {x, y}.
    let mut engine = engine;
    engine.set_input(
        Key::SourceText(A),
        source_input("x <- function() 1\ny <- 2"),
    );
    realize_all(&engine);
    let group = engine.group();

    // The names-only set changed, so the all-files fold runs exactly once more.
    assert_eq!(
        group.package_symbol_index_runs(),
        index_before + 1,
        "a structural export edit re-folds the index exactly once"
    );
    // The firewall: x's winner (a) and scheme (() -> integer) are both unchanged, so its referrer is not
    // disturbed. (GlobalScheme(x) does recompute via its lower(a) edge, but value-eq cuts it off.)
    assert_eq!(
        group.typecheck_runs(B),
        typecheck_b_before,
        "x's winner and scheme are unchanged -> referrer b does not re-typecheck"
    );
    assert_eq!(
        group.typecheck_runs(C),
        typecheck_c_before,
        "c is untouched by the structural edit"
    );

    // The new symbol y resolves to a winning file (a) through the same firewall.
    let y = group.intern("y");
    let defining_y = engine.fetch::<Option<FileId>>(Key::DefiningItem(y));
    assert_eq!(
        *defining_y,
        Some(A),
        "the newly added binding y wins in a.R"
    );
}

// PACKAGE-NAMING INCREMENTALITY. The per-file `PackageNamingDiagnostics` query is fine-grained
// at the same per-symbol grain as the type interface: a body edit elsewhere re-runs no unaffected file's
// package-naming, and the could-not-resolve firewall (`DefiningItem`) re-runs exactly the referrers when a
// referenced name's defined-ness flips. Diagnostics fetch `PackageNamingDiagnostics` regardless of config,
// so this holds under `Config::default()` (typing off).
#[test]
fn package_naming_diagnostics_are_incremental() {
    let mut engine = setup(&[
        (A, "x <- function() 1"), // defines x
        (B, "z <- x()"),          // references x
        (C, "w <- 5"),            // references nothing external
    ]);
    realize_all(&engine);

    let package_naming_b_before = engine.group().package_naming_diagnostics_runs(B);
    let package_naming_c_before = engine.group().package_naming_diagnostics_runs(C);

    // Body-only edit to a.R: same exported name `x`, body literal changes. b and c reference nothing
    // whose resolution moved, so neither re-runs its package-naming diagnostics.
    engine.set_input(Key::SourceText(A), source_input("x <- function() 2"));
    realize_all(&engine);
    assert_eq!(
        engine.group().package_naming_diagnostics_runs(B),
        package_naming_b_before,
        "a body edit to a.R does not re-run b's package-naming diagnostics"
    );
    assert_eq!(
        engine.group().package_naming_diagnostics_runs(C),
        package_naming_c_before,
        "a body edit to a.R does not re-run c's package-naming diagnostics"
    );

    // Delete a.R: `x`'s defined-ness flips Some -> None. Only its referrer b re-evaluates (its
    // could-not-resolve diagnostic now appears); the unrelated c does not.
    let package_naming_b_pre_delete = engine.group().package_naming_diagnostics_runs(B);
    let package_naming_c_pre_delete = engine.group().package_naming_diagnostics_runs(C);
    engine.remove_input(&Key::SourceText(A));
    engine.remove_input(&Key::DocumentKind(A));
    engine.remove_input(&Key::FileName(A));
    engine.set_input(Key::ProjectFiles, vec![B, C]);
    let _ = engine.fetch::<engine::queries::FileDiagnostics>(Key::Diagnostics(B));
    let _ = engine.fetch::<engine::queries::FileDiagnostics>(Key::Diagnostics(C));

    assert_eq!(
        engine.group().package_naming_diagnostics_runs(B),
        package_naming_b_pre_delete + 1,
        "deleting x's definer re-runs b's package-naming (its could-not-resolve diagnostic appears)"
    );
    assert_eq!(
        engine.group().package_naming_diagnostics_runs(C),
        package_naming_c_pre_delete,
        "c references nothing x-related, so the defined-ness flip does not re-run its package-naming"
    );

    // The firewall re-run produced the expected diagnostic: b can no longer resolve `x`.
    let diagnostics_b = engine.fetch::<engine::queries::FileDiagnostics>(Key::Diagnostics(B));
    assert!(
        diagnostics_b
            .package_naming
            .iter()
            .any(|diagnostic| diagnostic.message.contains("I could not resolve `x`")),
        "b reports the could-not-resolve diagnostic for the now-undefined x: {:?}",
        diagnostics_b.package_naming
    );
}

// TOMBSTONE HARDENING (defense-in-depth). The `validate` short-circuit normally recomputes a file-set fold
// before any stale per-file edge is walked, so a removed `SourceText` tombstone is never reached on the live
// path. This test deliberately removes the source *without* updating any fold, leaving the recorded
// `Lower(f) -> Parse(f) -> SourceText(f)` chain stale, then fetches `Lower(f)` directly. Validating the stale
// chain walks `Parse(f)` into the `SourceText(f)` tombstone; because `Parse`/`Lower` read through
// `fetch_optional`, the chain degrades to an empty module (a cutoff) instead of the input-resurrection panic
// a naive `fetch` would raise. A future fetch reorder that re-exposed this walk thus degrades, not crashes.
#[test]
fn fetching_through_a_tombstoned_source_yields_empty_without_panic() {
    let mut engine = Engine::new(RoughlyQueries::new());
    engine.set_input(Key::SourceText(A), source_input("x <- 1L"));
    // Record the `Lower(A) -> Parse(A) -> SourceText(A)` chain.
    let module = engine.fetch::<analysis::hir::Module>(Key::Lower(A));
    assert!(!module.expressions.is_empty());

    // Remove the source, leaving the recorded chain stale (no fold recompute dropped the per-file edges).
    engine.remove_input(&Key::SourceText(A));

    // Validating the stale chain walks `Parse(A)` into the `SourceText(A)` tombstone; the optional fetch
    // degrades it to an empty parse instead of panicking, so `Lower(A)` recomputes to an empty module.
    let module = engine.fetch::<analysis::hir::Module>(Key::Lower(A));
    assert!(module.expressions.is_empty());
    assert!(module.definitions.is_empty());
}

// A file of backward-chained top-level functions — each referencing only symbols defined above
// it — must not route through the interface-SCC machinery at all: the module walk rebinds each
// winner's global entry at its definition site, before any read, so no interface import (and no
// interface-dependency edge) exists for those references. Demanding the file's diagnostics then
// costs exactly one authoritative inference; the whole-file exported-schemes inference runs only
// if a referrer demands a scheme. Before this held, every same-file reference chain formed a
// fake SCC whose fixed point re-inferred the whole file per member and per round — the shape
// that made large real-world files' typecheck quadratic and worse.
#[test]
fn backward_reference_chain_stays_off_the_interface_scc() {
    let chain: String = (1..40).fold("h0 <- function(x) x + 1\n".to_owned(), |mut source, i| {
        source += &format!("h{i} <- function(x) h{}(x) + {i}\n", i - 1);
        source
    });
    let referrer = "total <- h39(1L)\n";
    let engine = setup(&[(A, chain.as_str()), (B, referrer)]);
    let _ = engine.fetch::<engine::queries::FileDiagnostics>(Key::Diagnostics(A));
    let _ = engine.fetch::<engine::queries::FileDiagnostics>(Key::Diagnostics(B));

    let totals: std::collections::HashMap<&str, u64> =
        engine.group().execution_totals().into_iter().collect();
    assert_eq!(
        totals["interface scc (fixed point)"], 0,
        "a backward chain is not a cycle; no fixed point may run"
    );
    assert_eq!(
        totals["exported schemes (inference)"], 1,
        "the referrer's scheme demand infers the chain file exactly once"
    );
    assert_eq!(
        totals["typecheck (inference)"], 2,
        "one authoritative inference per file"
    );
}
