//! Runs every semantics fixture source through BOTH stacks and compares the
//! semantic diagnostic classes: `type` (typing errors), `annotation` (block
//! and annotation-syntax errors, published regardless of typing mode),
//! `unresolved`, `unused`, and `strict`. Classes only one stack models
//! (legacy lints, legacy naming warnings, and the syntax class — the new
//! parser's errors must be better than the oracle's, not identical) are not
//! compared.
//!
//! Two findings match when class and message are byte-identical and the new
//! range equals or lies inside the legacy range: the rewrite is required to
//! be at least as precise as the oracle, and strictly tighter ranges are an
//! intended improvement, not a divergence. Cases where the oracle itself is
//! wrong are listed in `ACCEPTED_DIVERGENCES` with the reason; everything
//! else must match, and each suite's test fails on any unexplained
//! divergence (the details land in `target/differential-<suite>.txt`).
//!
//! Publication rules mirror the legacy host edge: type errors and strict
//! findings honor the per-file typing directive (`# typing: ...` /
//! `#: @strict`) over the configured default, annotation and naming findings
//! are always published.

use analysis::{Analysis, CheckConfig, DiagnosticCode, LintConfig};
use semantics::diagnostics::{file_diagnostics, strict_diagnostics};
use semantics::{
    DocumentKind, ProjectFiles, RootDatabase, SourceFile, TypingMode, file_typing_mode,
};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// One comparable finding: (class, start byte, end byte, message).
type Finding = (&'static str, usize, usize, String);

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
];

fn legacy_findings(source: &str, suite: &Suite) -> Vec<Finding> {
    let mut analysis_state = Analysis::new(
        PathBuf::from("/pkg"),
        LintConfig::default(),
        CheckConfig {
            unused: true,
            typing: true,
            strict: suite.strict,
        },
    );
    let Ok(document_id) =
        analysis_state.add_document_from_source(PathBuf::from(suite.document_path), source)
    else {
        return Vec::new();
    };
    analysis::run_full(&mut analysis_state);
    let mut findings = Vec::new();
    for diagnostic in analysis_state.document_diagnostics(document_id) {
        let class = match diagnostic.code {
            DiagnosticCode::TypeError => "type",
            DiagnosticCode::AnnotationError => "annotation",
            DiagnosticCode::Unresolved => "unresolved",
            DiagnosticCode::Unused => "unused",
            DiagnosticCode::Strict => "strict",
            DiagnosticCode::SyntaxError | DiagnosticCode::Lint(_) | DiagnosticCode::Naming => {
                continue;
            }
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

fn new_findings(source: &str, suite: &Suite) -> Vec<Finding> {
    let db = RootDatabase::default();
    semantics::stubs::install_shipped_stubs(&db);
    let file = SourceFile::new(&db, source.to_owned(), suite.kind);
    ProjectFiles::new(&db, vec![file]);
    let (typing_enabled, strict_enabled) = match file_typing_mode(&db, file) {
        Some(TypingMode::Off) => (false, false),
        Some(TypingMode::On) => (true, false),
        Some(TypingMode::Strict) => (true, true),
        None => (true, suite.strict),
    };
    let mut findings = Vec::new();
    for diagnostic in file_diagnostics(&db, file) {
        let class = match diagnostic.code {
            "type-mismatch" if typing_enabled => "type",
            "annotation" => "annotation",
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
    if strict_enabled {
        for diagnostic in strict_diagnostics(&db, file) {
            findings.push((
                "strict",
                u32::from(diagnostic.range.start()) as usize,
                u32::from(diagnostic.range.end()) as usize,
                diagnostic.message,
            ));
        }
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

fn run_suite(suite: &Suite) {
    let suite_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../semantics/tests")
        .join(suite.directory);
    let files = syntax::testing::parse_fixture_files(&suite_dir);
    assert!(!files.is_empty(), "suite not found at {suite_dir:?}");

    let mut report = String::new();
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
            let legacy = std::panic::catch_unwind(|| legacy_findings(&case.source, suite));
            let Ok(legacy) = legacy else {
                if accepted_case {
                    accepted += 1;
                } else {
                    diverging += 1;
                    let _ = writeln!(report, "==== {} ====\n  legacy oracle PANICKED", case.id);
                }
                continue;
            };
            let new = new_findings(&case.source, suite);
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
        "differential {}: {matching}/{cases} cases match, {accepted} accepted oracle divergences, {diverging} unexplained\n",
        suite.directory
    );
    println!("{summary}{report}");
    let report_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(suite.report);
    let _ = std::fs::write(&report_path, format!("{summary}\n{report}"));
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

    let suite = Suite {
        directory: "corpus",
        document_path: "/pkg/R/case.R",
        kind: DocumentKind::Package,
        strict: false,
        report: "differential-corpus.txt",
    };
    let mut report = String::new();
    let mut rollup: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut files = 0usize;
    let mut matching = 0usize;
    let mut skipped_syntax = 0usize;
    let mut diverging = 0usize;
    let mut panicking: Vec<String> = Vec::new();

    for path in &paths {
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        let relative = path.strip_prefix(&corpus_root).unwrap_or(path);
        if !syntax::parse(&source).errors().is_empty() {
            skipped_syntax += 1;
            continue;
        }
        let (legacy, legacy_syntax_errors) = legacy_findings_and_syntax(&source, &suite);
        if legacy_syntax_errors {
            skipped_syntax += 1;
            continue;
        }
        files += 1;
        // A panic on one file (a bug to fix) must not kill the whole triage
        // run: record the file and keep sweeping.
        let new = std::panic::catch_unwind(|| new_findings(&source, &suite));
        let Ok(new) = new else {
            panicking.push(relative.display().to_string());
            *rollup.entry("new stack PANICKED".to_owned()).or_default() += 1;
            continue;
        };
        if findings_match(&legacy, &new) {
            matching += 1;
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

    let summary = format!(
        "differential corpus: {matching}/{files} accepted files match, {diverging} diverging, {} panicking, {skipped_syntax} skipped for syntax errors\n",
        panicking.len()
    );
    let mut panic_text = String::new();
    for file in &panicking {
        let _ = writeln!(panic_text, "  PANIC: {file}");
    }
    println!("{summary}\n{panic_text}{rollup_text}");
    let report_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(suite.report);
    let _ = std::fs::write(
        &report_path,
        format!(
            "{summary}\n== panicking files ==\n{panic_text}\n== divergent-message rollup ==\n{rollup_text}\n== per-file details ==\n{report}"
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
        } else if path.extension().is_some_and(|extension| extension == "R") {
            paths.push(path);
        }
    }
}

/// Legacy findings plus whether the file had any syntax error (out-of-scope
/// files are skipped rather than compared).
fn legacy_findings_and_syntax(source: &str, suite: &Suite) -> (Vec<Finding>, bool) {
    let mut analysis_state = Analysis::new(
        PathBuf::from("/pkg"),
        LintConfig::default(),
        CheckConfig {
            unused: true,
            typing: true,
            strict: suite.strict,
        },
    );
    let Ok(document_id) =
        analysis_state.add_document_from_source(PathBuf::from(suite.document_path), source)
    else {
        return (Vec::new(), true);
    };
    analysis::run_full(&mut analysis_state);
    let mut findings = Vec::new();
    let mut syntax_errors = false;
    for diagnostic in analysis_state.document_diagnostics(document_id) {
        let class = match diagnostic.code {
            DiagnosticCode::TypeError => "type",
            DiagnosticCode::AnnotationError => "annotation",
            DiagnosticCode::Unresolved => "unresolved",
            DiagnosticCode::Unused => "unused",
            DiagnosticCode::Strict => "strict",
            DiagnosticCode::SyntaxError => {
                syntax_errors = true;
                continue;
            }
            DiagnosticCode::Lint(_) | DiagnosticCode::Naming => continue,
        };
        findings.push((
            class,
            diagnostic.range.start_byte,
            diagnostic.range.end_byte,
            diagnostic.message,
        ));
    }
    findings.sort();
    (findings, syntax_errors)
}
