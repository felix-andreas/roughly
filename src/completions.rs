#[cfg(feature = "async-lsp")]
use crate::lsp_types::Url as Uri;
use {
    crate::{
        index,
        lsp_types::{
            CompletionItem, CompletionItemKind, CompletionResponse, DocumentSymbol, Position,
            SymbolKind,
        },
    },
    dashmap::DashMap,
    ropey::Rope,
};

pub fn get(
    position: Position,
    rope: &Rope,
    symbols_map: &DashMap<Uri, Vec<DocumentSymbol>>,
) -> Option<CompletionResponse> {
    // todo: proper error handling. make ropey, dashmap -> JSONRpc error

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

    // TODO: consider passing Some(&uri) to avoid showing local symbols twice ...
    let workspace_symbols = index::get_workspace_symbols(&query, symbols_map, 1000, None);

    // optimization would be to get all symbols for enclosing function
    // TODO: write code to get local completion items
    // let document_symbols = if let Some(document) = self.document_map.get(&uri) {
    //     let point = Point {
    //         row: position.line as usize,
    //         column: position.character as usize,
    //     };
    //     document
    //         .tree
    //         .root_node()
    //         .descendant_for_point_range(point, point)
    //         .and_then(|node| {
    //             let mut candidate = None;
    //             while let Some(node) = node.parent() {
    //                 if node.kind() == "function_definition" {
    //                     candidate = Some(node);
    //                 }
    //             }
    //             candidate.and_then(|function| function.child(2))
    //         })
    //         .map(|node| index::symbols_for_block(&node, &document.rope))
    //         .unwrap_or_default()
    // } else {
    //     tracing::error!("failed to aquirce document :/");
    //     vec![]
    // };

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
            .map(|reserved_word| CompletionItem {
                label: reserved_word.to_string(),
                kind: Some(CompletionItemKind::KEYWORD),
                ..Default::default()
            })
            // todo: do proper traversing
            // .chain(document_symbols.iter().fold(vec![], |mut symbols, symbol| {
            //     symbols.push(CompletionItem {
            //         label: symbol.name.clone(),
            //         kind: Some(match symbol.kind {
            //             SymbolKind::FUNCTION => CompletionItemKind::FUNCTION,
            //             SymbolKind::CLASS => CompletionItemKind::CLASS,
            //             SymbolKind::METHOD => CompletionItemKind::METHOD,
            //             _ => CompletionItemKind::VARIABLE,
            //         }),
            //         detail: None,
            //         ..Default::default()
            //     });
            //     symbols
            // }))
            .chain(workspace_symbols.into_iter().map(|symbol| CompletionItem {
                label: symbol.name,
                kind: Some(match symbol.kind {
                    SymbolKind::FUNCTION => CompletionItemKind::FUNCTION,
                    SymbolKind::CLASS => CompletionItemKind::CLASS,
                    SymbolKind::METHOD => CompletionItemKind::METHOD,
                    _ => CompletionItemKind::VARIABLE,
                }),
                detail: None,
                ..Default::default()
            }))
            .collect(),
    ))
}
