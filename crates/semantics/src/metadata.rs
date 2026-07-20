//! Package metadata: the `NAMESPACE` and `DESCRIPTION` files.
//!
//! R package `NAMESPACE` files are plain R call syntax (`import(pkg)`,
//! `importFrom(pkg, name)`), so they parse with the ordinary grammar.
//! `DESCRIPTION` is DCF (`Field: value` with indented continuation lines).
//! Hosts parse both at the package root and install the facts as the
//! [`PackageMetadata`] input; resolution and diagnostics consume them:
//!
//! - an `importFrom(pkg, name)` makes `name` a known bare read package-wide
//!   (typed by `pkg`'s stubs when they exist, `Unknown` otherwise);
//! - a whole-namespace `import(pkg)` makes `pkg`'s stub exports known bare
//!   reads; when no stubs describe `pkg`, its export set is unknowable, so
//!   every otherwise-unresolved bare read is tolerated rather than guessed
//!   (zero false positives over typo detection);
//! - a `pkg::name` read of a namespace the stub corpus does not know is
//!   tolerated when `pkg` is a declared dependency instead of warning about
//!   an unknown namespace.

use crate::Db;
use std::collections::BTreeSet;
use syntax::{SyntaxKind, SyntaxNode, TextRange};

/// Import facts from `NAMESPACE` plus the dependency universe from
/// `DESCRIPTION`. Absent input (no metadata files, single-file analysis)
/// means no imports and no declared dependencies — resolution behaves exactly
/// as before metadata existed.
#[salsa::input(singleton, debug)]
pub struct PackageMetadata {
    /// `(namespace, None)` for `import(pkg)`; `(namespace, Some(name))` for
    /// one `importFrom(pkg, name)` name. Order-insensitive facts: hosts
    /// should sort + dedupe so formatting edits do not invalidate.
    #[returns(ref)]
    pub imports: Vec<(String, Option<String>)>,
    /// Package names from `DESCRIPTION`'s `Depends`/`Imports`/`Suggests`/
    /// `Enhances` fields.
    #[returns(ref)]
    pub dependencies: BTreeSet<String>,
}

/// Whether a bare read of `name` is satisfied by the package's declared
/// imports. Exact `importFrom` names always resolve (a typo against known
/// stubs is already warned at the import site); a whole-namespace import
/// resolves the namespace's stub exports, or — when no stubs describe the
/// namespace — any name at all, since the export set is unknowable.
pub fn imported_bare(db: &dyn Db, name: &str) -> bool {
    let Some(metadata) = PackageMetadata::try_get(db) else {
        return false;
    };
    metadata
        .imports(db)
        .iter()
        .any(|(namespace, imported)| match imported {
            Some(imported) => imported == name,
            None => match crate::stubs::namespace_known(db, namespace) {
                Some(true) => crate::stubs::namespace_exports(db, namespace, name),
                Some(false) | None => true,
            },
        })
}

/// Whether `package` is part of the package's declared universe: a
/// `DESCRIPTION` dependency or the source of any `NAMESPACE` import.
pub fn declared_dependency(db: &dyn Db, package: &str) -> bool {
    let Some(metadata) = PackageMetadata::try_get(db) else {
        return false;
    };
    metadata.dependencies(db).contains(package)
        || metadata
            .imports(db)
            .iter()
            .any(|(namespace, _)| namespace == package)
}

/// One `import`/`importFrom` directive occurrence with its source range, for
/// host-side validation at the import site.
pub struct NamespaceImport {
    pub namespace: String,
    /// `None` for a whole-namespace `import(pkg)`; `Some` for one
    /// `importFrom(pkg, name)` name.
    pub name: Option<String>,
    pub range: TextRange,
}

/// The `import`/`importFrom` directives of a NAMESPACE source, in file order.
/// Directives R would reject (a malformed file, non-name arguments) are
/// skipped rather than reported: R itself is the authority on NAMESPACE
/// syntax, and this pass only wants the import facts.
pub fn parse_namespace_imports(source: &str) -> Vec<NamespaceImport> {
    let parse = syntax::parse(source);
    let mut imports = Vec::new();
    for node in parse.syntax_node().children() {
        if node.kind() != SyntaxKind::CALL_EXPR {
            continue;
        }
        let Some(callee) = node
            .children()
            .find(|child| child.kind() != SyntaxKind::ARGUMENT_LIST)
        else {
            continue;
        };
        if callee.kind() != SyntaxKind::NAME {
            continue;
        }
        // A named argument (`except = ...`) keeps its name before the value;
        // the value is the argument's last expression child either way.
        let values: Vec<SyntaxNode> = node
            .children()
            .find(|child| child.kind() == SyntaxKind::ARGUMENT_LIST)
            .map(|list| {
                list.children()
                    .filter(|child| child.kind() == SyntaxKind::ARGUMENT)
                    .filter_map(|argument| {
                        argument
                            .children()
                            .filter(|child| syntax::ast::is_expression_kind(child.kind()))
                            .last()
                    })
                    .collect()
            })
            .unwrap_or_default();
        match callee.text().to_string().as_str() {
            "import" => {
                // `import(pkg, ...)` may list several namespaces; `except =`
                // keyword arguments are not name values and fall out of the
                // extraction below.
                for value in values {
                    if let Some((namespace, range)) = name_argument(&value) {
                        imports.push(NamespaceImport {
                            namespace,
                            name: None,
                            range,
                        });
                    }
                }
            }
            "importFrom" => {
                let mut values = values.into_iter();
                let Some((namespace, _)) = values.next().and_then(|value| name_argument(&value))
                else {
                    continue;
                };
                for value in values {
                    if let Some((name, range)) = name_argument(&value) {
                        imports.push(NamespaceImport {
                            namespace: namespace.clone(),
                            name: Some(name),
                            range,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    imports
}

/// The import facts of a parsed NAMESPACE, normalized for the
/// [`PackageMetadata`] input: sorted and deduplicated so directive order and
/// formatting edits do not invalidate downstream queries.
pub fn normalized_imports(imports: &[NamespaceImport]) -> Vec<(String, Option<String>)> {
    let set: BTreeSet<(String, Option<String>)> = imports
        .iter()
        .map(|import| (import.namespace.clone(), import.name.clone()))
        .collect();
    set.into_iter().collect()
}

/// Package names from a DESCRIPTION source's `Depends`, `Imports`,
/// `Suggests`, and `Enhances` fields. DCF format: a field starts at column
/// zero as `Name: value` and continues over indented lines; entries are
/// comma-separated package names with optional version constraints in
/// parentheses. `R` itself is a version pin, not a package.
pub fn parse_description_dependencies(source: &str) -> BTreeSet<String> {
    let mut dependencies = BTreeSet::new();
    let mut collecting = false;
    for line in source.lines() {
        let continuation = line.starts_with([' ', '\t']);
        if !continuation {
            collecting = false;
            if let Some((field, rest)) = line.split_once(':') {
                if matches!(
                    field.trim(),
                    "Depends" | "Imports" | "Suggests" | "Enhances"
                ) {
                    collecting = true;
                    collect_dependency_entries(rest, &mut dependencies);
                }
                continue;
            }
        }
        if collecting {
            collect_dependency_entries(line, &mut dependencies);
        }
    }
    dependencies
}

fn collect_dependency_entries(text: &str, dependencies: &mut BTreeSet<String>) {
    for entry in text.split(',') {
        let name = entry
            .split('(')
            .next()
            .unwrap_or_default()
            .trim()
            .trim_end_matches(',');
        if name.is_empty() || name == "R" {
            continue;
        }
        dependencies.insert(name.to_owned());
    }
}

/// A directive argument that names something: a bare identifier or a string
/// literal (R accepts both spellings in NAMESPACE files).
fn name_argument(value: &SyntaxNode) -> Option<(String, TextRange)> {
    match value.kind() {
        SyntaxKind::NAME => Some((value.text().to_string(), value.text_range())),
        SyntaxKind::LITERAL => {
            let token = value
                .children_with_tokens()
                .filter_map(|element| element.into_token())
                .find(|token| token.kind() == SyntaxKind::STRING)?;
            let text = token.text();
            let content = text
                .trim_start_matches(['"', '\''])
                .trim_end_matches(['"', '\'']);
            Some((content.to_owned(), value.text_range()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_dependencies_parse_dcf_fields() {
        let dependencies = parse_description_dependencies(
            "Package: demo\nDepends:\n    R (>= 4.0),\n    data.table (>= 1.14)\nImports: dplyr,\n    rlang (>= 1.0)\nSuggests: testthat\nTitle: A demo\n",
        );
        let names: Vec<&str> = dependencies.iter().map(String::as_str).collect();
        assert_eq!(names, ["data.table", "dplyr", "rlang", "testthat"]);
    }

    #[test]
    fn description_without_dependency_fields_is_empty() {
        let dependencies =
            parse_description_dependencies("Package: demo\nTitle: Imports: not-a-field\n");
        assert!(dependencies.is_empty());
    }
}
