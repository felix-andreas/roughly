//! Cross-stack resource measurement over the real corpus, for the rewrite's
//! memory and speed gates: the resident set must stay linear in LoC and
//! within 1.5x of the legacy stack; wall-clock must meet or beat it.
//!
//! Each stack runs in its own ignored test so per-process peak-RSS numbers
//! (`VmHWM`) are not polluted by the other stack:
//!
//! ```text
//! cargo test -p differential --release --test test_stats -- --ignored stats_new_stack
//! cargo test -p differential --release --test test_stats -- --ignored stats_legacy_stack
//! ```
//!
//! Both group corpus files per package directory (a corpus is not one
//! coherent package), build every package fully — parse, name, check, and
//! render every file's diagnostics — and RETAIN all state, measuring the
//! fully-warmed all-packages worst case. Reports land in
//! `target/stats-{new,legacy}.txt`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[test]
#[ignore = "measurement instrument; needs the fetched corpus and a release build"]
fn stats_new_stack() {
    use semantics::{DocumentKind, ProjectFiles, RootDatabase, SourceFile};

    let packages = corpus_packages();
    assert!(!packages.is_empty(), "run scripts/fetch-corpus.sh first");

    let mut total_lines = 0usize;
    let mut total_bytes = 0usize;
    let mut diagnostics = 0usize;
    let start = Instant::now();

    // One database per package, all retained (the warm worst case). Phase
    // splits are cumulative wall per stage, each stage building on the
    // previous one's memoized results.
    let mut parse_time = std::time::Duration::ZERO;
    let mut lower_time = std::time::Duration::ZERO;
    let mut check_time = std::time::Duration::ZERO;
    let mut render_time = std::time::Duration::ZERO;
    let mut retained = Vec::new();
    for sources in &packages {
        let db = RootDatabase::default();
        semantics::stubs::install_shipped_stubs(&db);
        let files: Vec<SourceFile> = sources
            .iter()
            .map(|source| {
                total_lines += source.lines().count();
                total_bytes += source.len();
                SourceFile::new(&db, source.clone(), DocumentKind::Package)
            })
            .collect();
        let project = ProjectFiles::new(&db, files.clone());
        let _ = project;

        let phase = Instant::now();
        for &file in &files {
            let _ = semantics::parse(&db, file);
        }
        parse_time += phase.elapsed();
        let phase = Instant::now();
        for &file in &files {
            for item in semantics::item_tree(&db, file) {
                let _ = semantics::item_hir(&db, item);
                let _ = semantics::item_naming(&db, item);
            }
        }
        lower_time += phase.elapsed();
        let phase = Instant::now();
        for &file in &files {
            for item in semantics::item_tree(&db, file) {
                let _ = semantics::item_check(&db, item);
            }
        }
        check_time += phase.elapsed();
        let phase = Instant::now();
        for &file in &files {
            diagnostics += semantics::diagnostics::file_diagnostics(&db, file).len();
            diagnostics += semantics::diagnostics::strict_diagnostics(&db, file).len();
        }
        render_time += phase.elapsed();
        retained.push((db, files));
    }
    let elapsed = start.elapsed();
    let mut items = 0usize;
    for (db, files) in &retained {
        for &file in files {
            items += semantics::item_tree(db, file).len();
        }
    }
    println!(
        "items: {items}, check executions: {}",
        semantics::check::CHECK_EXECUTIONS.load(std::sync::atomic::Ordering::Relaxed)
    );
    println!(
        "resolve inner calls: {}",
        semantics::infer::RESOLVE_CALLS.load(std::sync::atomic::Ordering::Relaxed),
    );
    println!(
        "phases: parse {:.2}s, lower+name {:.2}s, check {:.2}s, render {:.2}s",
        parse_time.as_secs_f64(),
        lower_time.as_secs_f64(),
        check_time.as_secs_f64(),
        render_time.as_secs_f64()
    );
    let report = render_report(
        "new",
        packages.len(),
        total_lines,
        total_bytes,
        diagnostics,
        elapsed,
    );
    println!("{report}");
    let _ = std::fs::write(report_path("stats-new.txt"), report);
    drop(retained);
}

#[test]
#[ignore = "measurement instrument; needs the fetched corpus and a release build"]
fn stats_legacy_stack() {
    use analysis::{Analysis, CheckConfig, LintConfig};

    let packages = corpus_packages();
    assert!(!packages.is_empty(), "run scripts/fetch-corpus.sh first");

    let mut total_lines = 0usize;
    let mut total_bytes = 0usize;
    let mut diagnostics = 0usize;
    let start = Instant::now();

    let mut retained = Vec::new();
    for sources in &packages {
        let mut analysis_state = Analysis::new(
            PathBuf::from("/pkg"),
            LintConfig::default(),
            CheckConfig {
                unused: true,
                typing: true,
                strict: false,
            },
        );
        let mut ids = Vec::new();
        for (index, source) in sources.iter().enumerate() {
            total_lines += source.lines().count();
            total_bytes += source.len();
            if let Ok(id) = analysis_state
                .add_document_from_source(PathBuf::from(format!("/pkg/R/f{index}.R")), source)
            {
                ids.push(id);
            }
        }
        analysis::run_full(&mut analysis_state);
        for id in ids {
            diagnostics += analysis_state.document_diagnostics(id).len();
        }
        retained.push(analysis_state);
    }
    let elapsed = start.elapsed();
    let report = render_report(
        "legacy",
        packages.len(),
        total_lines,
        total_bytes,
        diagnostics,
        elapsed,
    );
    println!("{report}");
    let _ = std::fs::write(report_path("stats-legacy.txt"), report);
    drop(retained);
}

fn render_report(
    stack: &str,
    packages: usize,
    lines: usize,
    bytes: usize,
    diagnostics: usize,
    elapsed: std::time::Duration,
) -> String {
    let mut report = String::new();
    let _ = writeln!(
        report,
        "{stack} stack: {packages} packages, {lines} lines ({:.1} MiB), {diagnostics} findings",
        bytes as f64 / (1024.0 * 1024.0)
    );
    let _ = writeln!(
        report,
        "wall: {:.2}s ({:.2} us/line)",
        elapsed.as_secs_f64(),
        elapsed.as_secs_f64() * 1e6 / lines.max(1) as f64
    );
    for (label, key) in [("resident (VmRSS)", "VmRSS"), ("peak (VmHWM)", "VmHWM")] {
        if let Some(kb) = proc_status_kb(key) {
            let _ = writeln!(report, "{label}: {:.1} MiB", kb as f64 / 1024.0);
        }
    }
    report
}

fn proc_status_kb(key: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find(|line| line.starts_with(key))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

fn report_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(name)
}

/// The corpus grouped per package directory (each directory under `r-base/`
/// and `cran/` is one package), sources loaded.
fn corpus_packages() -> Vec<Vec<String>> {
    let corpus_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus");
    let mut packages = Vec::new();
    for top in ["r-base", "cran"] {
        let Ok(entries) = std::fs::read_dir(corpus_root.join(top)) else {
            continue;
        };
        let mut directories: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        directories.sort();
        for directory in directories {
            let mut files = Vec::new();
            collect_r_files(&directory, &mut files);
            files.sort();
            let sources: Vec<String> = files
                .iter()
                .filter_map(|path| std::fs::read(path).ok())
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .collect();
            if !sources.is_empty() {
                packages.push(sources);
            }
        }
    }
    packages
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
