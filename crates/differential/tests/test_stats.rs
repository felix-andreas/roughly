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

/// The perf and memory witnesses as CI-checkable assertions (not prose): the
/// budgets come from the measured gate numbers with headroom — wall 40 µs per
/// line (measured ~25), resident 2 KiB per line (measured ~1.0), and resolve
/// steps 20 per line (measured ~8; the memoization regression tripwire).
/// Run wherever the corpus exists:
///
/// ```text
/// cargo test -p differential --release --test test_stats -- --ignored stats_witness
/// ```
#[test]
#[ignore = "witness; needs the fetched corpus and a release build"]
fn stats_witness() {
    use semantics::{DocumentKind, ProjectFiles, RootDatabase, SourceFile};

    let packages = corpus_packages();
    assert!(!packages.is_empty(), "run scripts/fetch-corpus.sh first");

    let mut total_lines = 0usize;
    let start = Instant::now();
    let mut retained = Vec::new();
    for sources in &packages {
        let db = RootDatabase::default();
        semantics::stubs::install_shipped_stubs(&db);
        let files: Vec<SourceFile> = sources
            .iter()
            .map(|source| {
                total_lines += source.lines().count();
                SourceFile::new(&db, source.clone(), DocumentKind::Package)
            })
            .collect();
        ProjectFiles::new(&db, files.clone());
        for &file in &files {
            let _ = semantics::diagnostics::file_diagnostics(&db, file);
            let _ = semantics::diagnostics::strict_diagnostics(&db, file);
        }
        retained.push((db, files));
    }
    let elapsed = start.elapsed();

    let microseconds_per_line = elapsed.as_secs_f64() * 1e6 / total_lines.max(1) as f64;
    assert!(
        microseconds_per_line <= 40.0,
        "cold-pass wall budget exceeded: {microseconds_per_line:.1} µs/line over {total_lines} lines"
    );
    let resolve_steps = semantics::infer::RESOLVE_CALLS.load(std::sync::atomic::Ordering::Relaxed);
    let steps_per_line = resolve_steps as f64 / total_lines.max(1) as f64;
    assert!(
        steps_per_line <= 20.0,
        "resolve-step budget exceeded (memoization regression): {steps_per_line:.1} steps/line"
    );
    if let Some(resident_kb) = proc_status_kb("VmRSS") {
        let bytes_per_line = resident_kb as f64 * 1024.0 / total_lines.max(1) as f64;
        assert!(
            bytes_per_line <= 2048.0,
            "resident-set budget exceeded: {bytes_per_line:.0} bytes/line"
        );
    }
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

/// The keystroke instrument: warm incremental latency on the largest corpus
/// package. Each "keystroke" edits one file's text through the salsa setter
/// (a byte appended inside a function body) and re-renders that file's
/// diagnostics — the server's per-edit work. Reports p50/p95 against the
/// quality-bar budgets (p50 ≤ 30 ms, p95 ≤ 100 ms).
///
/// ```text
/// cargo test -p differential --release --test test_stats -- --ignored stats_keystrokes --nocapture
/// ```
#[test]
#[ignore = "measurement instrument; needs the fetched corpus and a release build"]
fn stats_keystrokes() {
    use salsa::Setter;
    use semantics::{DocumentKind, ProjectFiles, RootDatabase, SourceFile};

    let packages = corpus_packages();
    assert!(!packages.is_empty(), "run scripts/fetch-corpus.sh first");
    let sources = packages
        .iter()
        .max_by_key(|sources| sources.iter().map(String::len).sum::<usize>())
        .expect("at least one package");
    let package_lines: usize = sources.iter().map(|source| source.lines().count()).sum();

    let mut db = RootDatabase::default();
    semantics::stubs::install_shipped_stubs(&db);
    let files: Vec<SourceFile> = sources
        .iter()
        .map(|source| SourceFile::new(&db, source.clone(), DocumentKind::Package))
        .collect();
    ProjectFiles::new(&db, files.clone());
    // Warm everything once (the server's steady state).
    for &file in &files {
        let _ = semantics::diagnostics::file_diagnostics(&db, file);
    }

    // The largest file is the worst case the quality bar cares about.
    let (edited_index, edited_source) = sources
        .iter()
        .enumerate()
        .max_by_key(|(_, source)| source.len())
        .expect("at least one file");
    let edited_file = files[edited_index];
    let edited_lines = edited_source.lines().count();

    // Append inside the file: a growing comment at the end simulates typing
    // (each revision differs by one byte; every item's text shifts nothing
    // before it, so item identity holds).
    let mut latencies: Vec<std::time::Duration> = Vec::new();
    let mut text = edited_source.clone();
    for _ in 0..50 {
        text.push('#');
        let start = Instant::now();
        edited_file.set_text(&mut db).to(text.clone());
        let _ = semantics::diagnostics::file_diagnostics(&db, edited_file);
        latencies.push(start.elapsed());
    }
    latencies.sort();
    let p50 = latencies[latencies.len() / 2];
    let p95 = latencies[latencies.len() * 95 / 100];
    println!(
        "keystrokes: package {package_lines} lines, edited file {edited_lines} lines, 50 edits"
    );
    println!(
        "latency: p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms",
        p50.as_secs_f64() * 1e3,
        p95.as_secs_f64() * 1e3,
        latencies.last().expect("nonempty").as_secs_f64() * 1e3,
    );

    // The raw from-scratch parse of the same text, to attribute the latency:
    // everything above this is item-tree rebuild plus salsa revalidation.
    let parse_start = Instant::now();
    let parses = 20;
    for _ in 0..parses {
        let _ = syntax::parse(&text);
    }
    println!(
        "raw parse of the edited file: {:.2} ms",
        parse_start.elapsed().as_secs_f64() * 1e3 / f64::from(parses)
    );
}

/// The multi-core cold pass: within each package's database, files fan out
/// across threads (storage-handle clones share memos). Compare against
/// `stats_new_stack`'s sequential wall to see the scaling.
///
/// ```text
/// cargo test -p differential --release --test test_stats -- --ignored stats_new_stack_parallel --nocapture
/// ```
#[test]
#[ignore = "measurement instrument; needs the fetched corpus and a release build"]
fn stats_new_stack_parallel() {
    use semantics::{DocumentKind, ProjectFiles, RootDatabase, SourceFile};

    let packages = corpus_packages();
    assert!(!packages.is_empty(), "run scripts/fetch-corpus.sh first");
    let workers = std::env::var("STATS_WORKERS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(std::num::NonZero::get)
                .unwrap_or(1)
        });

    let mut total_lines = 0usize;
    let mut diagnostics = 0usize;
    let start = Instant::now();
    let mut retained = Vec::new();
    for sources in &packages {
        let db = RootDatabase::default();
        semantics::stubs::install_shipped_stubs(&db);
        let files: Vec<SourceFile> = sources
            .iter()
            .map(|source| {
                total_lines += source.lines().count();
                SourceFile::new(&db, source.clone(), DocumentKind::Package)
            })
            .collect();
        ProjectFiles::new(&db, files.clone());

        let counted = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            let chunk_size = files.len().div_ceil(workers).max(1);
            for chunk in files.chunks(chunk_size) {
                let db = db.clone();
                let counted = &counted;
                scope.spawn(move || {
                    let mut local = 0usize;
                    for &file in chunk {
                        local += semantics::diagnostics::file_diagnostics(&db, file).len();
                        local += semantics::diagnostics::strict_diagnostics(&db, file).len();
                    }
                    counted.fetch_add(local, std::sync::atomic::Ordering::Relaxed);
                });
            }
        });
        diagnostics += counted.load(std::sync::atomic::Ordering::Relaxed);
        retained.push((db, files));
    }
    let elapsed = start.elapsed();
    println!("parallel cold pass ({workers} workers): {total_lines} lines, {diagnostics} findings");
    println!(
        "wall: {:.2}s ({:.2} us/line)",
        elapsed.as_secs_f64(),
        elapsed.as_secs_f64() * 1e6 / total_lines.max(1) as f64
    );
    drop(retained);
}
