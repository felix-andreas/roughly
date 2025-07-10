use {
    crate::{
        index::SymbolsMap,
        lsp_types::{CompletionItem, CompletionItemKind, CompletionResponse, Position, SymbolKind},
        tree::{self, kind},
        utils,
    },
    async_lsp::lsp_types::CompletionItemLabelDetails,
    ropey::Rope,
    tree_sitter::{Node, Point, Query, StreamingIterator},
};

pub fn get(
    position: Position,
    rope: &Rope,
    tree: &tree_sitter::Tree,
    symbols_map: &impl SymbolsMap,
) -> Option<CompletionResponse> {
    let (context, query) = extract_query(position, rope)?;

    tracing::debug!(?context, ?query, "completion");

    match context {
        Context::Default => {}
        // note: proper autocompletion for fields, collection items, or namespace items would
        // require type inference. for now, we use a simple heuristic: show all identifiers
        // already used in the current file.
        Context::Field | Context::Item | Context::Namespace => {
            let ts_query = Query::new(
                &tree_sitter_r::LANGUAGE.into(),
                match context {
                    Context::Field => FIELD_QUERY,
                    Context::Item => ITEM_QUERY,
                    Context::Namespace => NAMESPACE_QUERY,
                    _ => unreachable!(),
                },
            )
            .unwrap();

            let mut cursor = tree::new_query_cursor();
            let mut matches = tree::matches_rope(&mut cursor, &ts_query, rope, tree.root_node());

            let mut completions = std::iter::from_fn(move || {
                matches.next().map(|m| {
                    let identifier = m.captures[0].node;
                    rope.byte_slice(identifier.byte_range()).to_string()
                })
            })
            // the len check ensures that the query itself does not appear in the list of completions
            .filter(|item| utils::starts_with_lowercase(item, &query) && item.len() != query.len())
            .collect::<Vec<_>>();
            completions.dedup();

            return Some(CompletionResponse::Array(
                completions
                    .into_iter()
                    .map(|item| CompletionItem {
                        label: item,
                        ..Default::default()
                    })
                    .collect(),
            ));
        }
        Context::MaybeNamespace => return None,
    }

    let symbol_kind_to_completion_kind = |kind: SymbolKind| match kind {
        SymbolKind::FUNCTION => CompletionItemKind::FUNCTION,
        SymbolKind::CLASS => CompletionItemKind::CLASS,
        SymbolKind::METHOD => CompletionItemKind::METHOD,
        _ => CompletionItemKind::VARIABLE,
    };

    let keyword_symbols = {
        const RESERVED_WORDS: &[&str] = &[
            "if",
            "else",
            "repeat",
            "while",
            "function",
            "for",
            "in",
            "next",
            "break",
            "TRUE",
            "FALSE",
            "NULL",
            "Inf",
            "NaN",
            "NA",
            "NA_integer_",
            "NA_real_",
            "NA_complex_",
            "NA_character_",
        ];
        RESERVED_WORDS
            .iter()
            .filter(|keyword| utils::starts_with_lowercase(keyword, &query))
            .map(|reserved_word| CompletionItem {
                label: reserved_word.to_string(),
                label_details: Some(CompletionItemLabelDetails {
                    detail: None,
                    description: Some("Keyword".into()),
                }),
                kind: Some(CompletionItemKind::KEYWORD),
                ..Default::default()
            })
    };

    let workspace_symbols = symbols_map
        .filter_map(
            |_, symbols| {
                symbols
                    .iter()
                    .filter(|symbol| utils::starts_with_lowercase(&symbol.name, &query))
            },
            // TODO: use CompletionResponse::List.is_incomplete and only limit for short queries?
            1024,
        )
        .into_iter()
        .map(|symbol| CompletionItem {
            label: symbol.name.to_string(),
            label_details: Some(CompletionItemLabelDetails {
                detail: None,
                description: Some("Global".into()),
            }),
            kind: Some(symbol_kind_to_completion_kind(symbol.kind)),
            ..Default::default()
        });

    let local_symbols: Vec<CompletionItem> =
        {
            let point = Point {
                row: position.line as usize,
                column: position.character as usize,
            };
            tree.root_node()
                .descendant_for_point_range(point, point)
                .map(|node| {
                    std::iter::successors(Some(node), |node| node.parent())
                        // note: we just search functions, as global symbols are already included from workspace info
                        .filter(|node| node.kind_id() == kind::FUNCTION_DEFINITION)
                        .flat_map(|node| {
                            let mut items = Vec::new();

                            if let Some(parameters) = node.child_by_field_name("parameters") {
                                items.extend(
                                    parameters
                                        .children_by_field_name("parameter", &mut parameters.walk())
                                        .filter_map(|parameter| {
                                            parameter.child_by_field_name("name").map(|name| {
                                                rope.byte_slice(name.byte_range()).to_string()
                                            })
                                        })
                                        .filter(|name| utils::starts_with_lowercase(name, &query))
                                        .map(|label| CompletionItem {
                                            label,
                                            label_details: Some(CompletionItemLabelDetails {
                                                detail: None,
                                                description: Some("Parameter".into()),
                                            }),
                                            kind: Some(CompletionItemKind::VARIABLE),
                                            ..Default::default()
                                        }),
                                );
                            }

                            if let Some(body) = node.child_by_field_name("body") {
                                items.extend(locals_completion(body, rope).into_iter().filter(
                                    |item| utils::starts_with_lowercase(&item.label, &query),
                                ));
                            }

                            items
                        })
                        .collect()
                })
                .unwrap_or_default()
        };

    Some(CompletionResponse::Array(
        keyword_symbols
            .chain(local_symbols)
            .chain(workspace_symbols)
            .collect(),
    ))
}

#[derive(Debug, PartialEq, Eq)]
enum Context {
    Default,
    Field,
    Item,
    Namespace,
    MaybeNamespace,
}

fn extract_query(position: Position, rope: &Rope) -> Option<(Context, String)> {
    let line = rope.get_line(position.line as usize)?;
    let mut previous = None;
    Some(line.chars().take(position.character as usize).fold(
        (Context::Default, String::new()),
        |(mut context, mut query), item| {
            // note: we can be less strict than R
            // see: https://cran.r-project.org/doc/manuals/r-release/R-lang.html#Identifiers-1
            if item.is_alphabetic()
                || item == '.'
                || item == '_'
                || (!query.is_empty() && item.is_numeric())
            {
                query.push(item);
            } else {
                context = match item {
                    '@' => Context::Field,
                    '$' => Context::Item,
                    ':' => {
                        if previous.is_some_and(|previous| previous == ':') {
                            Context::Namespace
                        } else {
                            Context::MaybeNamespace
                        }
                    }
                    _ => Context::Default,
                };
                query.clear();
            }
            previous = Some(item);
            (context, query)
        },
    ))
}

pub fn locals_completion(node: Node, rope: &Rope) -> Vec<CompletionItem> {
    let mut symbols = vec![];

    for child in node.named_children(&mut node.walk()) {
        match child.kind_id() {
            kind::BINARY_OPERATOR => {
                let maybe_lhs = child.child_by_field_name("lhs");
                let maybe_op = child.child_by_field_name("operator");

                if let Some(lhs) = maybe_lhs
                    && lhs.kind_id() == kind::IDENTIFIER
                    && maybe_op
                        .is_some_and(|op| [kind::EQUAL, kind::LEFT_ASSIGN].contains(&op.kind_id()))
                {
                    let label = rope.byte_slice(lhs.byte_range()).to_string();
                    symbols.push(CompletionItem {
                        label,
                        label_details: Some(CompletionItemLabelDetails {
                            detail: None,
                            description: Some("Local".into()),
                        }),
                        kind: Some(CompletionItemKind::VARIABLE),
                        ..Default::default()
                    })
                }
            }
            // note: we don't want to walk into sub scopes -> ignore function definition
            kind::FUNCTION_DEFINITION => continue,
            _ => {}
        }
        if child.child_count() > 0 {
            symbols.extend(locals_completion(child, rope));
        }
    }
    symbols
}

const FIELD_QUERY: &str = r#"(extract_operator operator: "@" rhs: (identifier) @ident)"#;
const ITEM_QUERY: &str = r#"(extract_operator operator: "$" rhs: (identifier) @ident)"#;
const NAMESPACE_QUERY: &str = r#"(namespace_operator rhs: (identifier) @ident)"#;

#[cfg(test)]
mod tests {
    use {super::*, crate::tree, indoc::indoc, ropey::Rope, std::collections::HashMap};

    fn setup(
        text: &str,
        (line, character): (u32, u32),
    ) -> (String, Vec<(String, CompletionItemKind)>) {
        let rope = Rope::from_str(text);

        let tree = tree::parse(&mut tree::new_parser(), text, None);
        let position = Position::new(line, character);
        let (_, query) = extract_query(position, &rope).unwrap();
        let items = match get(position, &rope, &tree, &HashMap::new()).unwrap() {
            CompletionResponse::Array(items) => items
                .into_iter()
                .map(|item| (item.label, item.kind.unwrap()))
                .collect(),
            _ => unreachable!(),
        };
        (query, items)
    }

    #[test]
    fn completes_local_variables() {
        let (query, items) = setup(
            indoc! {"
                function(x) {
                    var <- 42
                    va
                }
            "},
            (2, 6),
        );

        assert_eq!(query, "va");
        assert_eq!(items.len(), 1);
        assert!(items.contains(&("var".into(), CompletionItemKind::VARIABLE)));
    }

    #[test]
    fn completes_function_parameters() {
        let (query, items) = setup(
            indoc! {"
                function(param1, param2) {
                    par
                }
            "},
            (1, 7),
        );

        assert_eq!(query, "par");
        assert_eq!(items.len(), 2);
        assert!(items.contains(&("param1".into(), CompletionItemKind::VARIABLE)));
        assert!(items.contains(&("param2".into(), CompletionItemKind::VARIABLE)));
    }

    #[test]
    fn completes_nested_function_variable() {
        let (query, items) = setup(
            indoc! {"
                function(x) {
                    var_a <- 1
                    function(y) {
                        var_b <- 2
                        function(y) {
                            var_c <- 3
                            var
                        }
                    }
                }
            "},
            (6, 15),
        );

        assert_eq!(query, "var");
        assert_eq!(items.len(), 3);
        assert!(items.contains(&("var_a".into(), CompletionItemKind::VARIABLE)));
        assert!(items.contains(&("var_b".into(), CompletionItemKind::VARIABLE)));
        assert!(items.contains(&("var_c".into(), CompletionItemKind::VARIABLE)));
    }

    #[test]
    fn completes_keywords() {
        let (query, items) = setup("i", (0, 1));

        assert_eq!(query, "i");
        assert_eq!(items.len(), 3);
        assert!(items.contains(&("if".into(), CompletionItemKind::KEYWORD)));
        assert!(items.contains(&("in".into(), CompletionItemKind::KEYWORD)));
        assert!(items.contains(&("Inf".into(), CompletionItemKind::KEYWORD)));

        let (query, items) = setup("na_ ", (0, 3));

        assert_eq!(query, "na_");
        assert_eq!(items.len(), 4);
    }

    #[test]
    fn completes_block_variable() {
        let (query, items) = setup(
            indoc! {"
                function(x) {
                    {
                        var <- 1
                        va
                    }
                }
            "},
            (3, 10),
        );

        assert_eq!(query, "va");
        assert!(items.contains(&("var".into(), CompletionItemKind::VARIABLE)));
    }

    #[test]
    fn completes_if_statement_variable() {
        let (query, items) = setup(
            indoc! {"
                function(x) {
                    if (TRUE) {
                        var <- 1
                        va
                    }
                }
            "},
            (3, 10),
        );

        assert_eq!(query, "va");
        assert!(items.contains(&("var".into(), CompletionItemKind::VARIABLE)));
    }

    #[test]
    fn completes_for_loop_variable() {
        let (query, items) = setup(
            indoc! {"
                function(x) {
                    for (i in 1:3) {
                        var <- 1
                        va
                    }
                }
            "},
            (3, 10),
        );

        assert_eq!(query, "va");
        assert!(items.contains(&("var".into(), CompletionItemKind::VARIABLE)));
    }

    #[test]
    fn completes_while_loop_variable() {
        let (query, items) = setup(
            indoc! {"
                function(x) {
                    while (TRUE) {
                        var <- 1
                        va
                    }
                }
            "},
            (3, 10),
        );

        assert_eq!(query, "va");
        assert!(items.contains(&("var".into(), CompletionItemKind::VARIABLE)));
    }

    #[test]
    fn completes_repeat_loop_variable() {
        let (query, items) = setup(
            indoc! {"
                function(x) {
                    repeat {
                        var <- 1
                        va
                    }
                }
            "},
            (3, 10),
        );

        assert_eq!(query, "va");
        assert!(items.contains(&("var".into(), CompletionItemKind::VARIABLE)));
    }

    #[test]
    fn completes_switch_statement_variable() {
        let (query, items) = setup(
            indoc! {"
                function(x) {
                    switch (
                        x,
                        a = {
                            var <- 1
                            va
                        }
                    )
                }
            "},
            (4, 14),
        );

        assert_eq!(query, "va");
        assert!(items.contains(&("var".into(), CompletionItemKind::VARIABLE)));
    }

    #[test]
    fn completes_nested_functions_scope_correctly() {
        let code = indoc! {"
            function(x) {
                function(param_a) {
                    var_a <- 1
                    va
                    pa
                }
                function(param_b) {
                    var_b <- 2
                    va
                    pa
                }
            }
        "};

        // Only `var_a` should be completed after "var" in the first nested function
        let (query, items) = setup(code, (3, 10));
        assert_eq!(query, "va");
        assert_eq!(items.len(), 1);
        assert!(items.contains(&("var_a".into(), CompletionItemKind::VARIABLE)));

        // Only `param_a` should be completed after "parm" in the first nested function
        let (query, items) = setup(code, (4, 10));
        assert_eq!(query, "pa");
        assert_eq!(items.len(), 1);
        assert!(items.contains(&("param_a".into(), CompletionItemKind::VARIABLE)));

        // Only `var_b` should be completed after "var" in the second nested function
        let (query, items) = setup(code, (8, 10));
        assert_eq!(query, "va");
        assert_eq!(items.len(), 1);
        assert!(items.contains(&("var_b".into(), CompletionItemKind::VARIABLE)));

        // Only `param_b` should be completed after "parm" in the second nested function
        let (query, items) = setup(code, (9, 10));
        assert_eq!(query, "pa");
        assert_eq!(items.len(), 1);
        assert!(items.contains(&("param_b".into(), CompletionItemKind::VARIABLE)));
    }

    #[test]
    fn extract_query_edge_cases() {
        fn setup(pos: u32, text: &str) -> (Context, String) {
            extract_query(Position::new(0, pos), &Rope::from_str(text)).unwrap()
        }

        // Default context
        assert_eq!(setup(4, "name"), (Context::Default, "name".to_string()));
        assert_eq!(setup(5, ".name"), (Context::Default, ".name".to_string()));
        assert_eq!(setup(5, "_name"), (Context::Default, "_name".to_string()));
        assert_eq!(setup(5, "1name"), (Context::Default, "name".to_string()));
        assert_eq!(setup(6, ".1name"), (Context::Default, ".1name".to_string()));
        assert_eq!(
            setup(9, "name._123"),
            (Context::Default, "name._123".to_string())
        );

        assert_eq!(setup(0, ""), (Context::Default, "".to_string()));
        assert_eq!(setup(1, " "), (Context::Default, "".to_string()));
        assert_eq!(setup(6, "expr +"), (Context::Default, "".to_string()));

        // Field context (@)
        assert_eq!(setup(4, "lhs@"), (Context::Field, "".to_string()));
        assert_eq!(setup(9, "lhs@field"), (Context::Field, "field".to_string()));

        // Item context ($)
        assert_eq!(setup(4, "lhs$"), (Context::Item, "".to_string()));
        assert_eq!(setup(8, "lhs$item"), (Context::Item, "item".to_string()));

        // Namespace context (::)
        assert_eq!(setup(9, "package::"), (Context::Namespace, "".to_string()));
        assert_eq!(
            setup(13, "package::item"),
            (Context::Namespace, "item".to_string())
        );
    }

    #[test]
    fn queries_compile() {
        // These should compile and not panic
        let _field = Query::new(&tree_sitter_r::LANGUAGE.into(), FIELD_QUERY).unwrap();
        let _item = Query::new(&tree_sitter_r::LANGUAGE.into(), ITEM_QUERY).unwrap();
        let _namespace = Query::new(&tree_sitter_r::LANGUAGE.into(), NAMESPACE_QUERY).unwrap();
    }
}
