//! Runs every semantics fixture source through BOTH stacks and compares the
//! semantic diagnostic classes: `type` (typing errors), `annotation` (block
//! and annotation-syntax errors, published regardless of typing mode),
//! `unresolved`, `unused`, and `strict`. Classes only one stack models
//! (legacy lints, legacy naming warnings, and the syntax class — the new
//! parser's errors must be better than the oracle's, not identical) are not
//! compared.
//!
//! The comparison harness — finding extraction, the class + range-containment
//! matching policy, and the accepted oracle-deficit filters — lives in the
//! `differential` library and is shared with the corpus and fuzz arms. Cases
//! where the oracle itself is wrong are listed in `ACCEPTED_DIVERGENCES` with
//! the reason; everything else must match, and each suite's test fails on any
//! unexplained divergence (the details land in `target/differential-<suite>.txt`).

use differential::{filter_oracle_deficits, findings_match, legacy_findings, new_findings};
use semantics::DocumentKind;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// How one suite runs: where its fixtures live, how the document is
/// classified (legacy classifies by path — only files under `R/` are package
/// members), and whether the configured default enables strict.
struct Suite {
    directory: &'static str,
    document_path: &'static str,
    kind: DocumentKind,
    strict: bool,
    report: &'static str,
}

/// Cases where the legacy oracle emits findings that are wrong and the new
/// stack is correct, with the reason each divergence is accepted.
const ACCEPTED_DIVERGENCES: &[(&str, &str)] = &[
    (
        "stubs__debugging_family_resolves",
        "the shipped stub corpus grew the interactive-debugging family (debug, menu, askYesNo); the frozen oracle's corpus predates it and reports unresolved",
    ),
    (
        "stubs__error_handler_stubs_resolve",
        "the shipped stub corpus grew recover and traceback; the frozen oracle's corpus predates it and reports unresolved",
    ),
    (
        "scoping__forward_capture_resolves_after_repass",
        "legacy flags a forward-captured binding as unresolved and unused; the new naming pass resolves captures written later in the enclosing frame",
    ),
    (
        "scoping__local_mutual_recursion_is_tolerant",
        "legacy flags the second function of a local mutually recursive pair as unresolved and unused",
    ),
    (
        "scoping__forward_capture_sees_the_frame_write",
        "legacy flags a frame write that a closure reads later as unresolved and unused",
    ),
    (
        "interface__growing_self_reference_pins_to_unknown",
        "the legacy interface fixed-point panics on a self-referential definition whose type grows each round; the new stack pins the scheme to Unknown at the round cap",
    ),
    (
        "recursion__growing_recursion_is_attributed",
        "the legacy interface fixed-point panics on a self-referential definition whose type grows each round; the new stack pins to Unknown and reports the undetermined read under strict mode",
    ),
    (
        "recursion__annotated_recursion_has_no_origin",
        "legacy leaves a recursive call's result Unknown even under a declared annotation and then fails its own declared-return check; the new fixpoint types the recursive call through the declaration",
    ),
    (
        "recursion__pure_self_call_is_attributed",
        "legacy records no strict origin on a recursive binding whose Unknown comes from the reference cycle itself; the new stack attributes the whole binding",
    ),
    (
        "resolution__forward_capture_resolves_to_the_later_binding",
        "legacy flags a forward-captured script binding as unresolved and unused; the new naming pass resolves closure reads against the whole settled frame",
    ),
    (
        "reads_that_keep_bindings_alive__pipe_read_is_a_use",
        "legacy does not model reads inside `|>` pipelines as uses and wrongly calls the piped binding dead",
    ),
    (
        "unknown_names__misspelled_type_inside_a_type_body",
        "the rewrite's unknown-type-name diagnostic has no oracle counterpart in the compared classes — legacy's type resolution warns through its naming class, which the differential excludes",
    ),
    (
        "unknown_names__unknown_type_in_a_checked_annotation",
        "the rewrite's unknown-type-name diagnostic has no oracle counterpart in the compared classes — legacy's type resolution warns through its naming class, which the differential excludes",
    ),
    (
        "unknown_names__unknown_generic_application",
        "the rewrite's unknown-type-name diagnostic has no oracle counterpart in the compared classes — legacy's type resolution warns through its naming class, which the differential excludes",
    ),
    (
        "new_shape__new_on_alias_is_refused",
        "legacy reports `@new` on an alias through its naming class, which the differential excludes; the rewrite reports it in the annotation class",
    ),
    (
        "generic_arity__applied_arity_must_match",
        "legacy classes generic-application arity as a syntax error, which the differential excludes; the rewrite reports it in the annotation class",
    ),
    (
        "generic_arity__non_generic_takes_no_arguments",
        "legacy classes generic-application arity as a syntax error, which the differential excludes; the rewrite reports it in the annotation class",
    ),
    (
        "generic_arity__bare_generic_reference_must_be_applied",
        "legacy classes generic-application arity as a syntax error, which the differential excludes; the rewrite reports it in the annotation class",
    ),
    (
        "vectors__alias_vector_element_must_be_atomic",
        "both stacks refuse the non-atomic vector element with the same wording, but the oracle blames the alias declaration (its check hits the violation while expanding the alias) where the rewrite blames the `Person[]` use site — outside the oracle span, so containment cannot credit it",
    ),
    (
        "vectors__structural_vector_element_is_refused",
        "the legacy type grammar cannot parse a structural type under a `[]` suffix inside `fn(...)` and reports a parse error; the rewrite parses the shape and reports the actual vector-element rule",
    ),
    (
        "higher_order__any_callee_arguments_are_not_checked",
        "the oracle never defined `[` on vectors and refuses it; the rewrite types vector subsetting per the reference's index-shape rules",
    ),
    (
        "vector_subsetting__scalar_index_selects_one_element",
        "the oracle never defined `[` on vectors and refuses it; the rewrite types vector subsetting per the reference's index-shape rules",
    ),
    (
        "vector_subsetting__vector_index_selects_a_sub_vector",
        "the oracle never defined `[` on vectors and refuses it; the rewrite types vector subsetting per the reference's index-shape rules",
    ),
    (
        "vector_subsetting__logical_mask_keeps_the_vector_shape",
        "the oracle never defined `[` on vectors and refuses it; the rewrite types vector subsetting per the reference's index-shape rules",
    ),
    (
        "vector_subsetting__named_subject_keeps_names_under_vector_indexes",
        "the oracle never defined `[` on vectors and refuses it; the rewrite types vector subsetting per the reference's index-shape rules",
    ),
    (
        "vector_subsetting__character_index_is_allowed_on_unnamed_vectors",
        "the oracle never defined `[` on vectors and refuses it; the rewrite types vector subsetting per the reference's index-shape rules",
    ),
    (
        "vector_subsetting__undetermined_index_claims_a_scalar",
        "the oracle never defined `[` on vectors and refuses it; the rewrite types vector subsetting per the reference's index-shape rules",
    ),
    (
        "exported_constraints__forwarded_dots_disable_arity_checking",
        "legacy arity-checks a call whose argument is the enclosing function's bare `...` as if the dots were one fixed argument; the forwarded count is unknowable statically, so the rewrite skips arity checks on such calls",
    ),
    (
        "pipes__pipe_argument_errors_blame_the_piped_value",
        "the oracle never modeled `|>` (opaque operator, silent Unknown); the rewrite lowers the pipe as the call R itself rewrites it to and checks the piped argument",
    ),
    (
        "pipes__placeholder_without_the_piped_first_argument_is_missing_an_argument",
        "the oracle never modeled `|>` (opaque operator, silent Unknown); the rewrite lowers the placeholder form as R does and reports the genuinely missing required argument",
    ),
];

/// Allowlist entries justified by an oracle PANIC (a `debug_assert` in the
/// legacy fixed-point): testable only in debug builds — release compiles the
/// assert out and the case may then match, which is not staleness.
const ORACLE_PANIC_CASES: &[&str] = &[
    "interface__growing_self_reference_pins_to_unknown",
    "recursion__growing_recursion_is_attributed",
];

fn run_suite(suite: &Suite) {
    let suite_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/semantics/tests")
        .join(suite.directory);
    let files = syntax::testing::parse_fixture_files(&suite_dir);
    assert!(!files.is_empty(), "suite not found at {suite_dir:?}");

    let mut report = String::new();
    let mut wording_report = String::new();
    let mut cases = 0usize;
    let mut matching = 0usize;
    let mut accepted = 0usize;
    let mut diverging = 0usize;
    let mut stale_acceptances = Vec::new();

    for file in &files {
        for case in &file.cases {
            cases += 1;
            let accepted_case = ACCEPTED_DIVERGENCES.iter().any(|(id, _)| *id == case.id);
            // The oracle itself can crash (its fixed-point panics on some
            // inputs the rewrite handles); such a case counts as an accepted
            // divergence when allowlisted, a failure otherwise.
            let legacy = std::panic::catch_unwind(|| {
                legacy_findings(&case.source, suite.document_path, suite.strict).0
            });
            let Ok(legacy) = legacy else {
                if accepted_case {
                    accepted += 1;
                } else {
                    diverging += 1;
                    let _ = writeln!(report, "==== {} ====\n  legacy oracle PANICKED", case.id);
                }
                continue;
            };
            let new = new_findings(&case.source, suite.kind, suite.strict);
            let mut wording = Vec::new();
            if findings_match(&legacy, &new, &mut wording) {
                matching += 1;
                for (legacy_message, new_message) in wording {
                    let _ = writeln!(
                        wording_report,
                        "  {}: legacy \"{legacy_message}\" / new \"{new_message}\"",
                        case.id
                    );
                }
                let panic_class = ORACLE_PANIC_CASES.contains(&case.id.as_str());
                if accepted_case && (cfg!(debug_assertions) || !panic_class) {
                    stale_acceptances.push(case.id.clone());
                } else if accepted_case {
                    accepted += 1;
                }
                continue;
            }
            if accepted_case {
                accepted += 1;
                continue;
            }
            diverging += 1;
            let _ = writeln!(report, "==== {} ====", case.id);
            for finding in &legacy {
                if !new.contains(finding) {
                    let _ = writeln!(
                        report,
                        "  legacy only: {}..{} [{}] {}",
                        finding.1, finding.2, finding.0, finding.3
                    );
                }
            }
            for finding in &new {
                if !legacy.contains(finding) {
                    let _ = writeln!(
                        report,
                        "  new only:    {}..{} [{}] {}",
                        finding.1, finding.2, finding.0, finding.3
                    );
                }
            }
        }
    }

    let summary = format!(
        "differential {}: {matching}/{cases} cases match, {accepted} accepted oracle divergences, {diverging} unexplained\n",
        suite.directory
    );
    println!("{summary}{report}");
    let report_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(suite.report);
    let _ = std::fs::write(
        &report_path,
        format!("{summary}\n{report}\n== wording differences (informational) ==\n{wording_report}"),
    );
    assert!(
        stale_acceptances.is_empty(),
        "cases match but are still allowlisted — remove them from ACCEPTED_DIVERGENCES: {stale_acceptances:?}"
    );
    assert!(
        diverging == 0,
        "{diverging} unexplained divergence(s); see the report above or target/{}",
        suite.report
    );
}

#[test]
fn differential_typing() {
    run_suite(&Suite {
        directory: "typing",
        document_path: "/pkg/R/case.R",
        kind: DocumentKind::Package,
        strict: false,
        report: "differential-typing.txt",
    });
}

#[test]
fn differential_scripts() {
    run_suite(&Suite {
        directory: "typing-scripts",
        document_path: "/pkg/scripts/case.R",
        kind: DocumentKind::Script,
        strict: false,
        report: "differential-scripts.txt",
    });
}

#[test]
fn differential_strict() {
    run_suite(&Suite {
        directory: "typing-strict",
        document_path: "/pkg/R/case.R",
        kind: DocumentKind::Package,
        strict: true,
        report: "differential-strict.txt",
    });
}

/// The real-file arm: every corpus `.R` file both parsers accept, compared
/// with the same matching policy as the fixture suites. Parity is scoped to
/// inputs both stacks parse cleanly, so files with syntax errors on either
/// side are counted and skipped. Ignored by default — it needs the fetched
/// corpus (`scripts/fetch-corpus.sh`) and runs the full legacy pipeline per
/// file; run with `cargo test -p differential -- --ignored differential_corpus`.
/// The report leads with a frequency rollup of divergent messages so one
/// wording gap repeated across hundreds of files reads as one line.
#[test]
#[ignore = "needs the fetched corpus; run explicitly with -- --ignored"]
fn differential_corpus() {
    let corpus_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus");
    let mut paths = Vec::new();
    for directory in ["r-base", "cran"] {
        collect_r_files(&corpus_root.join(directory), &mut paths);
    }
    paths.sort();
    assert!(
        !paths.is_empty(),
        "no corpus files under {corpus_root:?} — run scripts/fetch-corpus.sh first"
    );

    let document_path = "/pkg/R/case.R";
    let report_name = "differential-corpus.txt";
    let mut report = String::new();
    let mut rollup: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut wording_rollup: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();
    let mut files = 0usize;
    let mut matching = 0usize;
    let mut skipped_syntax = 0usize;
    let mut diverging = 0usize;
    let mut panicking: Vec<String> = Vec::new();
    let mut oracle_deficit_rollup: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();

    for path in &paths {
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        let relative = path.strip_prefix(&corpus_root).unwrap_or(path);
        if !syntax::parse(&source).errors().is_empty() {
            skipped_syntax += 1;
            continue;
        }
        // The oracle may panic on its own defects; a panicking oracle cannot
        // be consulted for the file, so it is counted and skipped rather
        // than killing the sweep (only new-stack panics fail the test).
        let legacy_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            legacy_findings(&source, document_path, false)
        }));
        let Ok((legacy, legacy_syntax_errors)) = legacy_result else {
            *rollup
                .entry("oracle panicked (tolerated)".to_owned())
                .or_default() += 1;
            continue;
        };
        if legacy_syntax_errors {
            skipped_syntax += 1;
            continue;
        }
        files += 1;
        // A panic on one file (a bug to fix) must not kill the whole triage
        // run: record the file and keep sweeping.
        let new = std::panic::catch_unwind(|| new_findings(&source, DocumentKind::Package, false));
        let Ok(new) = new else {
            panicking.push(relative.display().to_string());
            *rollup.entry("new stack PANICKED".to_owned()).or_default() += 1;
            continue;
        };
        let (legacy, _) = filter_oracle_deficits(legacy, &new, None, &mut oracle_deficit_rollup);
        let facts = differential::new_stack_facts(&source, DocumentKind::Package);
        let (legacy, new) =
            differential::filter_pipe_opacity(legacy, new, &facts, &mut oracle_deficit_rollup);
        let mut wording = Vec::new();
        if findings_match(&legacy, &new, &mut wording) {
            matching += 1;
            for (legacy_message, new_message) in wording {
                *wording_rollup
                    .entry(format!(
                        "legacy \"{legacy_message}\" / new \"{new_message}\""
                    ))
                    .or_default() += 1;
            }
            continue;
        }
        diverging += 1;
        let _ = writeln!(report, "==== {} ====", relative.display());
        for finding in &legacy {
            if !new.contains(finding) {
                *rollup
                    .entry(format!("legacy only [{}] {}", finding.0, finding.3))
                    .or_default() += 1;
                let _ = writeln!(
                    report,
                    "  legacy only: {}..{} [{}] {}",
                    finding.1, finding.2, finding.0, finding.3
                );
            }
        }
        for finding in &new {
            if !legacy.contains(finding) {
                *rollup
                    .entry(format!("new only    [{}] {}", finding.0, finding.3))
                    .or_default() += 1;
                let _ = writeln!(
                    report,
                    "  new only:    {}..{} [{}] {}",
                    finding.1, finding.2, finding.0, finding.3
                );
            }
        }
    }

    let mut ranked: Vec<(usize, &str)> = rollup
        .iter()
        .map(|(message, count)| (*count, message.as_str()))
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    let mut rollup_text = String::new();
    for (count, message) in ranked.iter().take(60) {
        let _ = writeln!(rollup_text, "{count:6}  {message}");
    }

    let mut deficit_text = String::new();
    let mut ranked_deficit: Vec<(usize, &str)> = oracle_deficit_rollup
        .iter()
        .map(|(name, count)| (*count, name.as_str()))
        .collect();
    ranked_deficit.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    for (count, name) in ranked_deficit.iter().take(40) {
        let _ = writeln!(deficit_text, "{count:6}  {name}");
    }
    let accepted_deficits: usize = oracle_deficit_rollup.values().sum();
    let summary = format!(
        "differential corpus: {matching}/{files} accepted files match, {diverging} diverging, {} panicking, {skipped_syntax} skipped for syntax errors, {accepted_deficits} oracle-deficit findings accepted\n",
        panicking.len()
    );
    let mut panic_text = String::new();
    for file in &panicking {
        let _ = writeln!(panic_text, "  PANIC: {file}");
    }
    println!("{summary}\n{panic_text}{rollup_text}");
    let mut wording_text = String::new();
    let mut ranked_wording: Vec<(usize, &str)> = wording_rollup
        .iter()
        .map(|(message, count)| (*count, message.as_str()))
        .collect();
    ranked_wording.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    for (count, message) in ranked_wording.iter().take(40) {
        let _ = writeln!(wording_text, "{count:6}  {message}");
    }
    let report_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(report_name);
    let _ = std::fs::write(
        &report_path,
        format!(
            "{summary}\n== panicking files ==\n{panic_text}\n== divergent-message rollup ==\n{rollup_text}\n== accepted oracle-deficit names (legacy-only unresolved the rewrite resolves) ==\n{deficit_text}\n== wording differences (informational) ==\n{wording_text}\n== per-file details ==\n{report}"
        ),
    );
    assert!(
        panicking.is_empty(),
        "the new stack panicked on {} corpus file(s):\n{panic_text}",
        panicking.len()
    );
}

fn collect_r_files(directory: &Path, paths: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_r_files(&path, paths);
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| matches!(extension, "R" | "r" | "S" | "s" | "q"))
        {
            paths.push(path);
        }
    }
}
