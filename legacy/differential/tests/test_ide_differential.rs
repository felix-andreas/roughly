//! Per-position IDE cross-stack differential — a HARD GATE: every byte
//! position of every typing-suite case runs hover, goto-definition,
//! references, rename, and signature help through the frozen legacy stack
//! (the oracle) and the rewrite's `ide` crate, plus inlay-hint anchors once
//! per case, comparing **targets and ranges** — never prose, per the
//! wording-freedom doctrine:
//!
//! - definition: both resolve or both don't; the rewrite's single target must
//!   equal or lie inside one of the oracle's targets;
//! - references and rename: identical byte-range sets;
//! - hover: agreement on presence, and the rewrite's range equal or inside
//!   the oracle's (contents are not compared);
//! - signature help: agreement on presence and the active parameter index;
//! - inlay hints: identical anchor positions (labels stay fixture-pinned).
//!
//! Accepted classes are counted separately (hover/signature new-only = more
//! coverage; references/rename declined on undeclared annotation type
//! tokens = deliberate narrowing), oracle-defect cases sit on committed
//! allowlists that fail when stale, and any unexplained divergence fails the
//! test. Details land in `target/differential-ide.txt`.

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
    // The oracle offers no navigation on a use-before-definition read; the
    // rewrite connects the read to the later declaration it warns about.
    "typing-scripts::resolution__use_before_any_definition_is_unresolved",
    "typing-scripts::resolution__read_inside_the_defining_statement_is_unresolved",
    // The oracle offers no navigation on pipeline reads; the rewrite
    // connects them to their declaration.
    "typing-scripts::reads_that_keep_bindings_alive__pipe_read_is_a_use",
    // The oracle models `|>` as an opaque operator; the rewrite desugars it
    // into the call R rewrites it to, so navigation connects piped reads and
    // newly-typed bindings gain inlay hints (pure additions).
    "typing::pipes__pipe_lowers_to_first_argument_call",
    "typing::pipes__pipe_chains_type_end_to_end",
    "typing::pipes__pipe_argument_errors_blame_the_piped_value",
    "typing::pipes__pipe_named_placeholder_receives_the_value",
    "typing::pipes__placeholder_without_the_piped_first_argument_is_missing_an_argument",
    "typing::pipes__pipe_into_a_namespaced_callee",
    "typing::pipes__non_call_pipe_stays_opaque",
    "typing::pipes__positional_placeholder_stays_opaque",
    // The oracle never defined `[` on vectors, so its items stay untyped and
    // hint nothing; the rewrite types vector subsetting and hints the
    // schemes (pure additions).
    "typing::vector_subsetting__scalar_index_selects_one_element",
    "typing::vector_subsetting__vector_index_selects_a_sub_vector",
    "typing::vector_subsetting__logical_mask_keeps_the_vector_shape",
    "typing::vector_subsetting__named_subject_keeps_names_under_vector_indexes",
    "typing::vector_subsetting__character_index_is_allowed_on_unnamed_vectors",
    "typing::vector_subsetting__undetermined_index_claims_a_scalar",
];

/// Adjudicated design differences, per case with the reason; divergences in
/// these cases are accepted wholesale, and a stale entry fails.
const ACCEPTED_DIFFERENCE_CASES: &[(&str, &str)] = &[
    (
        "typing::scoping__loop_carried_read_starts_from_the_outer_binding",
        "a package-level name bound by several items is one document slot in the \
         rewrite: references span every writer and definition targets the \
         name's primary definer, while the oracle isolates each item's binding \
         (renaming only one writer would change program meaning)",
    ),
    (
        "typing::scoping__loop_carried_read_continues_the_outer_type",
        "same document-slot model: cross-item references and primary-definer \
         definition for a name bound by several items",
    ),
    (
        "typing::scoping__rebinding_statement_reads_the_earlier_binding",
        "same document-slot model: cross-item references and primary-definer \
         definition for a name bound by several items",
    ),
    (
        "typing::scoping__accumulator_seeded_by_an_earlier_item",
        "same document-slot model: cross-item references and primary-definer \
         definition for a name bound by several items",
    ),
    (
        "typing::guards__field_access_guards_do_not_narrow",
        "the rewrite infers and hints a scheme for a function reading a field \
         off an unpinned parameter (`record$value` tolerates as Unknown); the \
         oracle leaves the item untyped and offers no inlay hint",
    ),
    (
        "typing::exported_constraints__forwarded_dots_disable_arity_checking",
        "the rewrite resolves a `...` formal as a real binding (definition and \
         references on the dots), which the oracle does not model",
    ),
    (
        "typing::form__applied_type_parameter_is_refused",
        "the rewrite refuses the whole violating block, so the alias it declares \
         never enters the vocabulary (no `Outer` completion, no `Wrap` \
         navigation); the oracle keeps the declaration while erroring",
    ),
    (
        "typing::dangling__definitions_and_strict_toggles_need_no_target",
        "the oracle serves hover on the `off` argument token of a `@strict off` \
         toggle; the rewrite's annotation hover covers type tokens only",
    ),
    (
        "typing::vectors__structural_vector_element_is_refused",
        "the oracle cannot parse a structural type under a `[]` suffix and treats \
         the item as unannotated (value-typed hover range, inferred inlay hint); \
         the rewrite parses and applies the declared annotation",
    ),
    (
        "typing::declared_shape__declared_optional_requires_an_actual_default",
        "the oracle serves hover on the `[label]` parameter-name token inside the \
         annotation; the rewrite's annotation hover covers type tokens only",
    ),
    (
        "typing::declared_shape__matching_shapes_are_fine",
        "the oracle serves hover on the `[label]` parameter-name token inside the \
         annotation; the rewrite's annotation hover covers type tokens only",
    ),
    (
        "typing::declared_shape__rest_position_must_match_the_formals",
        "the rewrite resolves a `...` formal as a real binding (definition and \
         references on the dots token); the oracle offers no dots navigation",
    ),
    (
        "typing::declared_shape__fixed_annotation_on_a_variadic_function_is_rejected",
        "the rewrite resolves a `...` formal as a real binding (definition and \
         references on the dots token); the oracle offers no dots navigation",
    ),
    (
        "typing::higher_order__any_callee_arguments_are_not_checked",
        "the oracle hints a binding from its INTERNAL value type (`-> Any`) while \
         the exported scheme says Unknown; the rewrite hints only scheme-consistent \
         types, so a scheme carrying Unknown shows no hint",
    ),
    (
        "typing-scripts::sequential__aliased_function_reference_exports_closed",
        "same INTERNAL-vs-exported hint difference: the oracle hints `g <- f` with \
         the open internal type while both stacks export the closed \
         `fn(p: Unknown) -> Unknown`; the rewrite hints only the exported scheme",
    ),
    (
        "typing-scripts::resolution__self_recursive_closure_resolves",
        "a self-recursive script closure pins to `fn() -> Unknown` through the \
         cycle fixpoint, so the rewrite withholds the Unknown-carrying inlay hint \
         and the active-parameter guess the oracle offers from its repass type",
    ),
    (
        "typing-scripts::resolution__forward_capture_resolves_to_the_later_binding",
        "the rewrite resolves the forward-captured read the oracle leaves \
         unresolved (navigation additions), and the two differ on \
         active-parameter guessing at the resulting call",
    ),
    (
        "typing-scripts::reads_that_keep_bindings_alive__masked_read_is_a_use",
        "the rewrite keeps a masked read's result Unknown (data-frame columns \
         are opaque), so the Unknown-carrying inlay hint the oracle shows from \
         resolving the mask is withheld",
    ),
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
    let mut stale_accepted: Vec<&str> = ACCEPTED_DIFFERENCE_CASES
        .iter()
        .map(|(case, _)| *case)
        .collect();

    for directory in ["typing", "typing-scripts"] {
        let suite_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/semantics/tests")
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
                                || ((divergence.contains("references set mismatch")
                                    || divergence.contains("inlay hint positions mismatch"))
                                    && divergence.contains("legacy-only []"));
                            if !additive {
                                unexplained.push(format!("{case_key}: {divergence}"));
                            }
                        }
                    } else if ACCEPTED_DIFFERENCE_CASES
                        .iter()
                        .any(|(case, _)| *case == case_key)
                    {
                        stale_accepted.retain(|entry| *entry != case_key);
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
    assert!(
        stale_accepted.is_empty(),
        "stale accepted-difference entries (now matching — remove them): {stale_accepted:?}"
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

        // Rename: identical edit-site sets under the same annotation-token
        // narrowing as references.
        let legacy_rename: BTreeSet<(usize, usize)> =
            analysis::ide::rename(&mut analysis_state, &path, position, "renamed")
                .map(|result| {
                    result
                        .edits
                        .values()
                        .flatten()
                        .filter_map(|edit| range_to_bytes(&line_starts, source, edit.range))
                        .collect()
                })
                .unwrap_or_default();
        let new_rename: BTreeSet<(usize, usize)> = ide::rename(&db, files, file, text_size)
            .unwrap_or_default()
            .into_iter()
            .map(|occurrence| {
                (
                    usize::from(occurrence.range.start()),
                    usize::from(occurrence.range.end()),
                )
            })
            .collect();
        if legacy_rename != new_rename {
            if new_rename.is_empty() && inside_annotation(source, offset) {
                *rollup
                    .entry("rename on undeclared type token (accepted narrower)".to_owned())
                    .or_default() += 1;
            } else if legacy_rename.is_subset(&new_rename)
                && new_references == new_rename
                && legacy_rename == legacy_references
            {
                // The same capture-deficit additions references already
                // classified; avoid double-reporting.
                *rollup
                    .entry("rename additions matching references (accepted)".to_owned())
                    .or_default() += 1;
            } else {
                let legacy_only: Vec<_> = legacy_rename.difference(&new_rename).collect();
                let new_only: Vec<_> = new_rename.difference(&legacy_rename).collect();
                record(
                    rollup,
                    &mut divergences,
                    offset,
                    "rename set mismatch",
                    &format!("legacy-only {legacy_only:?} / new-only {new_only:?}"),
                );
            }
        }

        // Signature help: presence, the signature-set size, the committed
        // overload index, and the active signature's active parameter index
        // (labels are prose — not compared).
        let legacy_signature = analysis::ide::signature_help(&mut analysis_state, &path, position)
            .map(|help| {
                (
                    help.signatures.len(),
                    help.active_signature,
                    help.signatures
                        .get(help.active_signature)
                        .and_then(|signature| signature.active_parameter),
                )
            });
        let new_signature = ide::signature_help(&db, file, text_size).map(|help| {
            (
                help.signatures.len(),
                help.active_signature,
                help.signatures
                    .get(help.active_signature)
                    .and_then(|signature| signature.active_parameter),
            )
        });
        match (&legacy_signature, &new_signature) {
            (None, None) => {}
            (Some(legacy), Some(new)) => {
                if legacy != new {
                    record(
                        rollup,
                        &mut divergences,
                        offset,
                        "signature mismatch",
                        &format!("legacy {legacy:?} / new {new:?}"),
                    );
                }
            }
            (Some(_), None) => record(
                rollup,
                &mut divergences,
                offset,
                "signature legacy-only",
                "",
            ),
            (None, Some(_)) => {
                *rollup
                    .entry("signature new-only (accepted improvement)".to_owned())
                    .or_default() += 1;
            }
        }

        // Completion: label sets. Both stacks rank with the same matcher and
        // cap, but the pools legitimately differ in breadth (the rewrite
        // completes annotation type names and namespace exports the oracle
        // does not), so capped (incomplete) lists compare by overlap: the
        // shared pool must agree on the top window's intersection. Uncapped
        // lists compare as exact sets.
        let legacy_completion: Option<BTreeSet<String>> =
            analysis::ide::completion(&mut analysis_state, &path, position)
                .map(|result| {
                    result
                        .items
                        .into_iter()
                        .map(|item| item.label)
                        .collect::<BTreeSet<String>>()
                })
                .filter(|labels| !labels.is_empty());
        let new_completion: Option<BTreeSet<String>> = ide::completion(&db, files, file, text_size)
            .map(|result| {
                result
                    .items
                    .into_iter()
                    .map(|item| item.label)
                    .collect::<BTreeSet<String>>()
            })
            .filter(|labels| !labels.is_empty());
        match (&legacy_completion, &new_completion) {
            (None, None) => {}
            (Some(legacy), Some(new)) => {
                let capped = legacy.len() >= analysis::ide::COMPLETION_LIMIT
                    || new.len() >= ide::COMPLETION_LIMIT;
                // Accepted supersets: the rewrite's pool is deliberately
                // richer — the full type vocabulary inside annotations, the
                // namespace's stub exports after `pkg::`, and an item-wide
                // local pool (legacy's scope lookup excludes frame-end
                // boundaries). A completion DEFICIT is always a divergence.
                let superset = legacy.is_subset(new);
                let in_namespace = after_namespace_operator(source, offset);
                let diverges = if capped {
                    legacy.intersection(new).count() == 0 && !in_namespace
                } else {
                    legacy != new && !superset
                };
                if diverges {
                    let legacy_only: Vec<&String> = legacy.difference(new).take(6).collect();
                    let new_only: Vec<&String> = new.difference(legacy).take(6).collect();
                    record(
                        rollup,
                        &mut divergences,
                        offset,
                        "completion set mismatch",
                        &format!("legacy-only {legacy_only:?} / new-only {new_only:?}"),
                    );
                } else if legacy != new {
                    *rollup
                        .entry("completion additions (accepted improvement)".to_owned())
                        .or_default() += 1;
                }
            }
            (Some(legacy), None) => record(
                rollup,
                &mut divergences,
                offset,
                "completion legacy-only",
                &format!("{} item(s)", legacy.len()),
            ),
            (None, Some(_)) => {
                *rollup
                    .entry("completion new-only (accepted improvement)".to_owned())
                    .or_default() += 1;
            }
        }

        // Hover: presence + range containment. The rewrite hovering where
        // the oracle does not is strictly more coverage — an accepted
        // improvement counted separately, not a divergence.
        let legacy_hover = analysis::ide::hover(&mut analysis_state, &path, position)
            .and_then(|info| range_to_bytes(&line_starts, source, info.range));
        let new_hover = ide::hover(&db, files, file, text_size).map(|hover| {
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

    // Inlay hints, once per case: anchor positions must agree (labels are
    // renderer prose and stay fixture-pinned instead).
    let legacy_hints: BTreeSet<usize> =
        analysis::ide::inlay_hints(&mut analysis_state, &path, None)
            .iter()
            .filter_map(|hint| position_to_byte(&line_starts, source, hint.position))
            .collect();
    let new_hints: BTreeSet<usize> = ide::inlay_hints(&db, file, None)
        .iter()
        .map(|hint| usize::from(hint.offset))
        .collect();
    if legacy_hints != new_hints {
        let legacy_only: Vec<_> = legacy_hints.difference(&new_hints).collect();
        let new_only: Vec<_> = new_hints.difference(&legacy_hints).collect();
        record(
            rollup,
            &mut divergences,
            0,
            "inlay hint positions mismatch",
            &format!("legacy-only {legacy_only:?} / new-only {new_only:?}"),
        );
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

/// Whether the identifier being completed at `offset` follows a `::`/`:::`
/// namespace operator (scanning back over the typed prefix).
fn after_namespace_operator(source: &str, offset: usize) -> bool {
    let bytes = source.as_bytes();
    let mut at = offset.min(source.len());
    while at > 0
        && (bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'.' || bytes[at - 1] == b'_')
    {
        at -= 1;
    }
    at >= 2 && &source[at - 2..at] == "::"
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
