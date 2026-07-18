//! R package `NAMESPACE` files are plain R call syntax (`import(pkg)`,
//! `importFrom(pkg, name)`), so they parse with the ordinary grammar; this
//! module recognizes the import directives and validates the imported names
//! against the stub corpus's namespaces. Resolution semantics are
//! deliberately unchanged — stubbed names already resolve bare — so the value
//! here is catching import typos against namespaces the stubs actually know.

use semantics::diagnostics::{Diagnostic, Severity};
use semantics::lints::LintLevel;
use std::collections::BTreeSet;
use syntax::{SyntaxKind, SyntaxNode, TextRange};

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

/// One warning per `importFrom(pkg, name)` whose namespace the export table
/// knows but whose name it does not list — the same fact `pkg::name`
/// validation checks, surfaced at the import site. Unknown namespaces produce
/// nothing: without stubs there is no export set to check against.
pub fn namespace_import_problems(
    imports: &[NamespaceImport],
    knows_namespace: &dyn Fn(&str) -> bool,
    exports: &dyn Fn(&str, &str) -> bool,
) -> Vec<Diagnostic> {
    imports
        .iter()
        .filter_map(|import| {
            let name = import.name.as_ref()?;
            if !knows_namespace(&import.namespace) || exports(&import.namespace, name) {
                return None;
            }
            Some(Diagnostic {
                range: import.range,
                severity: Severity::Warning,
                code: "unresolved",
                message: format!("`{name}` is not exported by `{}`.", import.namespace),
            })
        })
        .collect()
}

/// The `unused-import` lint findings: one per `importFrom(pkg, name)` whose
/// `name` appears nowhere in `used_tokens` — the set of every token text used
/// across the package's R sources. Default-off because a package may import a
/// name only to re-export it or for a side effect, and usage is a
/// deliberately conservative token scan (any token equal to the name counts,
/// including `pkg::name` and operator spellings), so it under-reports rather
/// than risk a false positive. Whole-namespace `import(pkg)` directives are
/// not checked.
pub fn unused_import_diagnostics(
    imports: &[NamespaceImport],
    used_tokens: &BTreeSet<String>,
    level: LintLevel,
) -> Vec<Diagnostic> {
    let severity = match level {
        LintLevel::Default | LintLevel::Off => return Vec::new(),
        LintLevel::Warn => Severity::Warning,
        LintLevel::Error => Severity::Error,
    };
    imports
        .iter()
        .filter_map(|import| {
            let name = import.name.as_ref()?;
            if used_tokens.contains(name) {
                return None;
            }
            Some(Diagnostic {
                range: import.range,
                severity: severity.clone(),
                code: "unused-import",
                message: format!(
                    "imported name `{name}` from `{}` is never used.",
                    import.namespace
                ),
            })
        })
        .collect()
}

/// Every token's text across an R source, for the conservative
/// `unused-import` usage scan. Uses the real parser so it sees exactly the
/// tokens R does (identifiers, operators like `%>%`, string contents), never
/// a hand-rolled scanner that could drift. String tokens contribute their
/// unquoted content (a NAMESPACE import may name an operator that sources
/// spell as a plain token).
pub fn collect_used_tokens(source: &str, out: &mut BTreeSet<String>) {
    let parse = syntax::parse(source);
    for element in parse.syntax_node().descendants_with_tokens() {
        if let syntax::SyntaxElement::Token(token) = element {
            if token.kind().is_trivia() {
                continue;
            }
            let text = token.text();
            out.insert(text.to_owned());
            if token.kind() == SyntaxKind::STRING {
                let content = text
                    .trim_start_matches(['"', '\''])
                    .trim_end_matches(['"', '\'']);
                if !content.is_empty() {
                    out.insert(content.to_owned());
                }
            }
        }
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

    fn stub_backed<'db>(
        db: &'db semantics::RootDatabase,
    ) -> (
        impl Fn(&str) -> bool + 'db,
        impl Fn(&str, &str) -> bool + 'db,
    ) {
        (
            move |namespace: &str| {
                semantics::stubs::namespace_known(db, namespace).unwrap_or(false)
            },
            move |namespace: &str, name: &str| {
                semantics::stubs::namespace_exports(db, namespace, name)
            },
        )
    }

    fn problems_for(source: &str) -> Vec<String> {
        let db = semantics::RootDatabase::default();
        semantics::stubs::install_shipped_stubs(&db);
        let (knows, exports) = stub_backed(&db);
        namespace_import_problems(&parse_namespace_imports(source), &knows, &exports)
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect()
    }

    #[test]
    fn known_namespace_with_real_exports_is_clean() {
        let problems = problems_for("import(stats)\nimportFrom(stats, sd, median)\n");
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn known_namespace_with_a_typo_warns() {
        let problems = problems_for("importFrom(stats, sd, medain)\n");
        assert_eq!(problems, ["`medain` is not exported by `stats`."]);
    }

    #[test]
    fn unknown_namespaces_are_not_checked() {
        let problems = problems_for("import(dplyr)\nimportFrom(dplyr, mutate)\n");
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn string_spellings_and_other_directives_parse() {
        let problems = problems_for(
            "export(run)\nS3method(print, thing)\nimportFrom(\"stats\", \"nope_not_real\")\n",
        );
        assert_eq!(problems, ["`nope_not_real` is not exported by `stats`."]);
    }

    fn unused_for(namespace: &str, sources: &[&str], level: LintLevel) -> Vec<String> {
        let imports = parse_namespace_imports(namespace);
        let mut used = BTreeSet::new();
        for source in sources {
            collect_used_tokens(source, &mut used);
        }
        unused_import_diagnostics(&imports, &used, level)
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect()
    }

    #[test]
    fn unused_import_is_flagged_when_opted_in() {
        let unused = unused_for(
            "importFrom(stats, sd, median)\n",
            &["x <- sd(values)\n"],
            LintLevel::Warn,
        );
        assert_eq!(
            unused,
            ["imported name `median` from `stats` is never used."]
        );
    }

    #[test]
    fn used_names_including_namespaced_and_operators_are_not_flagged() {
        let unused = unused_for(
            "importFrom(dplyr, mutate, filter)\nimportFrom(magrittr, \"%>%\")\n",
            &["out <- df %>% mutate(x = 1)\ny <- dplyr::filter(df, x > 0)\n"],
            LintLevel::Warn,
        );
        assert!(unused.is_empty(), "{unused:?}");
    }

    #[test]
    fn unused_import_is_silent_by_default() {
        let unused = unused_for(
            "importFrom(stats, median)\n",
            &["x <- 1\n"],
            LintLevel::Default,
        );
        assert!(unused.is_empty(), "{unused:?}");
    }
}
