//! The legacy-fixture-corpus arm of the cross-stack differential: every
//! single-file case INPUT from the frozen stack's own fixture suites runs
//! through both stacks with the shared matching policy. The legacy suites
//! hold years of accumulated shapes the rewrite's suites do not spell out;
//! sweeping them closes the coverage gap without porting expectations —
//! the oracle pipeline recomputes its findings from the sources, so the
//! legacy RENDERINGS (scheme dumps, wording) never constrain the rewrite.
//!
//! Multi-file composite cases (nested `#---- R/a.R` markers) are skipped and
//! counted — the shared harness compares one document at a time. Cases whose
//! source either stack refuses to parse are likewise counted and skipped
//! (syntax parity has its own suites). Divergences accepted by the shared
//! filters are counted, never failed; everything else fails the test with
//! the case id and both finding sets (details in
//! `target/differential-legacy-corpus.txt`).

use differential::{
    filter_legacy_slot_tolerance, filter_model_divergences, filter_oracle_deficits,
    filter_unknown_type_additions, findings_match, legacy_findings, new_findings, new_stack_facts,
};
use semantics::DocumentKind;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

/// Legacy suite directories whose case sources are semantic-diagnostic
/// inputs (lint/lowering/ide/stub suites render other facts).
const SUITES: &[&str] = &[
    "typecheck",
    "naming",
    "type_syntax",
    "diagnostics",
    "unused",
    "realworld",
];

/// Cases where the oracle itself is wrong and the rewrite is correct, with
/// the reason each divergence is accepted. Stale entries (cases that start
/// matching) fail the test.
const ACCEPTED_DIVERGENCES: &[(&str, &str)] = &[(
    "typecheck::generic_vector_elements__vector_suffix_rejects_non_atomic_element",
    "both stacks refuse the non-atomic vector element with the same wording, but the oracle blames \
     the alias declaration (its check hits the violation while expanding the alias) where the \
     rewrite blames the `Person[]` use site — a strictly better range that containment cannot \
     credit because it lies outside the oracle's span",
)];

/// Currently a triage instrument, not a gate: ~2% of cases surface known
/// rewrite gaps itemized in the backlog (expression-level annotations,
/// annotation block-form validations, declared-function shape checks,
/// nesting-depth caps, missing-formal flow analysis). Run it manually after
/// closing one of those to watch the count fall; promote it into the default
/// suite when the divergence list is empty or fully allowlisted.
#[test]
#[ignore = "triage instrument until the itemized rewrite gaps close; run explicitly with -- --ignored"]
fn legacy_corpus_differential() {
    let legacy_tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("../analysis-legacy/tests");
    let mut report = String::new();
    let mut rollup: BTreeMap<String, usize> = BTreeMap::new();
    let mut deficit_rollup: BTreeMap<String, usize> = BTreeMap::new();
    let mut cases = 0usize;
    let mut matching = 0usize;
    let mut skipped_composite = 0usize;
    let mut skipped_syntax = 0usize;
    let mut oracle_panics = 0usize;
    let mut accepted = 0usize;
    let mut diverging = 0usize;
    let mut stale_acceptances: Vec<&str> = Vec::new();

    for suite in SUITES {
        let mut paths = Vec::new();
        collect_test_files(&legacy_tests.join(suite), &mut paths);
        paths.sort();
        assert!(!paths.is_empty(), "legacy suite `{suite}` not found");
        for path in &paths {
            let text = std::fs::read_to_string(path).expect("read legacy fixture");
            // A multi-document composite (nested `#---- R/a.R` file markers)
            // cannot be expressed as one document; skip the whole file.
            if text
                .lines()
                .any(|line| line.starts_with("#---- ") && line.trim_end().ends_with(".R"))
            {
                skipped_composite += 1;
                continue;
            }
            let file = syntax::testing::parse_fixture_file(path, text);
            for case in &file.cases {
                let case_key = format!("{suite}::{}", case.id);
                if !syntax::parse(&case.source).errors().is_empty() {
                    skipped_syntax += 1;
                    continue;
                }
                let legacy = std::panic::catch_unwind(|| {
                    legacy_findings(&case.source, "/pkg/R/case.R", false)
                });
                let legacy = match legacy {
                    Ok((findings, syntax_errors)) => {
                        if syntax_errors {
                            skipped_syntax += 1;
                            continue;
                        }
                        findings
                    }
                    Err(_) => {
                        oracle_panics += 1;
                        continue;
                    }
                };
                let new = std::panic::catch_unwind(|| {
                    new_findings(&case.source, DocumentKind::Package, false)
                });
                let new = match new {
                    Ok(findings) => findings,
                    Err(payload) => {
                        eprintln!(
                            "---- new stack panicked on {case_key} ----\n{}\n----",
                            case.source
                        );
                        std::panic::resume_unwind(payload);
                    }
                };
                cases += 1;
                let accepted_case = ACCEPTED_DIVERGENCES.iter().any(|(id, _)| *id == case_key);
                let facts = new_stack_facts(&case.source, DocumentKind::Package);
                let new = filter_legacy_slot_tolerance(new, &legacy, &facts, &mut deficit_rollup);
                let new = filter_unknown_type_additions(new, &mut deficit_rollup);
                let (legacy, accepted_deficits) =
                    filter_oracle_deficits(legacy, &new, Some(&facts), &mut deficit_rollup);
                let (legacy, new) = filter_model_divergences(
                    legacy,
                    new,
                    &facts,
                    &accepted_deficits,
                    &mut deficit_rollup,
                );
                let mut wording = Vec::new();
                if findings_match(&legacy, &new, &mut wording) {
                    matching += 1;
                    if accepted_case {
                        stale_acceptances.push(
                            ACCEPTED_DIVERGENCES
                                .iter()
                                .find(|(id, _)| *id == case_key)
                                .map(|(id, _)| *id)
                                .unwrap_or_default(),
                        );
                    }
                    continue;
                }
                if accepted_case {
                    accepted += 1;
                    continue;
                }
                diverging += 1;
                let _ = writeln!(report, "==== {case_key} ====");
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
        }
    }

    let mut ranked: Vec<(usize, &str)> = rollup
        .iter()
        .map(|(message, count)| (*count, message.as_str()))
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    let mut rollup_text = String::new();
    for (count, message) in ranked.iter().take(80) {
        let _ = writeln!(rollup_text, "{count:6}  {message}");
    }
    let accepted_deficits: usize = deficit_rollup.values().sum();
    let summary = format!(
        "legacy-corpus differential: {matching}/{cases} cases match, {diverging} diverging, {accepted} accepted, {accepted_deficits} filter-accepted findings, {skipped_composite} composite and {skipped_syntax} syntax-scoped cases skipped, {oracle_panics} oracle panics\n"
    );
    println!("{summary}\n{rollup_text}");
    let report_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join("differential-legacy-corpus.txt");
    let _ = std::fs::write(
        &report_path,
        format!(
            "{summary}\n== divergent-message rollup ==\n{rollup_text}\n== per-case details ==\n{report}"
        ),
    );
    assert!(
        stale_acceptances.is_empty(),
        "cases match but are still allowlisted — remove them from ACCEPTED_DIVERGENCES: {stale_acceptances:?}"
    );
    assert!(
        diverging == 0,
        "{diverging} unexplained divergence(s); see target/differential-legacy-corpus.txt"
    );
}

fn collect_test_files(directory: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_test_files(&path, out);
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension == "test")
        {
            out.push(path);
        }
    }
}
