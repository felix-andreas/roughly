//! The diagnostic edge: item-relative findings become file diagnostics with
//! absolute positions, and structured type errors become wording.
//!
//! One type-display policy, one renderer: inference variables and rigid
//! binders display as `T`/`U`/`V` in first-occurrence order, and one renderer
//! instance must span everything that shares names (both sides of an
//! expected/found message), because a fresh renderer restarts the numbering.

use crate::check::{OperandExpectation, TypeError, TypeErrorKind};
use crate::types::{Atomic, Constraint, FunctionType, Name, Ty, TyKind, TypeScheme};
use crate::{Db, DocumentKind, Item, SourceFile, item_check, item_tree, parse};
use syntax::TextRange;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, salsa::SalsaValue)]
pub enum Severity {
    Error,
    Warning,
}

/// One rendered finding, file-absolute.
#[derive(Debug, Clone, PartialEq, Eq, salsa::SalsaValue)]
pub struct Diagnostic {
    pub range: TextRange,
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    /// Companion locations for findings whose story spans sites (the sibling
    /// definition an overwrite warning refers to). Rendered as `note:` lines
    /// by the CLI and as related information over LSP.
    pub related: Vec<RelatedLocation>,
}

/// One companion location of a diagnostic, possibly in another file.
#[derive(Debug, Clone, PartialEq, Eq, salsa::SalsaValue)]
pub struct RelatedLocation {
    pub file: SourceFile,
    pub range: TextRange,
    pub message: &'static str,
}

/// The diagnostics that are pure functions of the parse — syntax errors,
/// typing-directive errors, and `#:` block-form refusals. A host's fast
/// publication wave serves exactly these (plus lints) so typing never waits
/// on naming or type checking; [`file_diagnostics`] starts from the same set,
/// keeping the fast wave a faithful subset of the settled one.
#[salsa::tracked(returns(clone))]
pub fn parse_stage_diagnostics(db: &dyn Db, file: SourceFile) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    let parsed = parse(db, file);
    for error in parsed.errors() {
        diagnostics.push(Diagnostic {
            range: error.range,
            severity: Severity::Error,
            code: "syntax-error",
            message: error.message.clone(),
            related: Vec::new(),
        });
    }

    for (range, value) in crate::file_typing_directives(db, file).1 {
        diagnostics.push(Diagnostic {
            range,
            severity: Severity::Error,
            code: "annotation",
            message: format!(
                "Unknown typing directive `{value}`. Use `# typing: on`, `# typing: off`, or `# typing: strict`."
            ),
            related: Vec::new(),
        });
    }

    for node in parsed
        .syntax_node()
        .descendants()
        .filter(|node| node.kind() == syntax::SyntaxKind::ANNOTATION)
    {
        if let Some(violation) = crate::annotations::lower_annotation(db, &node).invalid {
            diagnostics.push(Diagnostic {
                range: node.text_range(),
                severity: Severity::Error,
                code: "annotation",
                message: violation.to_owned(),
                related: Vec::new(),
            });
        }
    }
    diagnostics
}

/// name → the index of the earliest item whose top-level frame writes it.
/// Conditional writes (inside a top-level `for`/`while`/`if`/`repeat` or a
/// bare block) create the document's variable slot exactly like an
/// unconditional `name <- value`, so they participate: a later read in the
/// same document resolves to the slot even though the name is not
/// package-visible.
#[salsa::tracked(returns(ref))]
fn frame_slot_positions(db: &dyn Db, file: SourceFile) -> rustc_hash::FxHashMap<String, usize> {
    let mut positions = rustc_hash::FxHashMap::default();
    for (index, item) in item_tree(db, file).into_iter().enumerate() {
        let Some(naming) = crate::item_naming(db, item) else {
            continue;
        };
        for binding in naming.bindings.values() {
            if binding.kind != crate::naming::BindingKind::TopLevel {
                continue;
            }
            positions.entry(binding.name.clone()).or_insert(index);
        }
    }
    positions
}

/// All diagnostics of one file: syntax errors, naming findings, and type
/// errors, in position order.
#[salsa::tracked(returns(clone))]
pub fn file_diagnostics(db: &dyn Db, file: SourceFile) -> Vec<Diagnostic> {
    let mut diagnostics = parse_stage_diagnostics(db, file);

    for (item_index, item) in item_tree(db, file).into_iter().enumerate() {
        let Some(offset) = item_offset(db, item) else {
            continue;
        };
        let Some(check) = item_check(db, item) else {
            continue;
        };
        for error in &check.errors {
            let range = TextRange::new(error.range.start() + offset, error.range.end() + offset);
            diagnostics.push(render_type_error(db, range, error));
        }
        // Naming findings are item-relative too.
        let Some(module) = crate::item_hir(db, item) else {
            continue;
        };
        let Some(naming) = crate::item_naming(db, item) else {
            continue;
        };
        for unused in &naming.unused_assignments {
            let range = TextRange::new(unused.range.start() + offset, unused.range.end() + offset);
            diagnostics.push(Diagnostic {
                range,
                severity: Severity::Warning,
                code: "unused",
                message: format!("`{}` is assigned but never used.", unused.name),
                related: Vec::new(),
            });
        }
        for (expression, read) in &naming.namespace_reads {
            let Some(message) = namespace_read_message(db, read) else {
                continue;
            };
            let expression_range = module.expression(*expression).range;
            let range = TextRange::new(
                expression_range.start() + offset,
                expression_range.end() + offset,
            );
            diagnostics.push(Diagnostic {
                range,
                severity: Severity::Warning,
                code: "unresolved",
                message,
                related: Vec::new(),
            });
        }
        // Scripts resolve reads against the same universe as package files
        // (their own bindings, the package interface, imports, builtins), so
        // an unresolved read warns in both document kinds. Document-local
        // variable slots resolve sequentially: an immediate read sees only
        // slots created by earlier statements (a use before every definition
        // is unresolved, matching the top-down run), while a read from
        // inside a nested function is deferred — the closure runs after the
        // frame settled, so any slot in the document resolves it, including
        // the enclosing statement's own target (self-recursion).
        for (expression, name) in &naming.non_locals {
            if crate::package_scheme_exists(db, name)
                || super_globals(db, file).contains(name)
                || declared_global_variable(db, name)
            {
                continue;
            }
            if let Some(&earliest) = frame_slot_positions(db, file).get(name)
                && (earliest < item_index || naming.deferred_non_locals.contains(expression))
            {
                continue;
            }
            let expression_range = module.expression(*expression).range;
            let range = TextRange::new(
                expression_range.start() + offset,
                expression_range.end() + offset,
            );
            let display = display_name(name);
            let suggestion = unresolved_suggestion(db, name)
                .map(|nearest| format!(" Did you mean `{nearest}`?"))
                .unwrap_or_default();
            diagnostics.push(Diagnostic {
                range,
                severity: Severity::Warning,
                code: "unresolved",
                message: format!(
                    "I could not resolve `{display}` in this package, its imports, or builtins.{suggestion}"
                ),
                related: Vec::new(),
            });
        }
    }

    if *file.kind(db) == DocumentKind::Script {
        diagnostics.extend(script_unused_bindings(db, file));
    }
    diagnostics.extend(duplicate_binding_diagnostics(db, file));
    diagnostics.extend(duplicate_type_diagnostics(db, file));
    diagnostics.extend(unknown_type_diagnostics(db, file));

    diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start(), diagnostic.range.end()));
    diagnostics
}

/// Errors for `#:` type references no vocabulary declares: a name in an
/// annotation must be a built-in type (excluded at lowering already), an
/// in-scope binder (likewise), a project `@type`/`@alias` declaration, or a
/// stub-declared class. Anything else is a typo the checker would otherwise
/// silently treat as an opaque nominal — the classic case is a misspelled
/// record field type inside a `@type` body.
fn unknown_type_diagnostics(db: &dyn Db, file: SourceFile) -> Vec<Diagnostic> {
    let parsed = parse(db, file);
    let mut references: Vec<(String, TextRange)> = Vec::new();
    for node in parsed
        .syntax_node()
        .descendants()
        .filter(|node| node.kind() == syntax::SyntaxKind::ANNOTATION)
    {
        references.extend(crate::annotations::lower_annotation(db, &node).nominal_references);
    }
    if references.is_empty() {
        return Vec::new();
    }

    let mut known: std::collections::BTreeSet<String> = crate::stubs::stubs(db)
        .map(|library| library.nominals.iter().cloned().collect())
        .unwrap_or_default();
    if let Some(files) = crate::ProjectFiles::try_get(db) {
        for name in crate::project_type_definitions(db, files).keys() {
            known.insert(name.text(db).to_owned());
        }
    }
    // A script's own declarations shadow the project namespace; for a
    // package file this adds nothing the project map lacks.
    for definition in crate::file_type_definitions(db, file) {
        known.insert(definition.name.text(db).to_owned());
    }

    references
        .into_iter()
        .filter(|(name, _)| !known.contains(name))
        .map(|(name, range)| {
            let suggestion = nearest_name(
                &name,
                known
                    .iter()
                    .map(String::as_str)
                    .chain(ANNOTATION_PRIMITIVES.iter().copied()),
            )
            .map(|nearest| format!(" Did you mean `{nearest}`?"))
            .unwrap_or_default();
            Diagnostic {
                range,
                severity: Severity::Error,
                code: "annotation",
                message: format!(
                    "I do not know the type `{name}`. It is not a built-in type, a declared `@type` or `@alias` name, or a class from the standard-library stubs.{suggestion}"
                ),
                related: Vec::new(),
            }
        })
        .collect()
}

/// The built-in type names annotations may spell, as typo-suggestion
/// candidates (lowering resolves them before the unknown-name check, so they
/// never appear as references).
const ANNOTATION_PRIMITIVES: &[&str] = &[
    "Any",
    "Unknown",
    "NULL",
    "logical",
    "integer",
    "double",
    "complex",
    "character",
    "raw",
];

/// The named top-level definition sites of one package file, in item order:
/// each name with its binding-name range (file-absolute).
#[salsa::tracked(returns(ref))]
fn top_level_name_sites(db: &dyn Db, file: SourceFile) -> Vec<(String, TextRange)> {
    let mut sites = Vec::new();
    for item in item_tree(db, file) {
        if !matches!(
            *item.kind(db),
            crate::ItemKind::Function | crate::ItemKind::Value
        ) {
            continue;
        }
        let Some(name) = item.name(db).clone() else {
            continue;
        };
        let Some(node) = crate::item_node(db, item) else {
            continue;
        };
        let range = node
            .descendants()
            .find(|child| {
                child.kind() == syntax::SyntaxKind::NAME && child.text().to_string() == name
            })
            .map(|child| child.text_range())
            .unwrap_or_else(|| TextRange::empty(node.text_range().start()));
        sites.push((name, range));
    }
    sites
}

/// Warnings for a package name defined at top level more than once: per-site
/// winner semantics make every earlier binding dead, so each site warns, with
/// a note pointing at its nearest neighbouring definition. Occurrence order
/// is project order — `ProjectFiles` lists package documents first in
/// workspace path order — then item order within a file.
fn duplicate_binding_diagnostics(db: &dyn Db, file: SourceFile) -> Vec<Diagnostic> {
    if *file.kind(db) != DocumentKind::Package {
        return Vec::new();
    }
    let Some(files) = crate::ProjectFiles::try_get(db) else {
        return Vec::new();
    };
    let mut occurrences: std::collections::BTreeMap<&str, Vec<(SourceFile, TextRange)>> =
        std::collections::BTreeMap::new();
    for &project_file in files.files(db) {
        if *project_file.kind(db) != DocumentKind::Package {
            continue;
        }
        for (name, range) in top_level_name_sites(db, project_file) {
            occurrences
                .entry(name.as_str())
                .or_default()
                .push((project_file, *range));
        }
    }
    let mut diagnostics = Vec::new();
    for (name, sites) in occurrences {
        if sites.len() < 2 {
            continue;
        }
        for (index, &(site_file, range)) in sites.iter().enumerate() {
            if site_file != file {
                continue;
            }
            if index > 0 {
                let (earlier_file, earlier_range) = sites[index - 1];
                diagnostics.push(Diagnostic {
                    range,
                    severity: Severity::Warning,
                    code: "duplicate",
                    message: format!(
                        "Top-level binding `{name}` overwrites an earlier top-level binding in this package."
                    ),
                    related: vec![RelatedLocation {
                        file: earlier_file,
                        range: earlier_range,
                        message: "the earlier binding is here.",
                    }],
                });
            }
            if index + 1 < sites.len() {
                let (later_file, later_range) = sites[index + 1];
                diagnostics.push(Diagnostic {
                    range,
                    severity: Severity::Warning,
                    code: "duplicate",
                    message: format!(
                        "Top-level binding `{name}` is overwritten by a later top-level binding in this package."
                    ),
                    related: vec![RelatedLocation {
                        file: later_file,
                        range: later_range,
                        message: "the later binding is here.",
                    }],
                });
            }
        }
    }
    diagnostics
}

/// The `@type` / `@alias` declaration sites of one package file, in
/// declaration order: each declared name with the range of its declaring
/// directive.
#[salsa::tracked(returns(ref))]
fn type_declaration_sites(db: &dyn Db, file: SourceFile) -> Vec<(String, TextRange)> {
    let parsed = parse(db, file);
    let mut sites = Vec::new();
    for node in parsed
        .syntax_node()
        .descendants()
        .filter(|node| node.kind() == syntax::SyntaxKind::ANNOTATION)
    {
        let annotation = crate::annotations::lower_annotation(db, &node);
        for (definition, range) in annotation
            .definitions
            .iter()
            .zip(annotation.definition_sites.iter())
        {
            sites.push((definition.name.text(db).to_owned(), *range));
        }
    }
    sites
}

/// Errors for a project-global type name declared more than once: `@type` and
/// `@alias` share one project-global namespace and every declaration
/// participating in a duplicate-name conflict is erroneous (see the typing
/// reference). Script declarations shadow the project namespace instead of
/// conflicting with it, so only package files participate. Occurrence order
/// is project order, then declaration order within a file; each site points
/// at its nearest neighbouring declaration.
fn duplicate_type_diagnostics(db: &dyn Db, file: SourceFile) -> Vec<Diagnostic> {
    if *file.kind(db) != DocumentKind::Package {
        return Vec::new();
    }
    let Some(files) = crate::ProjectFiles::try_get(db) else {
        return Vec::new();
    };
    let mut occurrences: std::collections::BTreeMap<&str, Vec<(SourceFile, TextRange)>> =
        std::collections::BTreeMap::new();
    for &project_file in files.files(db) {
        if *project_file.kind(db) != DocumentKind::Package {
            continue;
        }
        for (name, range) in type_declaration_sites(db, project_file) {
            occurrences
                .entry(name.as_str())
                .or_default()
                .push((project_file, *range));
        }
    }
    let mut diagnostics = Vec::new();
    for (name, sites) in occurrences {
        if sites.len() < 2 {
            continue;
        }
        for (index, &(site_file, range)) in sites.iter().enumerate() {
            if site_file != file {
                continue;
            }
            let (neighbour_file, neighbour_range) = if index > 0 {
                sites[index - 1]
            } else {
                sites[index + 1]
            };
            diagnostics.push(Diagnostic {
                range,
                severity: Severity::Error,
                code: "annotation",
                message: format!(
                    "the type name `{name}` is declared more than once — `@type` and `@alias` declarations share one project-global namespace."
                ),
                related: vec![RelatedLocation {
                    file: neighbour_file,
                    range: neighbour_range,
                    message: "another declaration of this name is here.",
                }],
            });
        }
    }
    diagnostics
}

/// The validation failure of one qualified read, if any: an unknown
/// namespace, or (for `::` only — `:::` reaches unexported names) a name the
/// namespace does not declare. No stub corpus means no validation.
fn namespace_read_message(db: &dyn Db, read: &crate::naming::NamespaceRead) -> Option<String> {
    match crate::stubs::namespace_known(db, &read.package)? {
        false => Some(format!("unknown package namespace `{}`.", read.package)),
        true => {
            let name = read.name.as_ref()?;
            if !read.internal && !crate::stubs::namespace_exports(db, &read.package, name) {
                Some(format!("`{name}` is not exported by `{}`.", read.package))
            } else {
                None
            }
        }
    }
}

/// Names written by `<<-` with no enclosing binding anywhere in the file: R
/// creates them in the global environment, so reads of them resolve.
#[salsa::tracked(returns(ref))]
fn super_globals(db: &dyn Db, file: SourceFile) -> std::collections::BTreeSet<String> {
    let mut names = std::collections::BTreeSet::new();
    for item in item_tree(db, file) {
        if let Some(naming) = crate::item_naming(db, item) {
            names.extend(naming.super_globals.iter().cloned());
        }
    }
    names
}

/// Names declared via top-level `globalVariables(c("a", "b"))` /
/// `utils::globalVariables(...)` calls in one file — the ecosystem-standard
/// escape hatch for names bound dynamically (non-standard evaluation,
/// generated bindings). Reads of them resolve nowhere lexically on purpose,
/// so the unresolved check skips them package-wide. Only direct top-level
/// calls with literal string arguments are recognized.
#[salsa::tracked(returns(ref))]
fn file_global_variable_declarations(
    db: &dyn Db,
    file: SourceFile,
) -> std::collections::BTreeSet<String> {
    use crate::hir::{ExpressionKind, LiteralKind};
    let mut declared = std::collections::BTreeSet::new();
    for item in item_tree(db, file) {
        let Some(module) = crate::item_hir(db, item) else {
            continue;
        };
        let Some(root) = module.root else {
            continue;
        };
        let ExpressionKind::Call { callee, arguments } = &module.expression(root).kind else {
            continue;
        };
        let is_global_variables = match &module.expression(*callee).kind {
            ExpressionKind::NameRef(name) => name == "globalVariables",
            ExpressionKind::Namespace { package, name, .. } => {
                package.as_deref() == Some("utils") && name.as_deref() == Some("globalVariables")
            }
            _ => false,
        };
        if !is_global_variables {
            continue;
        }
        let Some(first) = arguments.first().and_then(|argument| argument.value) else {
            continue;
        };
        // A single string, or a `c(...)` of strings — only what is
        // statically knowable is declared; other entries are skipped.
        let mut collect = |id| {
            if let ExpressionKind::Literal(LiteralKind::String(value)) = &module.expression(id).kind
                && !value.is_empty()
            {
                declared.insert(value.clone());
            }
        };
        match &module.expression(first).kind {
            ExpressionKind::Call {
                callee: inner,
                arguments: entries,
            } if matches!(
                &module.expression(*inner).kind,
                ExpressionKind::NameRef(name) if name == "c"
            ) =>
            {
                for entry in entries {
                    if let Some(value) = entry.value {
                        collect(value);
                    }
                }
            }
            _ => {
                collect(first);
            }
        }
    }
    declared
}

/// Whether any package file declares `name` via `globalVariables`.
fn declared_global_variable(db: &dyn Db, name: &str) -> bool {
    crate::ProjectFiles::try_get(db).is_some_and(|files| {
        files
            .files(db)
            .iter()
            .any(|&file| file_global_variable_declarations(db, file).contains(name))
    })
}

/// A name as R source spells it: non-syntactic names need backticks (a
/// leading dot must not be followed by a digit — `.2way` is not syntactic).
fn display_name(name: &str) -> String {
    let mut characters = name.chars();
    let syntactic = matches!(characters.next(), Some(first) if first.is_alphabetic() || first == '.')
        && characters.all(|c| c.is_alphanumeric() || c == '.' || c == '_')
        && !(name.starts_with('.') && name[1..].chars().next().is_some_and(|c| c.is_ascii_digit()));
    if syntactic {
        name.to_owned()
    } else {
        format!("`{name}`")
    }
}

/// The nearest stub name for a typo hint on an unresolved reference.
/// Memoized per name: the same unresolved name recurs across a project and
/// the candidate scan over the whole stub corpus is the expensive part.
fn unresolved_suggestion(db: &dyn Db, name: &str) -> Option<String> {
    typo_suggestion(db, crate::types::Name::new(db, name.to_owned()))
}

#[salsa::tracked(returns(clone))]
fn typo_suggestion<'db>(db: &'db dyn Db, name: crate::types::Name<'db>) -> Option<String> {
    let library = crate::stubs::stubs(db)?;
    nearest_name(name.text(db), library.schemes.keys().map(String::as_str)).map(str::to_owned)
}

/// The closest candidate within an edit-distance budget scaled to the name's
/// length (nothing for names shorter than 3 characters); distance ties break
/// to the lexicographically smallest candidate so the hint is deterministic.
fn nearest_name<'a>(name: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let name_characters: Vec<char> = name.chars().collect();
    if name_characters.len() < 3 {
        return None;
    }
    let budget = if name_characters.len() >= 5 { 2 } else { 1 };
    let mut candidate_characters: Vec<char> = Vec::new();
    let mut previous: Vec<usize> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut best: Option<(usize, &str)> = None;
    for candidate in candidates {
        if candidate == name {
            continue;
        }
        candidate_characters.clear();
        candidate_characters.extend(candidate.chars());
        let Some(distance) = edit_distance_within(
            &name_characters,
            &candidate_characters,
            budget,
            &mut previous,
            &mut current,
        ) else {
            continue;
        };
        best = match best {
            Some((best_distance, best_name))
                if (best_distance, best_name) <= (distance, candidate) =>
            {
                Some((best_distance, best_name))
            }
            _ => Some((distance, candidate)),
        };
    }
    best.map(|(_, candidate)| candidate)
}

/// Levenshtein distance, `None` when it exceeds `budget` (with a length
/// pre-check and an early bail once a whole DP row exceeds the budget). The
/// caller lends the two DP rows so a scan over many candidates allocates
/// nothing per candidate.
fn edit_distance_within(
    left: &[char],
    right: &[char],
    budget: usize,
    previous: &mut Vec<usize>,
    current: &mut Vec<usize>,
) -> Option<usize> {
    if left.len().abs_diff(right.len()) > budget {
        return None;
    }
    previous.clear();
    previous.extend(0..=right.len());
    current.clear();
    current.resize(right.len() + 1, 0);
    for (row, left_character) in left.iter().enumerate() {
        current[0] = row + 1;
        let mut row_minimum = current[0];
        for (column, right_character) in right.iter().enumerate() {
            let substitution = previous[column] + usize::from(left_character != right_character);
            current[column + 1] = substitution
                .min(previous[column + 1] + 1)
                .min(current[column] + 1);
            row_minimum = row_minimum.min(current[column + 1]);
        }
        if row_minimum > budget {
            return None;
        }
        std::mem::swap(previous, current);
    }
    (previous[right.len()] <= budget).then_some(previous[right.len()])
}

/// A script's top level is one frame executed in order, so its bindings are
/// subject to the unused check across items: an assignment is dead when no
/// later item reads it before the next rebinding, and no nested function
/// reads the name at all (a deferred read runs after the frame is built, so
/// it keeps every write to the name observable — the captured-slot rule).
fn script_unused_bindings(db: &dyn Db, file: SourceFile) -> Vec<Diagnostic> {
    // A broken statement reports its syntax error and nothing else, so a
    // definer inside an R-grammar error region never warns as unused.
    let error_ranges: Vec<TextRange> = crate::parse(db, file)
        .errors()
        .iter()
        .filter(|error| !error.in_annotation)
        .map(|error| error.range)
        .collect();
    struct Definer {
        item_index: usize,
        name: String,
        range: TextRange,
        used: bool,
        /// A conditionally executed write (inside a top-level loop or `if`):
        /// it rebinds the slot only on some runs, so it does not end the
        /// liveness of earlier definers of the name.
        conditional: bool,
    }
    let mut definers: Vec<Definer> = Vec::new();
    // Every item participates as a reader — a bare `print(x)` statement keeps
    // `x` alive. Definers are the item's own named definition plus any other
    // top-level frame slot the item creates (a conditional write inside a
    // top-level `for`/`while`/`if` binds the frame like any assignment).
    let mut reads: Vec<(usize, String, bool)> = Vec::new();
    for (index, item) in item_tree(db, file).into_iter().enumerate() {
        let Some(naming) = crate::item_naming(db, item) else {
            continue;
        };
        let Some(offset) = item_offset(db, item) else {
            continue;
        };
        let item_name = matches!(
            *item.kind(db),
            crate::ItemKind::Function | crate::ItemKind::Value
        )
        .then(|| item.name(db).clone())
        .flatten();
        let broken = |range: TextRange| {
            error_ranges
                .iter()
                .any(|error| error.start() <= range.end() && range.start() <= error.end())
        };
        if let Some(name) = item_name.clone()
            && let Some(module) = crate::item_hir(db, item)
            && let Some(root) = module.root
        {
            let root_range = module.expression(root).range;
            let range = TextRange::new(root_range.start() + offset, root_range.end() + offset);
            if !broken(range) {
                definers.push(Definer {
                    item_index: index,
                    name,
                    range,
                    used: false,
                    conditional: false,
                });
            }
        }
        for binding in naming.bindings.values() {
            if binding.kind != crate::naming::BindingKind::TopLevel
                || item_name.as_deref() == Some(binding.name.as_str())
            {
                continue;
            }
            let range =
                TextRange::new(binding.range.start() + offset, binding.range.end() + offset);
            if !broken(range) {
                definers.push(Definer {
                    item_index: index,
                    name: binding.name.clone(),
                    range,
                    // A read within the item itself (a loop reading its own
                    // carried variable) already keeps the definer alive.
                    used: naming.used_top_level_names.contains(&binding.name),
                    conditional: true,
                });
            }
        }
        for (expression, name) in &naming.non_locals {
            let deferred = naming.deferred_non_locals.contains(expression);
            reads.push((index, name.clone(), deferred));
        }
        // A quiet read (data masking, an opaque operator) is never reported
        // as unresolved, but at runtime it falls back to enclosing bindings,
        // so it keeps definers alive like any other read.
        for (expression, name) in &naming.quiet_reads {
            let deferred = naming.deferred_quiet_reads.contains(expression);
            reads.push((index, name.clone(), deferred));
        }
        // A maybe-undefined read of the item's own top-level slot (a loop
        // reading its carried variable) reaches the enclosing frame on the
        // unwritten path — its first iteration reads the earlier binding, so
        // it counts as a cross-item read too.
        for expression in &naming.maybe_undefined {
            let Some(slot) = naming.resolutions.get(expression) else {
                continue;
            };
            let Some(binding) = naming.bindings.get(slot) else {
                continue;
            };
            if binding.kind == crate::naming::BindingKind::TopLevel {
                reads.push((index, binding.name.clone(), false));
            }
        }
    }
    for (read_index, name, deferred) in &reads {
        if *deferred {
            // A read from inside a function: the function may run any time
            // after the frame exists, so every write to the name is live.
            for definer in definers.iter_mut().filter(|d| d.name == *name) {
                definer.used = true;
            }
        } else {
            // An immediate read sees the binding current at its item: the
            // nearest earlier definer — but a conditional definer rebinds
            // only on some runs, so marking continues past it until the
            // nearest unconditional one.
            for definer in definers
                .iter_mut()
                .rev()
                .filter(|d| d.name == *name && d.item_index < *read_index)
            {
                definer.used = true;
                if !definer.conditional {
                    break;
                }
            }
        }
    }
    definers
        .into_iter()
        .filter(|definer| {
            !definer.used && !definer.name.starts_with('.') && !definer.name.starts_with('_')
        })
        .map(|definer| Diagnostic {
            range: definer.range,
            severity: Severity::Warning,
            code: "unused",
            message: format!("`{}` is assigned but never used.", definer.name),
            related: Vec::new(),
        })
        .collect()
}

/// The absolute byte offset of an item's subtree inside its file.
fn item_offset(db: &dyn Db, item: Item<'_>) -> Option<syntax::TextSize> {
    crate::item_spans(db, *item.file(db))
        .iter()
        .find(|span| span.item == item)
        .map(|span| span.range.start())
}

/// Strict-mode diagnostics: reports at `Unknown` origins, assembled
/// separately so hosts publish them only under `[check] strict` or the
/// per-file directive. An origin that is an assignment's value is phrased
/// against the assigned name (that is where the annotation goes); any other
/// origin is phrased against the expression itself.
#[salsa::tracked(returns(clone))]
pub fn strict_diagnostics(db: &dyn Db, file: SourceFile) -> Vec<Diagnostic> {
    use crate::check::StrictOriginKind;
    use crate::hir::ExpressionKind;

    let mut diagnostics = Vec::new();
    for item in item_tree(db, file) {
        let Some(offset) = item_offset(db, item) else {
            continue;
        };
        let Some(check) = item_check(db, item) else {
            continue;
        };
        if check.strict_origins.is_empty() {
            continue;
        }
        let Some(module) = crate::item_hir(db, item) else {
            continue;
        };
        let mut assignment_targets = rustc_hash::FxHashMap::default();
        for expression in &module.expressions {
            if let ExpressionKind::Assign { target, value, .. } = &expression.kind
                && let ExpressionKind::NameRef(name) = &module.expression(*target).kind
            {
                assignment_targets.insert(*value, name.clone());
            }
        }
        for origin in &check.strict_origins {
            // A loop-widened or recursive origin is about the named binding,
            // never about the expression it happens to be the value of.
            let assignment_target = match &origin.kind {
                StrictOriginKind::LoopWidened(_) | StrictOriginKind::RecursiveUnknown(_) => None,
                _ => assignment_targets.get(&origin.expression),
            };
            let message = if let Some(name) = assignment_target {
                format!(
                    "strict mode: could not determine the type of `{name}`; add a type annotation"
                )
            } else {
                match &origin.kind {
                    StrictOriginKind::UnsupportedConstruct => {
                        "strict mode: this expression has an undetermined type (`Unknown`)"
                            .to_owned()
                    }
                    StrictOriginKind::UndeterminedReference(name) => format!(
                        "strict mode: could not determine the type of `{name}`; it has no known type"
                    ),
                    StrictOriginKind::LoopWidened(name) => format!(
                        "strict mode: could not determine the type of `{name}`; its type does not stabilize across loop iterations — add a type annotation"
                    ),
                    StrictOriginKind::RecursiveUnknown(name) => format!(
                        "strict mode: could not determine the full type of `{name}`; it is defined recursively — add a type annotation"
                    ),
                }
            };
            diagnostics.push(Diagnostic {
                range: TextRange::new(origin.range.start() + offset, origin.range.end() + offset),
                severity: Severity::Error,
                code: "strict",
                message,
                related: Vec::new(),
            });
        }
    }
    diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start(), diagnostic.range.end()));
    diagnostics
}

fn plural<'a>(count: usize, one: &'a str, many: &'a str) -> &'a str {
    if count == 1 { one } else { many }
}

fn render_names(names: &[String]) -> String {
    names
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_type_error(db: &dyn Db, range: TextRange, error: &TypeError<'_>) -> Diagnostic {
    let message = render_type_error_message(db, error);
    Diagnostic {
        range,
        severity: Severity::Error,
        code: "type-mismatch",
        message,
        related: Vec::new(),
    }
}

fn render_type_error_message(db: &dyn Db, error: &TypeError<'_>) -> String {
    let mut renderer = TypeRenderer::default();
    match &error.kind {
        TypeErrorKind::Mismatch { expected, found } => format!(
            "expected `{}`, found `{}`",
            renderer.render(db, *expected),
            renderer.render(db, *found)
        ),
        TypeErrorKind::NotAFunction { found } => {
            format!(
                "this has type `{}`, which is not a function — it cannot be called",
                renderer.render(db, *found)
            )
        }
        TypeErrorKind::ArityMismatch { expected, found } => format!(
            "this call passes {found} positional {}, but the function only takes {expected}",
            plural(*found, "argument", "arguments"),
        ),
        TypeErrorKind::NamedArgumentMismatch {
            expected_parameters,
            actual_arguments,
        } => {
            let arguments = format!(
                "this call names {} {}",
                plural(actual_arguments.len(), "an argument", "arguments"),
                render_names(actual_arguments),
            );
            if expected_parameters.is_empty() {
                format!("{arguments}, but the function has no named parameters")
            } else {
                format!(
                    "{arguments}, but the function's named {} {}",
                    plural(expected_parameters.len(), "parameter is", "parameters are"),
                    render_names(expected_parameters),
                )
            }
        }
        TypeErrorKind::AnnotationParameterMismatch { name } => format!(
            "this annotation names a parameter `{name}`, but the function does not define one — annotation parameter names must match the function's parameter names"
        ),
        TypeErrorKind::AliasCycle { name } => {
            format!("Type alias `{name}` expands in a cycle.")
        }
        TypeErrorKind::ConstraintViolation { constraint, found } => {
            let expected_description = match constraint {
                Constraint::Unconstrained => "a value",
                Constraint::Numeric => "a numeric value (`integer` or `double`)",
                Constraint::AtomicElement => {
                    "an atomic value (`logical`, `integer`, `double`, `complex`, `character`, or `raw`)"
                }
                Constraint::ScalarNumeric => "a scalar numeric value (`integer` or `double`)",
            };
            format!(
                "expected {expected_description}, found `{}`",
                renderer.render(db, *found)
            )
        }
        TypeErrorKind::NotAList { found } => {
            format!("expected a list, found `{}`", renderer.render(db, *found))
        }
        TypeErrorKind::NotIterable { found } => format!(
            "this `for` sequence is `{}`, which cannot be iterated — expected a vector or list.",
            renderer.render(db, *found)
        ),
        TypeErrorKind::UnsupportedSubset { found } => {
            format!("`[` is not supported on `{}`", renderer.render(db, *found))
        }
        TypeErrorKind::UnsupportedIndexShape { index_count } => match index_count {
            0 => "indexing with an empty index (`x[]`) is not supported yet".to_owned(),
            1 => "indexing with a named index argument is not supported yet".to_owned(),
            count => format!(
                "indexing with {count} indexes is not supported yet — Roughly does not model matrix and data.frame subsetting"
            ),
        },
        TypeErrorKind::PositionDoesNotExist {
            position,
            container,
        } => format!(
            "position {position} does not exist in `{}`",
            renderer.render(db, *container)
        ),
        TypeErrorKind::FieldDoesNotExist { field, container } => format!(
            "field `{field}` does not exist in `{}`",
            renderer.render(db, *container)
        ),
        TypeErrorKind::DollarOnAtomicVector { found } => format!(
            "R's `$` operator is invalid on atomic vectors; this value is `{}` — extract an element with `[[` instead.",
            renderer.render(db, *found)
        ),
        TypeErrorKind::MixedListElements => {
            "the elements of a `list(...)` must be either all named or all unnamed".to_owned()
        }
        TypeErrorKind::InvalidOperand { expected, found } => {
            let expected_description = match expected {
                OperandExpectation::Numeric => "a numeric value (`integer` or `double`)",
                OperandExpectation::ScalarNumeric => {
                    "a scalar numeric value (`integer` or `double`)"
                }
                OperandExpectation::Logical => "a `logical` value",
                OperandExpectation::Comparable => {
                    "a comparable value (numeric, `character`, or `logical`)"
                }
            };
            format!(
                "expected {expected_description}, found `{}`",
                renderer.render(db, *found)
            )
        }
        TypeErrorKind::NoMatchingOverload {
            name,
            candidates,
            first,
        } => {
            let mut message = format!(
                "no overload of `{name}` matches these arguments — I tried all {candidates} declared signatures"
            );
            if let Some(first) = first {
                message.push_str(&format!(
                    "; the first candidate fails with: {}",
                    render_type_error_message(db, first)
                ));
            }
            message
        }
        TypeErrorKind::InfiniteType {
            variable,
            container,
        } => format!(
            "I cannot construct an infinite type: {} occurs inside `{}`.",
            renderer.render(db, *variable),
            renderer.render(db, *container)
        ),
    }
}

/// The user-facing type renderer: `T`/`U`/`V`… in first-occurrence order.
#[derive(Default)]
pub struct TypeRenderer<'db> {
    names: Vec<RenderedVar<'db>>,
}

#[derive(PartialEq, Eq)]
enum RenderedVar<'db> {
    Inference(crate::types::InferenceVar),
    Rigid(Name<'db>),
}

impl<'db> TypeRenderer<'db> {
    pub fn render(&mut self, db: &'db dyn Db, ty: Ty<'db>) -> String {
        match ty.kind(db) {
            TyKind::Any => "Any".to_owned(),
            TyKind::Unknown => "Unknown".to_owned(),
            TyKind::Null => "NULL".to_owned(),
            TyKind::Scalar(atomic) => atomic_name(*atomic).to_owned(),
            TyKind::Vector(element) => format!("{}[]", self.render(db, *element)),
            TyKind::NamedVector(element) => format!("{}[named]", self.render(db, *element)),
            TyKind::List(element) => format!("list[{}]", self.render(db, *element)),
            TyKind::NamedList(element) => format!("list[named: {}]", self.render(db, *element)),
            TyKind::Tuple(items) => {
                let items: Vec<String> = items.iter().map(|&item| self.render(db, item)).collect();
                format!("list{{{}}}", items.join(", "))
            }
            TyKind::Record(fields) => {
                let fields: Vec<String> = fields
                    .iter()
                    .map(|field| format!("{}: {}", field.name.text(db), self.render(db, field.ty)))
                    .collect();
                format!("list{{{}}}", fields.join(", "))
            }
            TyKind::Function(function) => self.render_function(db, function),
            TyKind::Union(members) => {
                let members: Vec<String> = members
                    .iter()
                    .map(|&member| self.render(db, member))
                    .collect();
                members.join(" | ")
            }
            TyKind::Named(name, arguments) => {
                if arguments.is_empty() {
                    name.text(db).to_owned()
                } else {
                    let arguments: Vec<String> = arguments
                        .iter()
                        .map(|&argument| self.render(db, argument))
                        .collect();
                    format!("{}<{}>", name.text(db), arguments.join(", "))
                }
            }
            TyKind::Var(var) => self.variable_name(RenderedVar::Inference(*var)),
            TyKind::Rigid(name) => self.variable_name(RenderedVar::Rigid(*name)),
        }
    }

    pub fn render_scheme(&mut self, db: &'db dyn Db, scheme: &TypeScheme<'db>) -> String {
        match self.render_binder_prefix(&scheme.binders) {
            Some(prefix) => format!("{prefix} {}", self.render(db, scheme.body)),
            None => self.render(db, scheme.body),
        }
    }

    /// The `<T, U: numeric>` binder prefix of a scheme, registering each
    /// binder's display name so the body rendered afterwards through the same
    /// renderer reuses them. `None` for an empty binder list.
    pub fn render_binder_prefix(&mut self, binders: &[(Name<'db>, Constraint)]) -> Option<String> {
        if binders.is_empty() {
            return None;
        }
        let binders: Vec<String> = binders
            .iter()
            .map(|(name, constraint)| {
                let rendered = self.variable_name(RenderedVar::Rigid(*name));
                match constraint {
                    Constraint::Unconstrained => rendered,
                    Constraint::Numeric => format!("{rendered}: numeric"),
                    Constraint::AtomicElement => format!("{rendered}: atomic"),
                    Constraint::ScalarNumeric => format!("{rendered}: scalar numeric"),
                }
            })
            .collect();
        Some(format!("<{}>", binders.join(", ")))
    }

    fn render_function(&mut self, db: &'db dyn Db, function: &FunctionType<'db>) -> String {
        let mut parameters = Vec::new();
        for ty in &function.positional {
            parameters.push(self.render(db, *ty));
        }
        for field in &function.named {
            let name = if field.optional {
                format!("[{}]", field.name.text(db))
            } else {
                field.name.text(db).to_owned()
            };
            parameters.push(format!("{name}: {}", self.render(db, field.ty)));
        }
        if let Some(rest) = &function.variadic {
            parameters.push(format!("...: {}", self.render(db, rest.element)));
        }
        let ret = self.render(db, function.ret);
        format!("fn({}) -> {}", parameters.join(", "), ret)
    }

    fn variable_name(&mut self, var: RenderedVar<'db>) -> String {
        let index = match self.names.iter().position(|existing| *existing == var) {
            Some(index) => index,
            None => {
                self.names.push(var);
                self.names.len() - 1
            }
        };
        let letter = (b'T' + (index as u8 % 7)) as char;
        let suffix = index / 7;
        if suffix == 0 {
            letter.to_string()
        } else {
            format!("{letter}{suffix}")
        }
    }
}

fn atomic_name(atomic: Atomic) -> &'static str {
    match atomic {
        Atomic::Logical => "logical",
        Atomic::Integer => "integer",
        Atomic::Double => "double",
        Atomic::Complex => "complex",
        Atomic::Character => "character",
        Atomic::Raw => "raw",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DocumentKind, ProjectFiles, RootDatabase, SourceFile};

    fn render_all(db: &RootDatabase, file: SourceFile) -> Vec<String> {
        file_diagnostics(db, file)
            .into_iter()
            .map(|d| {
                format!(
                    "{}..{} {}[{}] {}",
                    u32::from(d.range.start()),
                    u32::from(d.range.end()),
                    match d.severity {
                        Severity::Error => "error",
                        Severity::Warning => "warning",
                    },
                    d.code,
                    d.message
                )
            })
            .collect()
    }

    #[test]
    fn file_diagnostics_end_to_end() {
        let db = RootDatabase::default();
        let util = SourceFile::new(
            &db,
            "add <- function(x, y) x + y\n".to_owned(),
            DocumentKind::Package,
        );
        let main = SourceFile::new(
            &db,
            "bad <- function() add(\"a\", 2L)\nmissing_fn <- function() nowhere()\n".to_owned(),
            DocumentKind::Package,
        );
        ProjectFiles::new(&db, vec![util, main]);
        let rendered = render_all(&db, main);
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("type-mismatch") && line.contains("character")),
            "expected a mismatch mentioning character: {rendered:?}"
        );
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("unresolved") && line.contains("nowhere")),
            "expected an unresolved warning for nowhere: {rendered:?}"
        );
        // The second item's finding is offset into the file (absolute range).
        let missing = rendered
            .iter()
            .find(|line| line.contains("nowhere"))
            .expect("nowhere finding");
        let start: u32 = missing.split("..").next().unwrap().parse().unwrap();
        assert!(start > 30, "range must be file-absolute, got {missing}");
    }

    #[test]
    fn unused_and_syntax_diagnostics_render() {
        let db = RootDatabase::default();
        let file = SourceFile::new(
            &db,
            "f <- function() {\n  dead <- 1\n  dead <- 2\n  dead\n}\ng <- function( {\n".to_owned(),
            DocumentKind::Script,
        );
        let rendered = render_all(&db, file);
        assert!(
            rendered.iter().any(|line| line.contains("unused")),
            "expected a dead-store warning: {rendered:?}"
        );
        assert!(
            rendered.iter().any(|line| line.contains("syntax-error")),
            "expected a syntax error: {rendered:?}"
        );
    }

    #[test]
    fn renderer_shares_names_across_both_sides() {
        let db = RootDatabase::default();
        let mut renderer = TypeRenderer::default();
        let t = crate::types::Ty::new(
            &db,
            crate::types::TyKind::Rigid(crate::types::Name::new(&db, "A".to_owned())),
        );
        let u = crate::types::Ty::new(
            &db,
            crate::types::TyKind::Rigid(crate::types::Name::new(&db, "B".to_owned())),
        );
        // First occurrence order: A -> T, B -> U; A again stays T.
        assert_eq!(renderer.render(&db, t), "T");
        assert_eq!(renderer.render(&db, u), "U");
        assert_eq!(renderer.render(&db, t), "T");
    }
}
