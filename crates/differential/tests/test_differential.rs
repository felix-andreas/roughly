//! Runs every typing-fixture source through BOTH stacks and compares the
//! semantic diagnostic classes: `type` (typing + annotation errors),
//! `unresolved`, and `unused`. Classes only one stack models (legacy lints,
//! legacy naming warnings, and the syntax class — the new parser's errors
//! must be better than the oracle's, not identical) are not compared.
//!
//! Two findings match when class and message are byte-identical and the new
//! range equals or lies inside the legacy range: the rewrite is required to
//! be at least as precise as the oracle, and strictly tighter ranges are an
//! intended improvement, not a divergence. Cases where the oracle itself is
//! wrong are listed in `ACCEPTED_DIVERGENCES` with the reason; everything
//! else must match, and the test fails on any unexplained divergence (the
//! details land in `target/differential-report.txt`).

use analysis::{Analysis, CheckConfig, DiagnosticCode, LintConfig};
use semantics::diagnostics::file_diagnostics;
use semantics::{DocumentKind, ProjectFiles, RootDatabase, SourceFile};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// One comparable finding: (class, start byte, end byte, message).
type Finding = (&'static str, usize, usize, String);

/// Cases where the legacy oracle emits findings that are wrong and the new
/// stack is correct, with the reason each divergence is accepted.
const ACCEPTED_DIVERGENCES: &[(&str, &str)] = &[
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
];

fn legacy_findings(source: &str) -> Vec<Finding> {
    let mut analysis_state = Analysis::new(
        PathBuf::from("/pkg"),
        LintConfig::default(),
        CheckConfig {
            unused: true,
            typing: true,
            strict: false,
        },
    );
    let Ok(document_id) =
        analysis_state.add_document_from_source(PathBuf::from("/pkg/R/case.R"), source)
    else {
        return Vec::new();
    };
    analysis::run_full(&mut analysis_state);
    let mut findings = Vec::new();
    for diagnostic in analysis_state.document_diagnostics(document_id) {
        let class = match diagnostic.code {
            DiagnosticCode::TypeError | DiagnosticCode::AnnotationError => "type",
            DiagnosticCode::Unresolved => "unresolved",
            DiagnosticCode::Unused => "unused",
            DiagnosticCode::SyntaxError
            | DiagnosticCode::Lint(_)
            | DiagnosticCode::Naming
            | DiagnosticCode::Strict => continue,
        };
        findings.push((
            class,
            diagnostic.range.start_byte,
            diagnostic.range.end_byte,
            diagnostic.message,
        ));
    }
    findings.sort();
    findings
}

fn new_findings(source: &str) -> Vec<Finding> {
    let db = RootDatabase::default();
    semantics::stubs::install_shipped_stubs(&db);
    let file = SourceFile::new(&db, source.to_owned(), DocumentKind::Package);
    ProjectFiles::new(&db, vec![file]);
    let mut findings = Vec::new();
    for diagnostic in file_diagnostics(&db, file) {
        let class = match diagnostic.code {
            "type-mismatch" | "annotation" => "type",
            "unresolved" => "unresolved",
            "unused" => "unused",
            _ => continue,
        };
        findings.push((
            class,
            u32::from(diagnostic.range.start()) as usize,
            u32::from(diagnostic.range.end()) as usize,
            diagnostic.message,
        ));
    }
    findings.sort();
    findings
}

/// Whether every legacy finding pairs with a distinct new finding of the same
/// class and message whose range is equal or contained, with no new findings
/// left over.
fn findings_match(legacy: &[Finding], new: &[Finding]) -> bool {
    if legacy.len() != new.len() {
        return false;
    }
    let mut used = vec![false; new.len()];
    'legacy: for (class, start, end, message) in legacy {
        for (index, (new_class, new_start, new_end, new_message)) in new.iter().enumerate() {
            let contained = new_start >= start && new_end <= end;
            if !used[index] && new_class == class && new_message == message && contained {
                used[index] = true;
                continue 'legacy;
            }
        }
        return false;
    }
    true
}

#[test]
fn differential_gate() {
    let typing_suite = Path::new(env!("CARGO_MANIFEST_DIR")).join("../semantics/tests/typing");
    let files = syntax::testing::parse_fixture_files(&typing_suite);
    assert!(
        !files.is_empty(),
        "typing suite not found at {typing_suite:?}"
    );

    let mut report = String::new();
    let mut cases = 0usize;
    let mut matching = 0usize;
    let mut accepted = 0usize;
    let mut diverging = 0usize;
    let mut stale_acceptances = Vec::new();

    for file in &files {
        for case in &file.cases {
            cases += 1;
            let legacy = legacy_findings(&case.source);
            let new = new_findings(&case.source);
            let accepted_case = ACCEPTED_DIVERGENCES.iter().any(|(id, _)| *id == case.id);
            if findings_match(&legacy, &new) {
                matching += 1;
                if accepted_case {
                    stale_acceptances.push(case.id.clone());
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
        "differential: {matching}/{cases} cases match, {accepted} accepted oracle divergences, {diverging} unexplained\n"
    );
    println!("{summary}{report}");
    let report_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/differential-report.txt");
    let _ = std::fs::write(&report_path, format!("{summary}\n{report}"));
    assert!(cases > 50, "the corpus should cover the typing suite");
    assert!(
        stale_acceptances.is_empty(),
        "cases match but are still allowlisted — remove them from ACCEPTED_DIVERGENCES: {stale_acceptances:?}"
    );
    assert!(
        diverging == 0,
        "{diverging} unexplained divergence(s); see the report above or target/differential-report.txt"
    );
}
