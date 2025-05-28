use {
    crate::{
        index::{self, SymbolsMap},
        lsp_types::{CompletionItem, CompletionItemKind, CompletionResponse, Position, SymbolKind},
        utils,
    },
    async_lsp::lsp_types::{CompletionItemLabelDetails, InsertTextFormat},
    ropey::Rope,
    tree_sitter::Point,
};

pub fn get(
    position: Position,
    rope: &Rope,
    tree: &tree_sitter::Tree,
    symbols_map: &impl SymbolsMap,
) -> Option<CompletionResponse> {
    let query = extract_query(position, rope)?;

    tracing::debug!("completion query: {query}");

    let symbol_kind_to_completion_kind = |kind: SymbolKind| match kind {
        SymbolKind::FUNCTION => CompletionItemKind::FUNCTION,
        SymbolKind::CLASS => CompletionItemKind::CLASS,
        SymbolKind::METHOD => CompletionItemKind::METHOD,
        _ => CompletionItemKind::VARIABLE,
    };

    let local_symbols: Vec<CompletionItem> = {
        let point = Point {
            row: position.line as usize,
            column: position.character as usize,
        };
        tree.root_node()
            .descendant_for_point_range(point, point)
            .map(|node| {
                // TODO: just use document children (requires to think about where to use nested and where not)
                std::iter::successors(Some(node), |node| node.parent())
                    .filter(|node| node.kind() == "function_definition")
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
                            items.extend(
                                // todo: use enable nested and use children
                                index::index(body, rope, false)
                                    .into_iter()
                                    .filter(|symbol| {
                                        utils::starts_with_lowercase(&symbol.name, &query)
                                    })
                                    .map(|symbol| CompletionItem {
                                        label: symbol.name,
                                        label_details: Some(CompletionItemLabelDetails {
                                            detail: None,
                                            description: Some("Local".into()),
                                        }),
                                        kind: Some(symbol_kind_to_completion_kind(symbol.kind)),
                                        ..Default::default()
                                    }),
                            );
                        }

                        items
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    let workspace_symbols = symbols_map.filter_map(
        |_, symbols| {
            symbols
                .iter()
                .filter(|symbol| utils::starts_with_lowercase(&symbol.name, &query))
        },
        // TODO: use CompletionResponse::List.is_incomplete and only limit for short queries?
        1024,
    );

    const RESERVED_WORDS: &[&str] = &[
        "if",
        "else",
        "repeat",
        "while",
        "function",
        // "for",
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

    Some(CompletionResponse::Array(
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
            .chain(std::iter::once(CompletionItem {
                label: "for".into(),
                insert_text: Some("for (${1:x} in $2) {\n${3:body}\n}".into()),
                insert_text_format: Some(InsertTextFormat::SNIPPET),
                ..Default::default()
            }))
            .chain(local_symbols)
            .chain(workspace_symbols.into_iter().map(|symbol| CompletionItem {
                label: symbol.name.to_string(),
                label_details: Some(CompletionItemLabelDetails {
                    detail: None,
                    description: Some("Global".into()),
                }),
                kind: Some(symbol_kind_to_completion_kind(symbol.kind)),
                ..Default::default()
            }))
            .collect(),
    ))
}

// TODO: consider throwing error instead of optional
// TODO: alternatively use tree-sitter to extract nearest identifer (to avoid reimplementing nearest identifer)
fn extract_query(position: Position, rope: &Rope) -> Option<String> {
    let line = rope.get_line(position.line as usize)?;
    Some(
        line.chars()
            .take(position.character as usize)
            .fold(String::new(), |mut acc, item| {
                // see: https://cran.r-project.org/doc/manuals/r-release/R-lang.html#Identifiers-1
                // note: we can be less strict than R otherwise its already an parser
                if item.is_alphabetic()
                    || item == '.'
                    || item == '_'
                    || (!acc.is_empty() && item.is_numeric())
                {
                    acc.push(item);
                } else {
                    acc.clear();
                }
                acc
            }),
    )
}

#[cfg(test)]
mod tests {
    use {super::*, crate::tree, indoc::indoc, ropey::Rope, std::collections::HashMap};

    fn setup(
        text: &str,
        (line, character): (u32, u32),
    ) -> (String, Vec<(String, CompletionItemKind)>) {
        let rope = Rope::from_str(text);
        let tree = tree::parse(text, None);
        let position = Position::new(line, character);
        let query = extract_query(position, &rope).unwrap();
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
                    local_var <- 42
                    loc
                }
            "},
            (2, 7),
        );

        assert_eq!(query, "loc");
        assert_eq!(items.len(), 1);
        assert!(items.contains(&("local_var".into(), CompletionItemKind::VARIABLE)));
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
    fn extract_query_edge_cases() {
        fn setup(pos: u32, text: &str) -> String {
            extract_query(Position::new(0, pos), &Rope::from_str(text)).unwrap()
        }

        assert_eq!(setup(11, "foo.bar_123"), "foo.bar_123");
        assert_eq!(setup(4, ".foo"), ".foo");
        assert_eq!(setup(4, "1foo"), "foo");
        assert_eq!(setup(4, "_foo"), "_foo");
        assert_eq!(setup(5, ".1foo"), ".1foo");
    }
}
