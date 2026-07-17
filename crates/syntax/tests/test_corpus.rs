//! Corpus tests over real R sources.
//!
//! `scripts/fetch-corpus.sh` populates a gitignored `corpus/` directory with the
//! R base library and a set of CRAN packages. These tests run the hand-written
//! parser over every `.R` file there:
//!
//!   * `corpus_round_trip` checks the lossless invariant — the tree must reprint
//!     byte-for-byte to its input.
//!   * `corpus_acceptance` compares our "has errors" verdict against tree-sitter-r
//!     as an oracle, bucketing agreement. It reports but does not yet gate on
//!     individual files; the ours-only-error bucket is the parser's true-positive
//!     rejection list, which a later change tightens into an allowlist.
//!
//! Both skip cleanly when the corpus is absent (CI may run without a fetch).
//! Read files with lossy UTF-8 decoding: some R sources are latin1, not UTF-8.
//! Run with `cargo test -p syntax --test test_corpus -- --nocapture`.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

#[test]
fn corpus_round_trip() {
    let Some(root) = corpus_dir() else {
        eprintln!(
            "corpus_round_trip: no corpus found (set ROUGHLY_CORPUS_DIR or run \
             scripts/fetch-corpus.sh); skipping"
        );
        return;
    };

    let files = corpus_r_files(&root);
    let mut lossy = 0usize;
    let mut failures = Vec::new();
    for path in &files {
        let (text, was_lossy) = read_lossy(path);
        if was_lossy {
            lossy += 1;
        }
        if syntax::parse(&text).text() != text {
            failures.push(path.clone());
        }
    }

    eprintln!(
        "corpus_round_trip: {} .R files, {lossy} needed lossy UTF-8 conversion, {} round-trip \
         failure(s)",
        files.len(),
        failures.len()
    );
    for path in failures.iter().take(30) {
        eprintln!("  ROUND-TRIP FAIL: {}", path.display());
    }
    assert!(
        failures.is_empty(),
        "{} corpus file(s) did not reprint byte-exactly",
        failures.len()
    );
}

#[test]
fn corpus_acceptance() {
    let Some(root) = corpus_dir() else {
        eprintln!("corpus_acceptance: no corpus found; skipping");
        return;
    };

    let files = corpus_r_files(&root);
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_r::LANGUAGE.into())
        .expect("load tree-sitter-r grammar");

    let mut both_clean = 0usize;
    let mut both_error = 0usize;
    let mut ours_only: Vec<(PathBuf, String)> = Vec::new();
    let mut ts_only: Vec<PathBuf> = Vec::new();
    let mut lossy = 0usize;

    for path in &files {
        let (text, was_lossy) = read_lossy(path);
        if was_lossy {
            lossy += 1;
        }
        let parse = syntax::parse(&text);
        let ours_error = !parse.errors().is_empty();
        // A `None` tree means tree-sitter could not parse at all — treat as error.
        let ts_error =
            parser.parse(text.as_str(), None).is_none_or(|tree| tree.root_node().has_error());

        match (ours_error, ts_error) {
            (false, false) => both_clean += 1,
            (true, true) => both_error += 1,
            (true, false) => {
                let message = parse.errors().first().map(|error| error.message.clone());
                ours_only.push((path.clone(), message.unwrap_or_default()));
            }
            (false, true) => ts_only.push(path.clone()),
        }
    }

    eprintln!("corpus_acceptance: {} .R files ({lossy} lossy)", files.len());
    eprintln!("  both-clean:      {both_clean}");
    eprintln!("  both-error:      {both_error}");
    eprintln!(
        "  ours-only-error: {}  (BAD — we reject what tree-sitter accepts)",
        ours_only.len()
    );
    eprintln!(
        "  ts-only-error:   {}  (interesting — tree-sitter rejects what we accept)",
        ts_only.len()
    );

    eprintln!("--- ours-only-error (up to 30) ---");
    for (path, message) in ours_only.iter().take(30) {
        eprintln!("  BAD {}: {message}", path.display());
    }
    eprintln!("--- ts-only-error (up to 30) ---");
    for path in ts_only.iter().take(30) {
        eprintln!("  {}", path.display());
    }

    assert!(
        !files.is_empty(),
        "corpus present but no .R files found under r-base/ or cran/"
    );
    // HARD GATE: zero acceptance divergence is the measured baseline over the
    // full corpus. Rejecting a file tree-sitter accepts means a real grammar
    // gap; a legitimate divergence (tree-sitter itself being wrong, R as
    // referee) earns an entry in tests/acceptance-allowlist.txt instead.
    let allowlist = allowlisted_paths();
    let unexplained: Vec<_> = ours_only
        .iter()
        .filter(|(path, _)| {
            !allowlist.iter().any(|entry| path.to_string_lossy().ends_with(entry.as_str()))
        })
        .collect();
    assert!(
        unexplained.is_empty(),
        "{} unexplained acceptance divergence(s); fix the parser or adjudicate (R as referee) into tests/acceptance-allowlist.txt",
        unexplained.len()
    );
}

/// Phase 1 speed gate: ≥5× tree-sitter batch parse over the real corpus.
/// Run in release (`cargo test -p syntax --release --test test_corpus -- --ignored --nocapture`):
/// tree-sitter's C is compiled optimized even in debug profiles, so a debug
/// run would compare unoptimized Rust against optimized C.
#[test]
#[ignore = "perf gate; run explicitly in release"]
fn corpus_parse_speed() {
    let Some(root) = corpus_dir() else {
        eprintln!("corpus_parse_speed: corpus not fetched; skipping");
        return;
    };
    let mut files = Vec::new();
    for base in ["r-base", "cran"] {
        collect_r_files(&root.join(base), &mut files);
    }
    let sources: Vec<String> = files
        .iter()
        .filter_map(|path| std::fs::read(path).ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .collect();
    let total_bytes: usize = sources.iter().map(String::len).sum();
    let total_lines: usize = sources.iter().map(|s| s.lines().count()).sum();

    // Warm both parsers once.
    for source in sources.iter().take(50) {
        let _ = syntax::parse(source);
    }
    let ours_start = std::time::Instant::now();
    for source in &sources {
        let parse = syntax::parse(source);
        std::hint::black_box(parse.green());
    }
    let ours = ours_start.elapsed();

    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_r::LANGUAGE.into()).expect("grammar loads");
    let ts_start = std::time::Instant::now();
    for source in &sources {
        let tree = parser.parse(source, None);
        std::hint::black_box(&tree);
    }
    let ts = ts_start.elapsed();

    let ratio = ts.as_secs_f64() / ours.as_secs_f64();
    let ours_us_per_loc = ours.as_secs_f64() * 1e6 / total_lines as f64;
    let ts_us_per_loc = ts.as_secs_f64() * 1e6 / total_lines as f64;
    eprintln!(
        "corpus_parse_speed: {} files, {:.1} MB, {} LoC",
        sources.len(),
        total_bytes as f64 / 1e6,
        total_lines
    );
    eprintln!("  ours:        {ours:?}  ({ours_us_per_loc:.2} µs/LoC)");
    eprintln!("  tree-sitter: {ts:?}  ({ts_us_per_loc:.2} µs/LoC)");
    eprintln!("  ratio:       {ratio:.2}x");
    assert!(
        ratio >= 5.0,
        "Phase 1 gate: expected ≥5× tree-sitter batch parse speed, measured {ratio:.2}x"
    );
}

/// Committed, adjudicated divergences (one corpus-relative path suffix per
/// line; `#` comments). Currently empty — the measured baseline is zero.
fn allowlisted_paths() -> Vec<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/acceptance-allowlist.txt");
    std::fs::read_to_string(path)
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Resolve the corpus root: `ROUGHLY_CORPUS_DIR` if set, else `<workspace>/corpus`
/// (this crate is `<workspace>/crates/syntax`). `None` when the directory is
/// absent, so the tests can skip.
fn corpus_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("ROUGHLY_CORPUS_DIR") {
        let path = PathBuf::from(dir);
        return path.is_dir().then_some(path);
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join("corpus");
    path.is_dir().then_some(path)
}

/// Every `.R` file under the parseable corpus subtrees (`r-base`, `cran`), sorted
/// for stable reporting. The tree-sitter grammar-test corpus is `.txt` and lives
/// elsewhere, so it is naturally excluded.
fn corpus_r_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for subtree in ["r-base", "cran"] {
        collect_r_files(&root.join(subtree), &mut files);
    }
    files.sort();
    files
}

fn collect_r_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_r_files(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "R") {
            out.push(path);
        }
    }
}

/// R sources are usually UTF-8 but some are latin1; decode lossily and report
/// whether any byte had to be replaced.
fn read_lossy(path: &Path) -> (String, bool) {
    let bytes =
        std::fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    match String::from_utf8_lossy(&bytes) {
        Cow::Borrowed(text) => (text.to_owned(), false),
        Cow::Owned(text) => (text, true),
    }
}
