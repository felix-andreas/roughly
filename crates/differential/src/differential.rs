//! The cross-stack differential: the legacy analysis pipeline is the frozen
//! oracle for semantic diagnostics while the greenfield stack is built. The
//! parity scope is the semantic diagnostic classes (`type`, `annotation`,
//! `unresolved`, `unused`, `strict`) — the syntax class is deliberately
//! excluded, because the new parser's errors must be better than the
//! oracle's, not identical.
//!
//! This library is the one comparison harness every differential arm shares
//! (the fixture suites, the corpus sweep, and the fuzz arm live in this
//! crate's tests): how findings are extracted from each stack, when two
//! findings match, and which oracle deficits are accepted.

use analysis::{Analysis, CheckConfig, DiagnosticCode, LintConfig};
use semantics::diagnostics::{file_diagnostics, strict_diagnostics};
use semantics::{
    DocumentKind, ProjectFiles, RootDatabase, SourceFile, TypingMode, file_typing_mode,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// One comparable finding: (class, start byte, end byte, message).
pub type Finding = (&'static str, usize, usize, String);

/// The legacy oracle's findings for one document, plus whether the document
/// had any syntax error (arms that scope parity to cleanly parsed input skip
/// such documents rather than compare them). Legacy classifies documents by
/// path — only files under `R/` are package members — so the path stands in
/// for the document kind.
pub fn legacy_findings(source: &str, document_path: &str, strict: bool) -> (Vec<Finding>, bool) {
    let mut analysis_state = Analysis::new(
        PathBuf::from("/pkg"),
        LintConfig::default(),
        CheckConfig {
            unused: true,
            typing: true,
            strict,
        },
    );
    let Ok(document_id) =
        analysis_state.add_document_from_source(PathBuf::from(document_path), source)
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

/// The new stack's findings for one document. Publication mirrors the legacy
/// host edge: type errors and strict findings honor the per-file typing
/// directive (`# typing: ...` / `#: @strict`) over the configured default,
/// annotation and naming findings are always published.
pub fn new_findings(source: &str, kind: DocumentKind, strict_default: bool) -> Vec<Finding> {
    let db = RootDatabase::default();
    semantics::stubs::install_shipped_stubs(&db);
    let file = SourceFile::new(&db, source.to_owned(), kind);
    ProjectFiles::new(&db, vec![file]);
    let (typing_enabled, strict_enabled) = match file_typing_mode(&db, file) {
        Some(TypingMode::Off) => (false, false),
        Some(TypingMode::On) => (true, false),
        Some(TypingMode::Strict) => (true, true),
        None => (true, strict_default),
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
/// class whose range is equal or contained, with no new findings left over.
/// Message text is NOT compared — wording is free to improve on the oracle's
/// (user directive); the oracle pins which findings exist and where. Pairs
/// whose messages differ land in `wording` for informational review. Exact
/// (class, range, message) pairs match first so a wording difference never
/// steals a partner from an exact counterpart.
pub fn findings_match(
    legacy: &[Finding],
    new: &[Finding],
    wording: &mut Vec<(String, String)>,
) -> bool {
    if legacy.len() != new.len() {
        return false;
    }
    let mut used = vec![false; new.len()];
    let mut unpaired = Vec::new();
    'exact: for (legacy_index, (class, start, end, message)) in legacy.iter().enumerate() {
        for (index, (new_class, new_start, new_end, new_message)) in new.iter().enumerate() {
            let contained = new_start >= start && new_end <= end;
            if !used[index] && new_class == class && new_message == message && contained {
                used[index] = true;
                continue 'exact;
            }
        }
        unpaired.push(legacy_index);
    }
    'relaxed: for legacy_index in unpaired {
        let (class, start, end, message) = &legacy[legacy_index];
        for (index, (new_class, new_start, new_end, new_message)) in new.iter().enumerate() {
            let contained = new_start >= start && new_end <= end;
            if !used[index] && new_class == class && contained {
                used[index] = true;
                wording.push((message.clone(), new_message.clone()));
                continue 'relaxed;
            }
        }
        return false;
    }
    true
}

/// New-stack facts about one document that the deficit filters consult:
/// item spans, the earliest item creating each top-level variable slot, the
/// names read in quiet contexts (data masking, opaque operators), and the
/// per-item read/define sets behind the unstable-name closure.
pub struct NewStackFacts {
    item_spans: Vec<(usize, usize)>,
    earliest_slot: std::collections::HashMap<String, usize>,
    quiet_read_names: std::collections::BTreeSet<String>,
    /// Cross-item reads per item (unresolved-within-item names, quiet ones
    /// included).
    item_reads: Vec<std::collections::BTreeSet<String>>,
    /// Top-level binding names per item.
    item_defines: Vec<std::collections::BTreeSet<String>>,
    /// Names bound by more than one item.
    multibound: std::collections::BTreeSet<String>,
    /// Names whose defining item reads the name itself.
    self_referential: std::collections::BTreeSet<String>,
}

pub fn new_stack_facts(source: &str, kind: DocumentKind) -> NewStackFacts {
    let db = RootDatabase::default();
    semantics::stubs::install_shipped_stubs(&db);
    let file = SourceFile::new(&db, source.to_owned(), kind);
    ProjectFiles::new(&db, vec![file]);
    let item_spans: Vec<(usize, usize)> = semantics::item_spans(&db, file)
        .iter()
        .map(|span| {
            (
                u32::from(span.range.start()) as usize,
                u32::from(span.range.end()) as usize,
            )
        })
        .collect();
    let mut earliest_slot: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut quiet_read_names = std::collections::BTreeSet::new();
    let mut item_reads = Vec::new();
    let mut item_defines = Vec::new();
    let mut binding_items: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut self_referential = std::collections::BTreeSet::new();
    for (index, item) in semantics::item_tree(&db, file).into_iter().enumerate() {
        let mut reads = std::collections::BTreeSet::new();
        let mut defines = std::collections::BTreeSet::new();
        if let Some(naming) = semantics::item_naming(&db, item) {
            for binding in naming.bindings.values() {
                if binding.kind == semantics::naming::BindingKind::TopLevel {
                    earliest_slot.entry(binding.name.clone()).or_insert(index);
                    defines.insert(binding.name.clone());
                }
            }
            reads.extend(naming.non_locals.values().cloned());
            reads.extend(naming.quiet_reads.values().cloned());
            quiet_read_names.extend(naming.quiet_reads.values().cloned());
        }
        for name in &defines {
            *binding_items.entry(name.clone()).or_default() += 1;
            if reads.contains(name) {
                self_referential.insert(name.clone());
            }
        }
        item_reads.push(reads);
        item_defines.push(defines);
    }
    let multibound = binding_items
        .into_iter()
        .filter(|(_, count)| *count >= 2)
        .map(|(name, _)| name)
        .collect();
    NewStackFacts {
        item_spans,
        earliest_slot,
        quiet_read_names,
        item_reads,
        item_defines,
        multibound,
        self_referential,
    }
}

/// The oracle deficits one comparison accepted: names whose unresolved
/// findings were forward-capture deficits, and the ranges of legacy type
/// findings dropped under the Unknown-tolerance rule.
#[derive(Default)]
pub struct AcceptedDeficits {
    pub names: Vec<String>,
    pub unknown_tolerance_ranges: Vec<(usize, usize)>,
}

/// Accepts unpaired TYPE findings on either side that sit in the territory
/// where the two stacks' resolution models legitimately diverge, so the fuzz
/// arm neither reproduces oracle bugs nor fails on intended rewrite
/// precision. The territory is the closure of the *unstable names*:
///
/// - names bound by more than one item — the oracle types every read from
///   one settled slot (its frame model has no sequence, and its occurs-check
///   can even reject valid self-referential rebindings outright), while the
///   rewrite gives immediate reads the nearest earlier binding and deferred
///   reads the last one;
/// - self-referential definitions — the oracle pins them to `Unknown`, the
///   rewrite types them through annotations or the cycle fixpoint;
/// - names whose unresolved findings were accepted as forward-capture
///   deficits — the oracle leaves those reads `Unknown`, so every downstream
///   type check silently passes where the rewrite, having resolved the
///   capture, checks for real;
/// - names defined by an item whose legacy finding was accepted under the
///   Unknown-tolerance rule — the oracle failed that item and exports
///   `Unknown` for its binding, while the rewrite exports the declared
///   contract, so downstream reads check for real only on the rewrite side;
///
/// closed transitively over "the name's defining item reads an unstable
/// name". A type finding (either side) with no intersecting counterpart in
/// the other stack is accepted when its item reads or defines an unstable
/// name. Additionally, a NEW-only type finding whose found-side mentions
/// `NULL` is accepted: the rewrite types `switch`/branch results as unions
/// with `NULL` where the oracle collapses to `Unknown` first, so its
/// NULL-union strictness has no oracle counterpart by construction. Every
/// acceptance is counted in `deficit_rollup`.
pub fn filter_model_divergences(
    legacy: Vec<Finding>,
    new: Vec<Finding>,
    facts: &NewStackFacts,
    accepted: &AcceptedDeficits,
    deficit_rollup: &mut BTreeMap<String, usize>,
) -> (Vec<Finding>, Vec<Finding>) {
    let mut unstable: std::collections::BTreeSet<String> = facts
        .multibound
        .iter()
        .chain(facts.self_referential.iter())
        .chain(accepted.names.iter())
        .cloned()
        .collect();
    for &(start, end) in &accepted.unknown_tolerance_ranges {
        if let Some(index) = facts
            .item_spans
            .iter()
            .position(|&(item_start, item_end)| item_start <= start && end <= item_end)
        {
            unstable.extend(facts.item_defines[index].iter().cloned());
        }
    }
    loop {
        let mut grew = false;
        for (index, defines) in facts.item_defines.iter().enumerate() {
            if facts.item_reads[index]
                .iter()
                .any(|name| unstable.contains(name))
            {
                for name in defines {
                    grew |= unstable.insert(name.clone());
                }
            }
        }
        if !grew {
            break;
        }
    }
    let item_is_unstable = |finding: &Finding| {
        facts
            .item_spans
            .iter()
            .position(|&(start, end)| start <= finding.1 && finding.2 <= end)
            .is_some_and(|index| {
                facts.item_reads[index]
                    .iter()
                    .chain(facts.item_defines[index].iter())
                    .any(|name| unstable.contains(name))
            })
    };
    let intersects = |finding: &Finding, other: &[Finding]| {
        other.iter().any(|(class, start, end, _)| {
            *class == "type" && *start < finding.2 && finding.1 < *end
        })
    };
    let legacy_kept: Vec<Finding> = legacy
        .iter()
        .filter(|finding| {
            if finding.0 == "type" && !intersects(finding, &new) && item_is_unstable(finding) {
                *deficit_rollup
                    .entry("(settled-slot model divergence, oracle side)".to_owned())
                    .or_default() += 1;
                return false;
            }
            true
        })
        .cloned()
        .collect();
    let new_kept: Vec<Finding> = new
        .iter()
        .filter(|finding| {
            if finding.0 != "type" || intersects(finding, &legacy) {
                return true;
            }
            if item_is_unstable(finding) {
                *deficit_rollup
                    .entry("(settled-slot model divergence, rewrite side)".to_owned())
                    .or_default() += 1;
                return false;
            }
            let null_union = finding
                .3
                .rsplit_once("found")
                .map(|(_, found)| found)
                .or_else(|| finding.3.rsplit_once("has type").map(|(_, found)| found))
                .is_some_and(|found| found.contains("NULL"));
            if null_union {
                *deficit_rollup
                    .entry("(NULL-union strictness)".to_owned())
                    .or_default() += 1;
                return false;
            }
            true
        })
        .cloned()
        .collect();
    (legacy_kept, new_kept)
}

/// Drops legacy findings that fall into an accepted oracle-deficit class,
/// counting each accepted drop in `deficit_rollup` so an accidental
/// resolution bug in the rewrite stays visible for review:
///
/// - The oracle's naming lacks forward-capture resolution (locals written
///   later in a frame that earlier-defined closures read), so it emits
///   unresolved warnings the rewrite correctly resolves. A legacy-only
///   unresolved finding at a site where the new side reports nothing for the
///   same name is accepted; a new unresolved finding for the same name
///   elsewhere still has to match exactly.
/// - The same deficit makes the oracle call the forward-captured binding
///   dead: it emits an unused warning for the very name whose captured read
///   it failed to resolve. A legacy-only unused finding is accepted only
///   when its name was accepted under the unresolved rule above AND the new
///   side considers the binding read — tying the acceptance to the
///   established forward-capture shape instead of excusing any missing
///   unused finding.
/// - The oracle does not model reads inside `|>` pipelines (and other opaque
///   operators) as uses, so it calls bindings dead that a pipeline reads. A
///   legacy-only unused finding whose name the new side saw in a quiet read
///   is that deficit (requires `facts`).
/// - The rewrite's tolerance floor treats `Unknown` as an absent fact (not a
///   checkable claim), while the oracle sometimes rejects Unknown-carrying
///   values (`list{Unknown}` against `list[T] | T[]`). A legacy-only type
///   finding whose found-side mentions Unknown, with no new type finding
///   inside its range, is that floor.
///
/// Returns the filtered legacy findings plus the names whose unresolved
/// findings were accepted as forward-capture deficits and the ranges of
/// accepted Unknown-tolerance drops (consumed by
/// [`filter_model_divergences`]'s unstable-name closure).
pub fn filter_oracle_deficits(
    legacy: Vec<Finding>,
    new: &[Finding],
    facts: Option<&NewStackFacts>,
    deficit_rollup: &mut BTreeMap<String, usize>,
) -> (Vec<Finding>, AcceptedDeficits) {
    let mut accepted = AcceptedDeficits::default();
    let kept: Vec<Finding> = legacy
        .into_iter()
        .filter(|finding| {
            if finding.0 == "unresolved" {
                let Some(name) = unresolved_name(&finding.3) else {
                    return true;
                };
                let needle = format!("`{name}`");
                let resolved_by_new = !new.iter().any(|(class, start, end, message)| {
                    *class == "unresolved"
                        && message.contains(&needle)
                        && *start < finding.2
                        && finding.1 < *end
                });
                if resolved_by_new {
                    *deficit_rollup.entry(name.to_owned()).or_default() += 1;
                    accepted.names.push(name.to_owned());
                }
                return !resolved_by_new;
            }
            // Either side mentioning `Unknown` marks the oracle checking a
            // non-fact: rejecting an Unknown-carrying value, or checking a
            // value against a declared `Unknown`.
            if finding.0 == "type" && finding.3.contains("Unknown") {
                let new_has_counterpart = new.iter().any(|(class, start, end, _)| {
                    *class == "type" && *start >= finding.1 && *end <= finding.2
                });
                if !new_has_counterpart {
                    *deficit_rollup
                        .entry("(Unknown-tolerant type check)".to_owned())
                        .or_default() += 1;
                    accepted
                        .unknown_tolerance_ranges
                        .push((finding.1, finding.2));
                    return false;
                }
            }
            true
        })
        .collect();
    let filtered: Vec<Finding> = kept
        .into_iter()
        .filter(|finding| {
            if finding.0 != "unused" {
                return true;
            }
            let Some(name) = unused_name(&finding.3) else {
                return true;
            };
            let read_by_new = !new.iter().any(|(class, start, end, message)| {
                *class == "unused"
                    && unused_name(message) == Some(name)
                    && *start < finding.2
                    && finding.1 < *end
            });
            if !read_by_new {
                return true;
            }
            let forward_captured = accepted.names.iter().any(|accepted| accepted == name);
            if forward_captured {
                *deficit_rollup
                    .entry(format!("{name} (unused)"))
                    .or_default() += 1;
                return false;
            }
            let quiet_read = facts.is_some_and(|facts| facts.quiet_read_names.contains(name));
            if quiet_read {
                *deficit_rollup
                    .entry(format!("{name} (quiet read)"))
                    .or_default() += 1;
                return false;
            }
            true
        })
        .collect();
    (filtered, accepted)
}

/// Accepts the rewrite's unknown-type-name diagnostics: legacy models the
/// same finding only through its naming class, which the differential
/// excludes by construction, so a NEW-only annotation finding of this shape
/// can never pair. Each accepted drop is counted in `deficit_rollup`.
pub fn filter_unknown_type_additions(
    new: Vec<Finding>,
    deficit_rollup: &mut BTreeMap<String, usize>,
) -> Vec<Finding> {
    new.into_iter()
        .filter(|finding| {
            if finding.0 != "annotation" || !finding.3.starts_with("I do not know the type") {
                return true;
            }
            *deficit_rollup
                .entry("(unknown-type diagnostic, rewrite side)".to_owned())
                .or_default() += 1;
            false
        })
        .collect()
}

/// Accepts the oracle's within-statement slot tolerance: the legacy frame
/// model mints a statement's target slot before resolving its right-hand
/// side, so an immediate read of a name inside its own defining statement
/// (`x <- x + 1L`, `x <- local({ t <- x; t })` with no earlier `x`) often
/// resolves silently there, while the rewrite reports it — the value
/// executes before the write completes, so the read is a real runtime
/// error. A NEW-only unresolved finding whose read lies inside the very
/// item that earliest-binds the name is that tolerance; each accepted drop
/// is counted in `deficit_rollup`.
pub fn filter_legacy_slot_tolerance(
    new: Vec<Finding>,
    legacy: &[Finding],
    facts: &NewStackFacts,
    deficit_rollup: &mut BTreeMap<String, usize>,
) -> Vec<Finding> {
    new.into_iter()
        .filter(|finding| {
            if finding.0 != "unresolved" {
                return true;
            }
            let Some(name) = unresolved_name(&finding.3) else {
                return true;
            };
            let Some(&earliest) = facts.earliest_slot.get(name) else {
                return true;
            };
            let in_defining_item = facts
                .item_spans
                .get(earliest)
                .is_some_and(|&(start, end)| start <= finding.1 && finding.2 <= end);
            if !in_defining_item {
                return true;
            }
            let needle = format!("`{name}`");
            let legacy_reports_it = legacy.iter().any(|(class, start, end, message)| {
                *class == "unresolved"
                    && message.contains(&needle)
                    && *start < finding.2
                    && finding.1 < *end
            });
            if legacy_reports_it {
                return true;
            }
            *deficit_rollup
                .entry(format!("{name} (in-statement read)"))
                .or_default() += 1;
            false
        })
        .collect()
}

/// The name inside the first backtick pair of an unresolved message.
fn unresolved_name(message: &str) -> Option<&str> {
    let start = message.find('`')? + 1;
    let rest = &message[start..];
    let end = rest.rfind("` in this package")?;
    Some(&rest[..end])
}

/// The name in an "assigned but never used" message (both stacks share the
/// wording).
fn unused_name(message: &str) -> Option<&str> {
    message
        .strip_prefix('`')?
        .strip_suffix("` is assigned but never used.")
}
