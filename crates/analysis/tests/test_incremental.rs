mod common;

use {
    analysis::{Analysis, CheckConfig, DocumentChange, LintConfig, TextPosition, TextRange},
    std::path::PathBuf,
};

fn typing_analysis(base: &str) -> Analysis {
    Analysis::new(
        PathBuf::from(base),
        LintConfig::default(),
        CheckConfig {
            unused: false,
            typing: true,
        },
    )
}

fn error_messages(analysis_state: &Analysis, path: &str) -> Vec<String> {
    let document_id = analysis_state
        .document_id_for_path(std::path::Path::new(path))
        .expect("document should exist");
    analysis_state
        .document_diagnostics(document_id)
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect()
}

fn replace_document(analysis_state: &mut Analysis, path: &str, source: &str) {
    analysis_state
        .add_document_from_source(PathBuf::from(path), source)
        .expect("document should parse");
}

#[test]
fn absolute_base_path_reports_type_errors() {
    let mut analysis_state = typing_analysis("/pkg");
    replace_document(
        &mut analysis_state,
        "/pkg/R/main.R",
        "#: fn(count: integer) -> integer\ndouble_count <- function(count) count + count\nbad <- double_count(\"two\")\n",
    );
    analysis::run_full(&mut analysis_state);

    let messages = error_messages(&analysis_state, "/pkg/R/main.R");
    assert_eq!(messages, vec!["expected `integer`, found `character`"]);
}

#[test]
fn every_error_in_a_document_is_reported() {
    let mut analysis_state = typing_analysis("/pkg");
    replace_document(
        &mut analysis_state,
        "/pkg/R/main.R",
        "#: integer\nfirst <- \"one\"\n#: integer\nsecond <- \"two\"\n",
    );
    analysis::run_full(&mut analysis_state);

    let messages = error_messages(&analysis_state, "/pkg/R/main.R");
    assert_eq!(messages.len(), 2, "{messages:?}");
}

#[test]
fn errors_in_several_documents_are_reported_in_one_run() {
    let mut analysis_state = typing_analysis("/pkg");
    replace_document(&mut analysis_state, "/pkg/R/a.R", "#: integer\na <- \"one\"\n");
    replace_document(&mut analysis_state, "/pkg/R/b.R", "#: integer\nb <- \"two\"\n");
    analysis::run_full(&mut analysis_state);

    assert_eq!(error_messages(&analysis_state, "/pkg/R/a.R").len(), 1);
    assert_eq!(error_messages(&analysis_state, "/pkg/R/b.R").len(), 1);
}

#[test]
fn scripts_are_typechecked_against_package_globals() {
    let mut analysis_state = typing_analysis("/pkg");
    replace_document(
        &mut analysis_state,
        "/pkg/R/a.R",
        "#: fn(count: integer) -> integer\ndouble_count <- function(count) count + count\n",
    );
    replace_document(
        &mut analysis_state,
        "/pkg/scripts/use.R",
        "good <- double_count(2L)\nbad <- double_count(\"two\")\n",
    );
    analysis::run_full(&mut analysis_state);

    assert_eq!(error_messages(&analysis_state, "/pkg/R/a.R").len(), 0);
    assert_eq!(
        error_messages(&analysis_state, "/pkg/scripts/use.R"),
        vec!["expected `integer`, found `character`"]
    );
}

#[test]
fn body_edit_without_interface_change_rechecks_only_edited_document() {
    let mut analysis_state = typing_analysis("/pkg");
    replace_document(
        &mut analysis_state,
        "/pkg/R/a.R",
        "#: fn(count: integer) -> integer\ndouble_count <- function(count) count + count\n",
    );
    replace_document(
        &mut analysis_state,
        "/pkg/R/b.R",
        "result <- double_count(2L)\n",
    );
    let first_run = analysis::typecheck(&mut analysis_state);
    assert_eq!(first_run.len(), 2, "initial run checks both documents");

    // Body change that keeps the exported interface identical.
    replace_document(
        &mut analysis_state,
        "/pkg/R/a.R",
        "#: fn(count: integer) -> integer\ndouble_count <- function(count) { count + count }\n",
    );
    let second_run = analysis::typecheck(&mut analysis_state);
    let a_id = analysis_state
        .document_id_for_path(std::path::Path::new("/pkg/R/a.R"))
        .expect("document should exist");
    assert_eq!(second_run, vec![a_id], "only the edited document rechecks");
}

#[test]
fn interface_change_rechecks_dependents_and_reports_dependent_errors() {
    let mut analysis_state = typing_analysis("/pkg");
    replace_document(
        &mut analysis_state,
        "/pkg/R/a.R",
        "#: fn(count: integer) -> integer\ndouble_count <- function(count) count + count\n",
    );
    replace_document(
        &mut analysis_state,
        "/pkg/R/b.R",
        "result <- double_count(2L)\n",
    );
    analysis::run_full(&mut analysis_state);
    assert_eq!(error_messages(&analysis_state, "/pkg/R/b.R").len(), 0);

    // The exported interface changes; the dependent document must be rechecked and now fail.
    replace_document(
        &mut analysis_state,
        "/pkg/R/a.R",
        "#: fn(count: character) -> integer\ndouble_count <- function(count) 1L\n",
    );
    let second_run = analysis::typecheck(&mut analysis_state);
    let b_id = analysis_state
        .document_id_for_path(std::path::Path::new("/pkg/R/b.R"))
        .expect("document should exist");
    assert!(
        second_run.contains(&b_id),
        "dependent document should recheck after an interface change"
    );
    assert_eq!(
        error_messages(&analysis_state, "/pkg/R/b.R"),
        vec!["expected `character`, found `integer`"]
    );
}

#[test]
fn interface_change_rechecks_exactly_referrers_not_independent() {
    // A defines global `g`; B references `g`; C references no global. Changing A's exported
    // interface must recheck exactly A and its single referrer B (k + 1 = 2), and must leave the
    // independent document C on its cache.
    let mut analysis_state = typing_analysis("/pkg");
    replace_document(
        &mut analysis_state,
        "/pkg/R/a.R",
        "#: fn(count: integer) -> integer\ng <- function(count) count + count\n",
    );
    replace_document(&mut analysis_state, "/pkg/R/b.R", "result <- g(2L)\n");
    replace_document(&mut analysis_state, "/pkg/R/c.R", "c_value <- 1L\n");
    let first_run = analysis::typecheck(&mut analysis_state);
    assert_eq!(first_run.len(), 3, "initial run checks every document");

    // Change `g`'s exported scheme.
    replace_document(
        &mut analysis_state,
        "/pkg/R/a.R",
        "#: fn(count: character) -> integer\ng <- function(count) 1L\n",
    );
    let second_run = analysis::typecheck(&mut analysis_state);
    let a_id = analysis_state
        .document_id_for_path(std::path::Path::new("/pkg/R/a.R"))
        .expect("document should exist");
    let b_id = analysis_state
        .document_id_for_path(std::path::Path::new("/pkg/R/b.R"))
        .expect("document should exist");
    assert_eq!(
        second_run,
        vec![a_id, b_id],
        "exactly the changed document and its referrer recheck"
    );
}

#[test]
fn interface_change_leaves_document_referencing_other_global_cached() {
    // A defines `g`; B references `g`; D defines `h`; C references only `h`. Changing A's interface
    // must not recheck C, whose only dependency (`h`) is unchanged.
    let mut analysis_state = typing_analysis("/pkg");
    replace_document(
        &mut analysis_state,
        "/pkg/R/a.R",
        "#: fn(count: integer) -> integer\ng <- function(count) count + count\n",
    );
    replace_document(&mut analysis_state, "/pkg/R/b.R", "result <- g(2L)\n");
    replace_document(
        &mut analysis_state,
        "/pkg/R/d.R",
        "#: fn(value: integer) -> integer\nh <- function(value) value\n",
    );
    replace_document(&mut analysis_state, "/pkg/R/c.R", "c_value <- h(3L)\n");
    let first_run = analysis::typecheck(&mut analysis_state);
    assert_eq!(first_run.len(), 4, "initial run checks every document");

    replace_document(
        &mut analysis_state,
        "/pkg/R/a.R",
        "#: fn(count: character) -> integer\ng <- function(count) 1L\n",
    );
    let second_run = analysis::typecheck(&mut analysis_state);
    let c_id = analysis_state
        .document_id_for_path(std::path::Path::new("/pkg/R/c.R"))
        .expect("document should exist");
    assert!(
        !second_run.contains(&c_id),
        "document referencing an unchanged global stays cached: {second_run:?}"
    );
}

#[test]
fn body_only_edit_examines_constant_number_of_candidates() {
    // Slice-3 perf-correctness proof: a body-only edit in a large package must bound the package
    // scan to O(1) candidates, not O(documents). Without the dirty-set + reverse-dependency routing
    // the round-2 scan would examine every document.
    let file_count = 25;
    let items_per_file = 3;
    let mut analysis_state = typing_analysis("/pkg");
    for (path, source) in common::generate_package(file_count, items_per_file) {
        analysis_state
            .add_document_from_source(path, &source)
            .expect("document should parse");
    }
    analysis::run_full(&mut analysis_state);

    // Body-only edit (braced bodies) to a single file; its exported interface is unchanged.
    analysis_state
        .add_document_from_source(
            common::file_path(12),
            &common::generate_file(12, items_per_file, true),
        )
        .expect("document should parse");
    let recomputed = analysis::typecheck(&mut analysis_state);

    assert_eq!(recomputed.len(), 1, "body edit rechecks exactly one document");
    let candidate_count = analysis_state.last_candidate_count();
    assert!(
        candidate_count <= 2,
        "a body-only edit must examine O(1) documents, examined {candidate_count} of {file_count}"
    );
}

#[test]
fn interface_change_propagates_through_a_re_export() {
    // A defines `first`; B re-exports it as `second <- first`; C references `second`. Changing
    // `first`'s scheme must propagate transitively through the round-1 worklist to A, B, and C, and
    // must leave the unrelated document D cached.
    let mut analysis_state = typing_analysis("/pkg");
    replace_document(
        &mut analysis_state,
        "/pkg/R/a.R",
        "#: fn(count: integer) -> integer\nfirst <- function(count) count + count\n",
    );
    replace_document(&mut analysis_state, "/pkg/R/b.R", "second <- first\n");
    replace_document(&mut analysis_state, "/pkg/R/c.R", "result <- second(2L)\n");
    replace_document(&mut analysis_state, "/pkg/R/d.R", "d_value <- 1L\n");
    let first_run = analysis::typecheck(&mut analysis_state);
    assert_eq!(first_run.len(), 4, "initial run checks every document");

    replace_document(
        &mut analysis_state,
        "/pkg/R/a.R",
        "#: fn(count: character) -> integer\nfirst <- function(count) 1L\n",
    );
    let second_run = analysis::typecheck(&mut analysis_state);
    let a_id = analysis_state
        .document_id_for_path(std::path::Path::new("/pkg/R/a.R"))
        .expect("document should exist");
    let b_id = analysis_state
        .document_id_for_path(std::path::Path::new("/pkg/R/b.R"))
        .expect("document should exist");
    let c_id = analysis_state
        .document_id_for_path(std::path::Path::new("/pkg/R/c.R"))
        .expect("document should exist");
    let d_id = analysis_state
        .document_id_for_path(std::path::Path::new("/pkg/R/d.R"))
        .expect("document should exist");
    assert!(
        second_run.contains(&a_id) && second_run.contains(&b_id) && second_run.contains(&c_id),
        "the re-export chain A -> B -> C must all recheck: {second_run:?}"
    );
    assert!(
        !second_run.contains(&d_id),
        "the unrelated document must stay cached: {second_run:?}"
    );
}

#[test]
fn defining_a_forward_referenced_global_rechecks_the_referrer() {
    // B references `late` before any document defines it (a forward reference, which still registers
    // a reverse-dependency edge). Adding a document that defines `late` changes the global's winner,
    // so B must be picked as a candidate and rechecked.
    let mut analysis_state = typing_analysis("/pkg");
    replace_document(&mut analysis_state, "/pkg/R/b.R", "result <- late(3L)\n");
    analysis::run_full(&mut analysis_state);

    replace_document(
        &mut analysis_state,
        "/pkg/R/late.R",
        "#: fn(value: integer) -> integer\nlate <- function(value) value\n",
    );
    let second_run = analysis::typecheck(&mut analysis_state);
    let b_id = analysis_state
        .document_id_for_path(std::path::Path::new("/pkg/R/b.R"))
        .expect("document should exist");
    assert!(
        second_run.contains(&b_id),
        "the forward referrer rechecks once its global is defined: {second_run:?}"
    );
}

#[test]
fn winner_flip_with_intervening_resolve_package_still_rechecks_referrer() {
    // Reproduces the slice-3 baseline-staleness blocker. b.R references `helper`, whose winning
    // definition is a2.R (integer). Dropping `helper` from a2.R flips the winner to a1.R (character),
    // which must make b.R error. An intervening `resolve_package` (as completion / references / rename /
    // typing-off `run_full` would trigger) refreshes the live package naming without clearing the dirty
    // set; the winner-diff must compare against the bindings frozen at the last completed typecheck, not
    // the live ones, or b.R is never selected as a candidate and keeps a stale (empty) result.
    let mut analysis_state = typing_analysis("/pkg");
    replace_document(
        &mut analysis_state,
        "/pkg/R/a1.R",
        "#: fn() -> character\nhelper <- function() \"x\"\n",
    );
    replace_document(
        &mut analysis_state,
        "/pkg/R/a2.R",
        "#: fn() -> integer\nhelper <- function() 1L\n",
    );
    replace_document(&mut analysis_state, "/pkg/R/b.R", "result <- helper() + 1L\n");
    analysis::typecheck(&mut analysis_state);
    assert_eq!(
        error_messages(&analysis_state, "/pkg/R/b.R").len(),
        0,
        "b is initially clean because the integer winner a2 is in scope"
    );

    // Drop `helper` from a2.R so the winner flips to a1.R (character).
    replace_document(&mut analysis_state, "/pkg/R/a2.R", "unrelated <- 1L\n");
    // Intervening resolve_package-driving operation BEFORE the authoritative typecheck. This refreshes
    // the live package naming (as completion / references / rename would) while the dirty set persists.
    analysis::resolve_package(&mut analysis_state);

    let recomputed = analysis::typecheck(&mut analysis_state);
    let b_id = analysis_state
        .document_id_for_path(std::path::Path::new("/pkg/R/b.R"))
        .expect("document should exist");
    assert!(
        recomputed.contains(&b_id),
        "the referrer must recompute after the winner flip despite the intervening resolve_package: {recomputed:?}"
    );
    assert!(
        !error_messages(&analysis_state, "/pkg/R/b.R").is_empty(),
        "b must report the now-character helper as a type error, not keep its stale empty result"
    );
}

#[test]
fn unedited_documents_keep_diagnostics_after_unrelated_edit() {
    let mut analysis_state = typing_analysis("/pkg");
    replace_document(&mut analysis_state, "/pkg/R/a.R", "#: integer\na <- \"bad\"\n");
    replace_document(&mut analysis_state, "/pkg/R/b.R", "b <- 1L\n");
    analysis::run_full(&mut analysis_state);
    assert_eq!(error_messages(&analysis_state, "/pkg/R/a.R").len(), 1);

    analysis_state
        .edit_document(
            std::path::Path::new("/pkg/R/b.R"),
            &[DocumentChange {
                range: TextRange {
                    start: TextPosition {
                        line_index: 0,
                        character_index: 5,
                    },
                    end: TextPosition {
                        line_index: 0,
                        character_index: 7,
                    },
                },
                text: "2L".to_owned(),
            }],
        )
        .expect("edit should apply");
    analysis::run_full(&mut analysis_state);
    assert_eq!(
        error_messages(&analysis_state, "/pkg/R/a.R").len(),
        1,
        "untouched document keeps its diagnostics"
    );
}

#[test]
fn deep_re_export_chain_past_round_cap_is_not_left_stale() {
    // A re-export chain far longer than the old fixed 32-round interface cap: f0 defines `g`, then each
    // link `v{i} <- v{i-1}` re-exports it, and the tail calls the deepest link with `+ 1L`. The
    // interface fixed-point advances one hop per round, so under a fixed cap this chain never converged
    // and the deep links (and tail) were silently left unresolved — flipping `g`'s return type would not
    // reach the tail. With the round bound scaled to the package document count the chain converges, so
    // the tail recomputes against the converged interface and the now-real `character` type error
    // surfaces instead of a stale clean result.
    const CHAIN: usize = 40;
    let mut analysis_state = typing_analysis("/pkg");
    replace_document(
        &mut analysis_state,
        "/pkg/R/f0.R",
        "#: fn() -> integer\ng <- function() 1L\n",
    );
    for index in 1..=CHAIN {
        let previous = if index == 1 {
            "g".to_string()
        } else {
            format!("v{}", index - 1)
        };
        replace_document(
            &mut analysis_state,
            &format!("/pkg/R/f{index}.R"),
            &format!("v{index} <- {previous}\n"),
        );
    }
    let tail_path = "/pkg/R/tail.R";
    replace_document(
        &mut analysis_state,
        tail_path,
        &format!("result <- v{CHAIN}() + 1L\n"),
    );

    analysis::run_full(&mut analysis_state);
    assert_eq!(
        error_messages(&analysis_state, tail_path).len(),
        0,
        "tail is clean while the chain returns integer"
    );

    replace_document(
        &mut analysis_state,
        "/pkg/R/f0.R",
        "#: fn() -> character\ng <- function() \"x\"\n",
    );
    let recomputed = analysis::typecheck(&mut analysis_state);
    let tail_id = analysis_state
        .document_id_for_path(std::path::Path::new(tail_path))
        .expect("document should exist");
    assert!(
        recomputed.contains(&tail_id),
        "deep tail past the round cap must be a recompute candidate via the all-docs fallback: \
         {recomputed:?}"
    );
    assert_eq!(
        error_messages(&analysis_state, tail_path),
        vec!["expected a numeric value (`integer` or `double`), found `character`"],
        "the tail must report the now-character error, not a stale clean result"
    );
}

// Fixed-seed soak test for the incremental package-naming and type-index maintenance. Drives the
// public `Analysis` API through a long, deterministic stream of add / edit / delete operations over a
// small pool of paths and names, so the same names overwrite, re-export, and flip winners across the
// package path order repeatedly, and the type names cycle kind/arity/presence. `resolve_package` after
// every operation runs (under `debug_assertions`, i.e. the dev profile this test runs in) all five
// drift assertions — reverse-dependency index, value/type candidate indexes, materialized type index,
// and the four-category package-naming oracle — each comparing the incrementally maintained state to a
// from-scratch rebuild. Any drift panics, so this is in-tree, re-runnable coverage of the incremental
// path equalling the full rebuild across thousands of states. Fixed seed = reproducible.
#[test]
fn seeded_soak_incremental_matches_full_rebuild() {
    const OPERATION_COUNT: usize = 3000;
    let paths = [
        "/pkg/R/f0.R",
        "/pkg/R/f1.R",
        "/pkg/R/f2.R",
        "/pkg/R/f3.R",
        "/pkg/R/f4.R",
        "/pkg/R/f5.R",
    ];

    let mut analysis_state = typing_analysis("/pkg");
    let mut rng = Xorshift(0x9E37_79B9_7F4A_7C15);
    let mut live_sources: Vec<Option<String>> = vec![None; paths.len()];

    for _ in 0..OPERATION_COUNT {
        let index = rng.below(paths.len());
        let path = paths[index];
        match (&live_sources[index], rng.below(10)) {
            (Some(_), 0) => {
                analysis_state
                    .delete_document(std::path::Path::new(path))
                    .expect("delete should succeed for a live document");
                live_sources[index] = None;
            }
            (Some(current), 1..=3) => {
                let range = whole_document_range(current);
                let new_source = random_source(&mut rng);
                analysis_state
                    .edit_document(
                        std::path::Path::new(path),
                        &[DocumentChange {
                            range,
                            text: new_source.clone(),
                        }],
                    )
                    .expect("whole-document edit should apply");
                live_sources[index] = Some(new_source);
            }
            _ => {
                let new_source = random_source(&mut rng);
                analysis_state
                    .add_document_from_source(PathBuf::from(path), &new_source)
                    .expect("generated source should parse");
                live_sources[index] = Some(new_source);
            }
        }

        analysis::resolve_package(&mut analysis_state);
    }
}

// A deterministic xorshift64 generator: a fixed nonzero seed makes the whole soak run reproducible
// without pulling in a dependency.
struct Xorshift(u64);

impl Xorshift {
    fn next_u64(&mut self) -> u64 {
        let mut state = self.0;
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        self.0 = state;
        state
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next_u64() % bound as u64) as usize
    }
}

// A short package source built from a small pool of names and type names, mixing the constructs whose
// incremental maintenance the assertions guard: plain top-level assigns, bare-block globals,
// conditional (non-global) assigns, cross-file value references / re-exports, `@type` / `@alias`
// definitions, type-annotation references, and typed-function interfaces. The small pools force
// repeated overwrites and winner flips across the package path order.
fn random_source(rng: &mut Xorshift) -> String {
    let names = ["a", "b", "c", "d"];
    let types = ["T1", "T2", "T3"];
    let line_count = 1 + rng.below(3);
    let mut lines = Vec::new();
    for _ in 0..line_count {
        let name = names[rng.below(names.len())];
        let other = names[rng.below(names.len())];
        let type_name = types[rng.below(types.len())];
        lines.push(match rng.below(9) {
            0 => format!("{name} <- 1L"),
            1 => format!("{name} <- \"x\""),
            2 => format!("{{\n  {name} <- 2L\n}}"),
            3 => format!("if (TRUE) {{\n  {name} <- 3L\n}}"),
            4 => format!("{name} <- {other}"),
            5 => format!("#: @type {type_name} {{integer}}"),
            6 => format!("#: @alias {type_name} {{double}}"),
            7 => format!("#: {type_name}\n{name} <- 1L"),
            _ => format!("#: fn(value: integer) -> integer\n{name} <- function(value) value"),
        });
    }
    lines.push(String::new());
    lines.join("\n")
}

// The text range spanning an entire document, so an edit can replace it wholesale (the generated
// sources are ASCII, so a character index is the column).
fn whole_document_range(source: &str) -> TextRange {
    let line_index = source.matches('\n').count();
    let character_index = source.rsplit('\n').next().unwrap_or("").chars().count();
    TextRange {
        start: TextPosition {
            line_index: 0,
            character_index: 0,
        },
        end: TextPosition {
            line_index,
            character_index,
        },
    }
}

// Run manually with: cargo test -p analysis --release --test test_incremental -- --ignored --nocapture
#[test]
#[ignore = "timing benchmark, run manually"]
fn benchmark_single_file_recheck_in_large_package() {
    let items_per_file = 30;
    let mut analysis_state = typing_analysis("/pkg");
    for (path, source) in common::generate_package(500, items_per_file) {
        analysis_state
            .add_document_from_source(path, &source)
            .expect("document should parse");
    }

    let full_start = std::time::Instant::now();
    analysis::run_full(&mut analysis_state);
    let full_elapsed = full_start.elapsed();

    // Body-only edit in one file keeps the exported interface identical.
    analysis_state
        .add_document_from_source(
            common::file_path(250),
            &common::generate_file(250, items_per_file, true),
        )
        .expect("document should parse");
    let incremental_start = std::time::Instant::now();
    let recomputed = analysis::typecheck(&mut analysis_state);
    let incremental_elapsed = incremental_start.elapsed();

    println!("full check: {full_elapsed:?}");
    println!(
        "single-file recheck: {incremental_elapsed:?} ({} documents recomputed)",
        recomputed.len()
    );
    assert_eq!(recomputed.len(), 1, "body edit should recheck one document");
}
