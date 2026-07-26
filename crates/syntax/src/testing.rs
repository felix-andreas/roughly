//! Fixture harness for the syntax crate.
//!
//! The file format is the project-wide fixture format: `#==== <group>` opens a
//! group, `#---- <case>` opens a case, the case source runs until a `#++++`
//! line, and the expectation body runs until the next directive line (or end of
//! file). Case identity is `group__case` and must be unique across a whole
//! suite. `FIXTURE_FILTER=group__case` runs one case; `RY_BLESS=1`
//! rewrites expectation bodies in place from the rendered output.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::ops::Range;
use std::path::{Path, PathBuf};

pub struct FixtureCase {
    pub id: String,
    pub source: String,
    pub expected: String,
    /// Byte range of the expectation body inside the fixture file.
    expected_span: Range<usize>,
}

pub struct FixtureFile {
    pub path: PathBuf,
    pub text: String,
    pub cases: Vec<FixtureCase>,
}

/// Parse every `.test` fixture file under `suite_dir` (recursively), without
/// running anything — for harnesses that reuse the corpus, like the
/// cross-stack differential.
pub fn parse_fixture_files(suite_dir: &Path) -> Vec<FixtureFile> {
    let mut paths = Vec::new();
    collect_fixture_files(suite_dir, &mut paths);
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read fixture {}: {error}", path.display()));
            parse_fixture_file(&path, text)
        })
        .collect()
}

/// Run every `.test` fixture under `suite_dir` through `render`, comparing (or
/// blessing) expectations. Panics with a readable report on mismatch.
/// Reads an environment variable under its `RY_` name, falling back to the
/// pre-rename `ROUGHLY_` spelling. Both are honoured so a developer's existing
/// shell aliases and CI scripts keep working after the rename.
pub fn env_var(suffix: &str) -> Option<String> {
    std::env::var(format!("RY_{suffix}"))
        .or_else(|_| std::env::var(format!("ROUGHLY_{suffix}")))
        .ok()
}

pub fn run_fixture_suite(suite_dir: &Path, render: &dyn Fn(&str) -> String) {
    let mut files = Vec::new();
    collect_fixture_files(suite_dir, &mut files);
    files.sort();
    assert!(
        !files.is_empty(),
        "no fixture files found under {}",
        suite_dir.display()
    );

    let filter = std::env::var("FIXTURE_FILTER").ok();
    let bless = env_var("BLESS").is_some_and(|value| value == "1");
    let mut seen_ids = HashSet::new();
    let mut failures = Vec::new();

    for path in files {
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        let file = parse_fixture_file(&path, text);
        for case in &file.cases {
            assert!(
                seen_ids.insert(case.id.clone()),
                "duplicate fixture id `{}` (fixture ids must be unique across the suite)",
                case.id
            );
        }

        let mut blessed_edits: Vec<(Range<usize>, String)> = Vec::new();
        for case in &file.cases {
            if filter.as_deref().is_some_and(|filter| filter != case.id) {
                continue;
            }
            let rendered = normalize(&render(&case.source));
            let expected = normalize(&case.expected);
            if rendered == expected {
                continue;
            }
            if bless {
                blessed_edits.push((case.expected_span.clone(), rendered));
            } else {
                let mut report = String::new();
                let _ = writeln!(report, "fixture `{}` ({})", case.id, file.path.display());
                let _ = writeln!(report, "---- expected ----\n{expected}");
                let _ = writeln!(report, "---- actual ----\n{rendered}");
                failures.push(report);
            }
        }

        if !blessed_edits.is_empty() {
            let mut new_text = file.text.clone();
            for (span, rendered) in blessed_edits.into_iter().rev() {
                let at_eof = span.end == new_text.len();
                let replacement = if at_eof {
                    format!("{rendered}\n")
                } else {
                    format!("{rendered}\n\n")
                };
                new_text.replace_range(span, &replacement);
            }
            std::fs::write(&file.path, new_text)
                .unwrap_or_else(|error| panic!("cannot bless {}: {error}", file.path.display()));
        }
    }

    assert!(
        failures.is_empty(),
        "{} fixture failure(s):\n\n{}\n(set RY_BLESS=1 to accept the new output)",
        failures.len(),
        failures.join("\n")
    );
}

fn collect_fixture_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("cannot read fixture dir {}: {error}", dir.display()));
    for entry in entries {
        let entry = entry.expect("readable directory entry");
        let path = entry.path();
        if path.is_dir() {
            collect_fixture_files(&path, out);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "test")
        {
            out.push(path);
        }
    }
}

/// Parse one fixture file. Public for harnesses that pre-filter which files
/// to consume (the legacy-corpus differential skips multi-document
/// composites the shared single-file format cannot express).
pub fn parse_fixture_file(path: &Path, text: String) -> FixtureFile {
    let mut cases = Vec::new();
    let mut group: Option<String> = None;
    let mut case: Option<String> = None;
    let mut source_lines: Vec<&str> = Vec::new();
    let mut expected_start: Option<usize> = None;
    let mut expected_end = 0usize;

    let mut offset = 0usize;
    let mut flush = |group: &Option<String>,
                     case: &mut Option<String>,
                     source_lines: &mut Vec<&str>,
                     expected_start: &mut Option<usize>,
                     expected_end: usize,
                     text: &str| {
        if let Some(case_name) = case.take() {
            let group_name = group.clone().unwrap_or_else(|| {
                panic!(
                    "{}: case `{case_name}` appears before any `#====` group",
                    path.display()
                )
            });
            let start = expected_start.take().unwrap_or_else(|| {
                panic!(
                    "{}: case `{case_name}` has no `#++++` expectation",
                    path.display()
                )
            });
            let source = source_lines.join("\n").trim_end().to_owned();
            source_lines.clear();
            cases.push(FixtureCase {
                id: format!("{group_name}__{case_name}"),
                source,
                expected: text[start..expected_end].to_owned(),
                expected_span: start..expected_end,
            });
        }
        source_lines.clear();
    };

    for line in text.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if let Some(name) = trimmed.strip_prefix("#====") {
            flush(
                &group,
                &mut case,
                &mut source_lines,
                &mut expected_start,
                expected_end,
                &text,
            );
            group = Some(name.trim().to_owned());
        } else if let Some(name) = trimmed.strip_prefix("#----") {
            flush(
                &group,
                &mut case,
                &mut source_lines,
                &mut expected_start,
                expected_end,
                &text,
            );
            case = Some(name.trim().to_owned());
        } else if trimmed.starts_with("#++++") {
            expected_start = Some(offset);
            expected_end = offset;
        } else if expected_start.is_some() && case.is_some() {
            expected_end = offset;
        } else if case.is_some() {
            source_lines.push(trimmed);
        }
        let _ = line_start;
    }
    flush(
        &group,
        &mut case,
        &mut source_lines,
        &mut expected_start,
        expected_end,
        &text,
    );

    FixtureFile {
        path: path.to_owned(),
        text,
        cases,
    }
}

/// Trailing-whitespace-insensitive comparison form.
fn normalize(text: &str) -> String {
    let mut normalized = text
        .lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n");
    while normalized.ends_with('\n') {
        normalized.pop();
    }
    normalized.trim_end().to_owned()
}

/// The full invariant battery for one input.
pub fn check_parse_invariants(input: &str) {
    let parse = crate::parse(input);
    let reprinted = parse.text();
    assert_eq!(
        reprinted, input,
        "lossless round-trip violated for input {input:?}"
    );

    let (tokens, _errors) = crate::lex(input);
    let total: u32 = tokens.iter().map(|token| u32::from(token.len)).sum();
    assert_eq!(
        total as usize,
        input.len(),
        "token cover violated for input {input:?}"
    );
    for token in &tokens {
        assert!(
            u32::from(token.len) > 0,
            "zero-length token for input {input:?}"
        );
    }

    check_geometry(&parse.syntax_node(), input);

    // Error-report quality: every diagnostic is renderable (non-empty
    // message, in-bounds range), and the report count stays linear in the
    // input — a cascade that fans one mistake into a storm is a bug even
    // when every individual message is well-formed.
    assert!(
        parse.errors().len() <= 2 * tokens.len() + 16,
        "error cascade: {} errors from {} tokens for input {input:?}",
        parse.errors().len(),
        tokens.len()
    );
    for error in parse.errors() {
        assert!(
            !error.message.is_empty(),
            "empty error message for input {input:?}"
        );
        assert!(
            u32::from(error.range.end()) as usize <= input.len(),
            "error range out of bounds for input {input:?}"
        );
    }

    // Determinism: a second parse yields a structurally equal tree and the
    // same errors.
    let again = crate::parse(input);
    assert_eq!(
        parse.green(),
        again.green(),
        "non-deterministic parse for input {input:?}"
    );
    assert_eq!(
        parse.errors(),
        again.errors(),
        "non-deterministic errors for input {input:?}"
    );
}

/// Every node's children (nodes and tokens) must tile its range exactly.
fn check_geometry(node: &crate::SyntaxNode, input: &str) {
    let range = node.text_range();
    let mut cursor = range.start();
    for child in node.children_with_tokens() {
        let child_range = child.text_range();
        assert_eq!(
            child_range.start(),
            cursor,
            "child ranges not contiguous under {:?} for input {input:?}",
            node.kind()
        );
        cursor = child_range.end();
        if let rowan::NodeOrToken::Node(child) = child {
            check_geometry(&child, input);
        }
    }
    if node.children_with_tokens().next().is_some() {
        assert_eq!(
            cursor,
            range.end(),
            "children do not cover {:?} for input {input:?}",
            node.kind()
        );
    }
}
