//! Formatter cross-stack comparison over the real-file corpus: every corpus
//! `.R` file is formatted by both stacks and the outputs are compared
//! byte-for-byte. Divergences are triage material — each gets reviewed and
//! either fixed in the new formatter or accepted as a deliberate improvement —
//! so the run reports rather than gates on them; only a new-stack panic fails
//! the sweep. The report (`target/differential-format.txt`) leads with a
//! frequency rollup keyed by the first divergent line pair, so one layout rule
//! repeated across hundreds of files reads as one line.
//!
//! Ignored by default — it needs the fetched corpus (`scripts/fetch-corpus.sh`):
//! `cargo test -p differential -- --ignored format_differential_corpus`.

use ropey::Rope;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[test]
#[ignore = "needs the fetched corpus; run explicitly with -- --ignored"]
fn format_differential_corpus() {
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

    let mut parser = roughly_legacy::tree::new_parser();
    let mut report = String::new();
    let mut rollup: BTreeMap<String, usize> = BTreeMap::new();
    let mut files = 0usize;
    let mut matching = 0usize;
    let mut refused_both = 0usize;
    let mut refused_legacy_only = 0usize;
    let mut refused_new_only = 0usize;
    let mut diverging = 0usize;
    let mut panicking: Vec<String> = Vec::new();

    for path in &paths {
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        let relative = path.strip_prefix(&corpus_root).unwrap_or(path);

        let tree = roughly_legacy::tree::parse(&mut parser, &source, None);
        let rope = Rope::from_str(&source);
        let legacy = roughly_legacy::format::format(
            tree.root_node(),
            &rope,
            roughly_legacy::format::Config::default(),
        );

        // A panic on one file (a bug to fix) must not kill the whole triage
        // run: record the file and keep sweeping.
        let new = std::panic::catch_unwind(|| format::format(&source, format::Config::default()));
        let Ok(new) = new else {
            panicking.push(relative.display().to_string());
            continue;
        };

        match (legacy, new) {
            (Err(_), Err(_)) => refused_both += 1,
            (Err(_), Ok(_)) => {
                // The stacks refuse on different grounds (tree-sitter
                // acceptance vs the new parser's R grammar; annotation-grammar
                // errors never refuse on the new side) — an asymmetry is
                // acceptance drift worth seeing, not an output divergence.
                refused_legacy_only += 1;
                let _ = writeln!(
                    report,
                    "==== {} ==== legacy refused, new formatted",
                    relative.display()
                );
            }
            (Ok(_), Err(error)) => {
                refused_new_only += 1;
                let _ = writeln!(
                    report,
                    "==== {} ==== new refused, legacy formatted: {error}",
                    relative.display()
                );
            }
            (Ok(legacy_output), Ok(new_output)) => {
                files += 1;
                if legacy_output == new_output {
                    matching += 1;
                    continue;
                }
                diverging += 1;
                let (line, legacy_line, new_line) =
                    first_divergent_line(&legacy_output, &new_output);
                *rollup
                    .entry(format!("legacy {legacy_line:?} / new {new_line:?}"))
                    .or_default() += 1;
                let _ = writeln!(
                    report,
                    "==== {} ==== first divergence at output line {line}\n  legacy: {legacy_line}\n  new:    {new_line}",
                    relative.display()
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
    for (count, message) in ranked.iter().take(80) {
        let _ = writeln!(rollup_text, "{count:6}  {message}");
    }
    let mut panic_text = String::new();
    for file in &panicking {
        let _ = writeln!(panic_text, "  PANIC: {file}");
    }
    let summary = format!(
        "format differential corpus: {matching}/{files} formatted files match, {diverging} diverging, {} panicking, {refused_both} refused by both, {refused_legacy_only} refused by legacy only, {refused_new_only} refused by new only\n",
        panicking.len()
    );
    println!("{summary}\n{panic_text}{rollup_text}");
    let report_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join("differential-format.txt");
    let _ = std::fs::write(
        &report_path,
        format!(
            "{summary}\n== panicking files ==\n{panic_text}\n== first-divergent-line rollup ==\n{rollup_text}\n== per-file details ==\n{report}"
        ),
    );
    assert!(
        panicking.is_empty(),
        "the new formatter panicked on {} corpus file(s):\n{panic_text}",
        panicking.len()
    );
}

/// The first line where the two outputs differ (1-based), with both lines.
fn first_divergent_line(legacy: &str, new: &str) -> (usize, String, String) {
    let mut legacy_lines = legacy.lines();
    let mut new_lines = new.lines();
    let mut line = 0usize;
    loop {
        line += 1;
        match (legacy_lines.next(), new_lines.next()) {
            (Some(a), Some(b)) if a == b => continue,
            (a, b) => {
                return (
                    line,
                    a.unwrap_or("<end of output>").to_owned(),
                    b.unwrap_or("<end of output>").to_owned(),
                );
            }
        }
    }
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
