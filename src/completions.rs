use {
    crate::{
        index,
        index::SymbolsMap,
        lsp_types::{CompletionItem, CompletionItemKind, CompletionResponse, Position, SymbolKind},
        utils,
    },
    ropey::Rope,
    tree_sitter::Point,
};

pub fn get(
    position: Position,
    rope: &Rope,
    tree: &tree_sitter::Tree,
    symbols_map: &impl SymbolsMap,
) -> Option<CompletionResponse> {
    let line = rope.get_line(position.line as usize)?;
    let mut query = String::new();
    for (i, char) in line.chars().enumerate() {
        if char.is_alphabetic() || char == '.' || (!query.is_empty() && char.is_numeric()) {
            query.push(char)
        } else {
            query.clear();
        }
        if i == (position.character - 1) as usize {
            break;
        }
    }
    tracing::debug!("completion query: {query}");

    let symbol_kind_to_completion_kind = |kind: SymbolKind| match kind {
        SymbolKind::FUNCTION => CompletionItemKind::FUNCTION,
        SymbolKind::CLASS => CompletionItemKind::CLASS,
        SymbolKind::METHOD => CompletionItemKind::METHOD,
        _ => CompletionItemKind::VARIABLE,
    };

    let point = Point {
        row: position.line as usize,
        column: position.character as usize,
    };

    // TODO: filter for query
    let local_symbols: Vec<CompletionItem> = tree
        .root_node()
        .descendant_for_point_range(point, point)
        .map(|node| {
            std::iter::successors(Some(node), |node| node.parent())
                .filter(|n| n.kind() == "function_definition")
                .flat_map(|func_node| {
                    let mut items = Vec::new();

                    if let Some(parameters) = func_node.child_by_field_name("parameters") {
                        items.extend(
                            parameters
                                .children_by_field_name("parameter", &mut parameters.walk())
                                .filter_map(|parameter| {
                                    parameter.child_by_field_name("name").map(|name| {
                                        CompletionItem {
                                            label: rope
                                                .byte_slice(name.start_byte()..name.end_byte())
                                                .to_string(),
                                            kind: Some(CompletionItemKind::VARIABLE),
                                            ..Default::default()
                                        }
                                    })
                                }),
                        );
                    }

                    if let Some(body) = func_node.child_by_field_name("body") {
                        items.extend(index::symbols_for_block(body, rope).into_iter().map(
                            |symbol| CompletionItem {
                                label: symbol.name,
                                kind: Some(symbol_kind_to_completion_kind(symbol.kind)),
                                ..Default::default()
                            },
                        ));
                    }

                    items
                })
                .collect()
        })
        .unwrap_or_default();

    let workspace_symbols = symbols_map.filter_map(
        |_, symbols| {
            symbols
                .iter()
                .filter(|symbol| utils::starts_with_lowercase(&symbol.name, &query))
        },
        1024,
    );

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

    Some(CompletionResponse::Array(
        RESERVED_WORDS
            .iter()
            // TODO: filter keywords
            .map(|reserved_word| CompletionItem {
                label: reserved_word.to_string(),
                kind: Some(CompletionItemKind::KEYWORD),
                ..Default::default()
            })
            .chain(local_symbols)
            .chain(workspace_symbols.into_iter().map(|symbol| CompletionItem {
                label: symbol.name.to_string(),
                kind: Some(symbol_kind_to_completion_kind(symbol.kind)),
                ..Default::default()
            }))
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::tree,
        async_lsp::lsp_types::{CompletionItemKind, Position},
        indoc::indoc,
        ropey::Rope,
    };

    fn get_labels(
        text: &str,
        line: u32,
        character: u32,
    ) -> Vec<(String, Option<CompletionItemKind>)> {
        let rope = Rope::from_str(text);
        let tree = tree::parse(text, None);
        let dummy_symbols = std::collections::HashMap::new();
        let result = get(Position { line, character }, &rope, &tree, &dummy_symbols).unwrap();
        match result {
            async_lsp::lsp_types::CompletionResponse::Array(items) => items
                .into_iter()
                .map(|item| (item.label, item.kind))
                .collect(),
            _ => unreachable!(),
        }
    }

    #[test]
    fn completes_local_variables() {
        let text = indoc! {"
            foo <- function(x) {
                local_var <- 42
                loc
            }
        "};

        let labels = get_labels(text, 2, 7);

        assert!(labels.contains(&("local_var".into(), Some(CompletionItemKind::VARIABLE))));
    }

    #[test]
    fn completes_function_parameters() {
        let text = indoc! {"
            foo <- function(param1, param2) {
                par
            }
        "};

        let labels = get_labels(text, 1, 7);

        assert!(labels.contains(&("param1".into(), Some(CompletionItemKind::VARIABLE))));
        assert!(labels.contains(&("param2".into(), Some(CompletionItemKind::VARIABLE))));
    }

    #[test]
    fn completes_nested_function_variable() {
        let text = indoc! {"
            foo <- function(x) {
                var_outer <- 1
                bar <- function(y) {
                    var_inner <- 2
                    var
                }
            }
        "};

        let labels = get_labels(text, 4, 11);

        assert!(labels.contains(&("var_inner".into(), Some(CompletionItemKind::VARIABLE))));
        assert!(labels.contains(&("var_outer".into(), Some(CompletionItemKind::VARIABLE))));
    }
}
