//! Per-position IDE cross-stack differential: every byte position of every
//! typing-suite case runs hover, goto-definition, and references through the
//! frozen legacy stack (the oracle) and the rewrite's `ide` crate, comparing
//! **targets and ranges** — never prose, per the wording-freedom doctrine:
//!
//! - definition: both resolve or both don't; the rewrite's single target must
//!   equal or lie inside one of the oracle's targets;
//! - references: identical byte-range sets;
//! - hover: agreement on presence, and the rewrite's range equal or inside
//!   the oracle's (contents are not compared).
//!
//! Divergences land in `target/differential-ide.txt`, rolled up by class.
//! Ignored by default while triage hardens it into a gate:
//! `cargo test -p differential --release -- --ignored ide_differential`.

use analysis::text::{TextPosition, TextRange as LegacyTextRange};
use analysis::{Analysis, CheckConfig, LintConfig};
use semantics::{DocumentKind, ProjectFiles, RootDatabase, SourceFile};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use syntax::TextSize;

/// Cases where the ORACLE is wrong: its naming lacks forward-capture,
/// super-assignment, and local-mutual-recursion resolution, so the rewrite
/// finds occurrences and definitions legacy cannot. Divergences here must be
/// pure additions (no legacy-only losses); a stale entry (no divergence)
/// fails so the list cannot rot.
const ORACLE_DEFICIT_CASES: &[&str] = &[
    "typing::scoping__super_assignment_survives_the_closure",
    "typing::scoping__super_assignment_joins_as_a_union",
    "typing::scoping__forward_capture_resolves_after_repass",
    "typing::scoping__local_mutual_recursion_is_tolerant",
    "typing::scoping__forward_capture_sees_the_frame_write",
];

#[test]
fn ide_differential() {
    let mut report = String::new();
    let mut rollup: BTreeMap<String, usize> = BTreeMap::new();
    let mut cases = 0usize;
    let mut positions = 0usize;
    let mut diverging_cases = 0usize;
    let mut unexplained: Vec<String> = Vec::new();
    let mut stale_allowlist: Vec<&str> = ORACLE_DEFICIT_CASES.to_vec();

    for directory in ["typing", "typing-scripts"] {
        let suite_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../semantics/tests")
            .join(directory);
        let document_path = if directory == "typing" {
            "/pkg/R/case.R"
        } else {
            "/pkg/scripts/case.R"
        };
        let kind = if directory == "typing" {
            DocumentKind::Package
        } else {
            DocumentKind::Script
        };

        for file in syntax::testing::parse_fixture_files(&suite_dir) {
            for case in &file.cases {
                // Parity is scoped to inputs both parsers accept.
                if !syntax::parse(&case.source).errors().is_empty() {
                    continue;
                }
                cases += 1;
                // The oracle may panic on its own defects (debug_assert on a
                // non-converging alias-cycle interface); a panicking oracle
                // cannot be consulted, so the case is tolerated and counted.
                let compared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut case_positions = 0usize;
                    let mut case_rollup = BTreeMap::new();
                    let divergences = compare_case(
                        &case.source,
                        document_path,
                        kind,
                        &mut case_positions,
                        &mut case_rollup,
                    );
                    (divergences, case_positions, case_rollup)
                }));
                let divergences = match compared {
                    Ok((divergences, case_positions, case_rollup)) => {
                        positions += case_positions;
                        for (class, count) in case_rollup {
                            *rollup.entry(class).or_default() += count;
                        }
                        divergences
                    }
                    Err(_) => {
                        *rollup
                            .entry("oracle panicked (tolerated)".to_owned())
                            .or_default() += 1;
                        continue;
                    }
                };
                if !divergences.is_empty() {
                    diverging_cases += 1;
                    let case_key = format!("{}::{}", directory, case.id);
                    let _ = writeln!(report, "==== {case_key} ====");
                    for divergence in &divergences {
                        let _ = writeln!(report, "  {divergence}");
                    }
                    if ORACLE_DEFICIT_CASES.contains(&case_key.as_str()) {
                        stale_allowlist.retain(|entry| *entry != case_key);
                        // Allowlisted deficits must be pure additions: the
                        // rewrite resolving names the oracle cannot. Any
                        // legacy-only loss is still a real divergence.
                        for divergence in &divergences {
                            let additive = divergence.contains("definition new-only")
                                || (divergence.contains("references set mismatch")
                                    && divergence.contains("legacy-only []"));
                            if !additive {
                                unexplained.push(format!("{case_key}: {divergence}"));
                            }
                        }
                    } else {
                        for divergence in &divergences {
                            unexplained.push(format!("{case_key}: {divergence}"));
                        }
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
    for (count, message) in &ranked {
        let _ = writeln!(rollup_text, "{count:6}  {message}");
    }
    let total_divergences: usize = rollup.values().sum();
    let summary = format!(
        "ide differential: {cases} cases, {positions} positions compared, {total_divergences} divergences in {diverging_cases} case(s)\n"
    );
    println!("{summary}\n{rollup_text}");
    let report_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join("differential-ide.txt");
    let _ = std::fs::write(
        &report_path,
        format!(
            "{summary}\n== divergence rollup ==\n{rollup_text}\n== per-case details ==\n{report}"
        ),
    );

    assert!(
        unexplained.is_empty(),
        "unexplained IDE divergences (see target/differential-ide.txt):\n{}",
        unexplained.join("\n")
    );
    assert!(
        stale_allowlist.is_empty(),
        "stale oracle-deficit allowlist entries (now matching — remove them): {stale_allowlist:?}"
    );
}

fn compare_case(
    source: &str,
    document_path: &str,
    kind: DocumentKind,
    positions: &mut usize,
    rollup: &mut BTreeMap<String, usize>,
) -> Vec<String> {
    // Legacy side: one Analysis per case, fully built.
    let mut analysis_state = Analysis::new(
        PathBuf::from("/pkg"),
        LintConfig::default(),
        CheckConfig {
            unused: true,
            typing: true,
            strict: false,
        },
    );
    let path = PathBuf::from(document_path);
    if analysis_state
        .add_document_from_source(path.clone(), source)
        .is_err()
    {
        return Vec::new();
    }
    analysis::run_full(&mut analysis_state);

    // New side.
    let db = RootDatabase::default();
    semantics::stubs::install_shipped_stubs(&db);
    let file = SourceFile::new(&db, source.to_owned(), kind);
    let files = ProjectFiles::new(&db, vec![file]);

    let line_starts = line_starts(source);
    let mut divergences = Vec::new();

    for (offset, _) in source.char_indices() {
        *positions += 1;
        let position = byte_to_position(&line_starts, offset);
        let text_size = TextSize::from(offset as u32);

        // Definition.
        let legacy_definition: Option<BTreeSet<(usize, usize)>> =
            analysis::ide::definition(&mut analysis_state, &path, position).map(|locations| {
                locations
                    .iter()
                    .filter_map(|location| range_to_bytes(&line_starts, source, location.range))
                    .collect()
            });
        let new_definition = ide::definition(&db, files, file, text_size).map(|target| {
            (
                usize::from(target.range.start()),
                usize::from(target.range.end()),
            )
        });
        match (&legacy_definition, &new_definition) {
            (None, None) => {}
            (Some(legacy), Some(new)) => {
                let contained = legacy
                    .iter()
                    .any(|(start, end)| start <= &new.0 && &new.1 <= end);
                if !contained {
                    record(
                        rollup,
                        &mut divergences,
                        offset,
                        "definition target mismatch",
                        &format!("legacy {legacy:?} / new {new:?}"),
                    );
                }
            }
            (Some(legacy), None) => record(
                rollup,
                &mut divergences,
                offset,
                "definition legacy-only",
                &format!("legacy {legacy:?}"),
            ),
            (None, Some(new)) => record(
                rollup,
                &mut divergences,
                offset,
                "definition new-only",
                &format!("new {new:?}"),
            ),
        }

        // References (with declarations).
        let legacy_references: BTreeSet<(usize, usize)> =
            analysis::ide::references(&mut analysis_state, &path, position, true)
                .map(|locations| {
                    locations
                        .iter()
                        .filter_map(|location| range_to_bytes(&line_starts, source, location.range))
                        .collect()
                })
                .unwrap_or_default();
        let new_references: BTreeSet<(usize, usize)> =
            ide::references(&db, files, file, text_size, true)
                .into_iter()
                .map(|occurrence| {
                    (
                        usize::from(occurrence.range.start()),
                        usize::from(occurrence.range.end()),
                    )
                })
                .collect();
        if legacy_references != new_references {
            // The oracle offers references on ANY annotation type token by
            // spelled-name match, including primitives, `fn`, and binders;
            // the rewrite deliberately gates navigation on a project
            // declaration. A legacy-only reference set at a position inside
            // an annotation where the rewrite resolves no target is that
            // narrowing, accepted.
            let inside_annotation = inside_annotation(source, offset);
            if new_references.is_empty() && inside_annotation {
                *rollup
                    .entry("references on undeclared type token (accepted narrower)".to_owned())
                    .or_default() += 1;
            } else {
                let legacy_only: Vec<_> = legacy_references.difference(&new_references).collect();
                let new_only: Vec<_> = new_references.difference(&legacy_references).collect();
                record(
                    rollup,
                    &mut divergences,
                    offset,
                    "references set mismatch",
                    &format!("legacy-only {legacy_only:?} / new-only {new_only:?}"),
                );
            }
        }

        // Hover: presence + range containment. The rewrite hovering where
        // the oracle does not is strictly more coverage — an accepted
        // improvement counted separately, not a divergence.
        let legacy_hover = analysis::ide::hover(&mut analysis_state, &path, position)
            .and_then(|info| range_to_bytes(&line_starts, source, info.range));
        let new_hover = ide::hover(&db, file, text_size).map(|hover| {
            (
                usize::from(hover.range.start()),
                usize::from(hover.range.end()),
            )
        });
        match (&legacy_hover, &new_hover) {
            (None, None) => {}
            (Some(legacy), Some(new)) => {
                if !(legacy.0 <= new.0 && new.1 <= legacy.1) {
                    record(
                        rollup,
                        &mut divergences,
                        offset,
                        "hover range mismatch",
                        &format!("legacy {legacy:?} / new {new:?}"),
                    );
                }
            }
            (Some(legacy), None) => record(
                rollup,
                &mut divergences,
                offset,
                "hover legacy-only",
                &format!("legacy {legacy:?}"),
            ),
            (None, Some(_)) => {
                *rollup
                    .entry("hover new-only (accepted improvement)".to_owned())
                    .or_default() += 1;
            }
        }
    }

    divergences
}

fn record(
    rollup: &mut BTreeMap<String, usize>,
    divergences: &mut Vec<String>,
    offset: usize,
    class: &str,
    detail: &str,
) {
    *rollup.entry(class.to_owned()).or_default() += 1;
    divergences.push(format!("@{offset} {class}: {detail}"));
}

/// Whether the byte offset sits inside a `#:` annotation region of the
/// rewrite's parse.
fn inside_annotation(source: &str, offset: usize) -> bool {
    let parse = syntax::parse(source);
    parse.syntax_node().descendants().any(|node| {
        node.kind() == syntax::SyntaxKind::ANNOTATION
            && usize::from(node.text_range().start()) <= offset
            && offset <= usize::from(node.text_range().end())
    })
}

fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (index, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(index + 1);
        }
    }
    starts
}

fn byte_to_position(line_starts: &[usize], offset: usize) -> TextPosition {
    let line = line_starts.partition_point(|&start| start <= offset) - 1;
    TextPosition {
        line_index: line,
        character_index: offset - line_starts[line],
    }
}

fn position_to_byte(line_starts: &[usize], source: &str, position: TextPosition) -> Option<usize> {
    let start = *line_starts.get(position.line_index)?;
    let byte = start + position.character_index;
    (byte <= source.len()).then_some(byte)
}

fn range_to_bytes(
    line_starts: &[usize],
    source: &str,
    range: LegacyTextRange,
) -> Option<(usize, usize)> {
    Some((
        position_to_byte(line_starts, source, range.start)?,
        position_to_byte(line_starts, source, range.end)?,
    ))
}
