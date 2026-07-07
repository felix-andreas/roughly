// R package `NAMESPACE` files are plain R call syntax (`import(pkg)`, `importFrom(pkg, name)`),
// so they parse with the ordinary grammar; this module recognizes the import directives and
// validates the imported names against the stub corpus's namespaces. Resolution semantics are
// deliberately unchanged — stubbed names already resolve bare — so the value here is catching
// import typos against namespaces the stubs actually know.
//
// The whole module is interner-free (it works on the raw import name strings and a string-keyed
// export table), so a live editor buffer can be validated without touching the shared interner —
// the same isolation the `.Rtypes` stub-buffer path keeps.
use {
    crate::{
        diagnostic::Diagnostic,
        tree::{self, kind},
    },
    std::collections::{BTreeMap, BTreeSet},
    tree_sitter::{Node, Range},
};

pub struct NamespaceImport {
    pub namespace: String,
    // `None` for a whole-namespace `import(pkg)`; `Some` for one `importFrom(pkg, name)` name.
    pub name: Option<String>,
    pub range: Range,
}

// The `import`/`importFrom` directives of a NAMESPACE source, in file order. Directives R would
// reject (a malformed file, non-name arguments) are skipped rather than reported: R itself is the
// authority on NAMESPACE syntax, and this pass only wants the import facts.
pub fn parse_namespace_imports(source: &str) -> Vec<NamespaceImport> {
    let Ok(mut parser) = tree::new_parser() else {
        return Vec::new();
    };
    let Some(parsed) = parser.parse(source, None) else {
        return Vec::new();
    };
    let root = parsed.root_node();

    let mut imports = Vec::new();
    let mut cursor = root.walk();
    for node in root.named_children(&mut cursor) {
        if node.kind_id() != kind::CALL {
            continue;
        }
        let Some(callee) = node.child_by_field_name("function") else {
            continue;
        };
        if callee.kind_id() != kind::IDENTIFIER {
            continue;
        }
        let Some(arguments) = node.child_by_field_name("arguments") else {
            continue;
        };
        let mut argument_cursor = arguments.walk();
        let values: Vec<Node> = arguments
            .named_children(&mut argument_cursor)
            .filter(|argument| argument.kind_id() == kind::ARGUMENT)
            .filter_map(|argument| argument.child_by_field_name("value"))
            .collect();
        match &source[callee.byte_range()] {
            "import" => {
                // `import(pkg, ...)` may list several namespaces; `except = ...` keyword
                // arguments are not name values and fall out of the extraction below.
                for value in values {
                    if let Some((namespace, range)) = name_argument(value, source) {
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
                let Some((namespace, _)) =
                    values.next().and_then(|value| name_argument(value, source))
                else {
                    continue;
                };
                for value in values {
                    if let Some((name, range)) = name_argument(value, source) {
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

// One warning per `importFrom(pkg, name)` whose namespace the export table knows but whose name it
// does not list — the same fact `pkg::name` validation checks, surfaced at the import site.
// Unknown namespaces produce nothing: without stubs there is no export set to check against.
pub fn namespace_import_problems(
    imports: &[NamespaceImport],
    exports_by_namespace: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<Diagnostic> {
    imports
        .iter()
        .filter_map(|import| {
            let name = import.name.as_ref()?;
            let exports = exports_by_namespace.get(&import.namespace)?;
            if exports.contains(name) {
                return None;
            }
            Some(Diagnostic::naming_warning(
                import.range,
                format!("`{name}` is not exported by `{}`.", import.namespace),
            ))
        })
        .collect()
}

// A directive argument that names something: a bare identifier or a string literal (R accepts
// both spellings in NAMESPACE files).
fn name_argument(value: Node<'_>, source: &str) -> Option<(String, Range)> {
    match value.kind_id() {
        kind::IDENTIFIER => Some((source[value.byte_range()].to_owned(), value.range())),
        kind::STRING => {
            let mut cursor = value.walk();
            let content = value
                .named_children(&mut cursor)
                .find(|child| child.kind_id() == kind::STRING_CONTENT)?;
            Some((source[content.byte_range()].to_owned(), value.range()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use {super::*, crate::stdlib::stub_names_by_namespace};

    fn problems_for(source: &str) -> Vec<String> {
        let exports = stub_names_by_namespace(&[]);
        namespace_import_problems(&parse_namespace_imports(source), &exports)
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
}
