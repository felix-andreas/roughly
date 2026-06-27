use {
    crate::{
        index::{Item, ItemInfo, SymbolsMap},
        lsp_types::{
            DocumentSymbol, Location, OneOf, Range, SymbolKind, Url as Uri, WorkspaceSymbol,
        },
    },
    analysis::{
        TextRange,
        ide::{MatchScore, search_match},
    },
    std::path::Path,
};

// `Item` ranges are byte columns; symbol output is encoded into the negotiated LSP encoding by the
// `to_lsp` converter the server boundary supplies (it carries the document rope and encoding).
pub fn document(items: &[Item], to_lsp: &impl Fn(TextRange) -> Range) -> Vec<DocumentSymbol> {
    items
        .iter()
        .filter(|item| !item.name.is_empty()) // lsp: doens't allow empty names
        .map(|item| to_document_symbol(item, to_lsp))
        .collect()
}

const WORKSPACE_SYMBOL_LIMIT: usize = 128;

pub fn workspace(
    query: &str,
    workspace_symbols: &impl SymbolsMap,
    to_lsp: &impl Fn(&Path, TextRange) -> Range,
) -> Vec<WorkspaceSymbol> {
    // Collect every subsequence match with its ranking score, then sort and truncate. Truncating
    // during collection (the previous behaviour) would drop good matches in arbitrary map order.
    let mut matches: Vec<(MatchScore, String, WorkspaceSymbol)> = workspace_symbols.filter_map(
        |path, symbols| {
            let uri = Uri::from_file_path(path).unwrap();
            symbols
                .iter()
                .flat_map(|symbol| {
                    std::iter::once((symbol, None)).chain(
                        symbol
                            .children
                            .as_ref()
                            .into_iter()
                            .flatten()
                            .map(|child| (child, Some(symbol.name.as_ref()))),
                    )
                })
                .filter_map(move |(symbol, container_name)| {
                    let name = symbol.display_name();
                    let score = search_match(&name, query)?;
                    Some((
                        score,
                        name.clone(),
                        WorkspaceSymbol {
                            name,
                            kind: to_symbol_kind(&symbol.info),
                            tags: None,
                            container_name: container_name.map(str::to_string),
                            location: OneOf::Left(Location {
                                uri: uri.clone(),
                                range: to_lsp(path, symbol.range),
                            }),
                            data: None,
                        },
                    ))
                })
        },
        usize::MAX,
    );

    matches.sort_by(|(left_score, left_name, _), (right_score, right_name, _)| {
        (left_score, left_name).cmp(&(right_score, right_name))
    });
    matches.truncate(WORKSPACE_SYMBOL_LIMIT);
    matches
        .into_iter()
        .map(|(_, _, symbol)| symbol)
        .collect()
}

pub fn to_document_symbol(item: &Item, to_lsp: &impl Fn(TextRange) -> Range) -> DocumentSymbol {
    DocumentSymbol {
        name: item.display_name(),
        kind: to_symbol_kind(&item.info),
        detail: item.detail.clone(),
        tags: None,
        range: to_lsp(item.range),
        selection_range: to_lsp(item.selection_range),
        children: item
            .children
            .as_ref()
            .map(|children| children.iter().map(|child| to_document_symbol(child, to_lsp)).collect()),
        #[allow(deprecated)]
        deprecated: None,
    }
}

fn to_symbol_kind(info: &ItemInfo) -> SymbolKind {
    match info {
        ItemInfo::Unknown => SymbolKind::VARIABLE,
        ItemInfo::Integer | ItemInfo::Float | ItemInfo::Complex => SymbolKind::NUMBER,
        ItemInfo::Bool => SymbolKind::BOOLEAN,
        ItemInfo::String => SymbolKind::STRING,
        ItemInfo::Null => SymbolKind::NULL,
        ItemInfo::Function => SymbolKind::FUNCTION,
        ItemInfo::S4Class => SymbolKind::CLASS,
        ItemInfo::S4Generic => SymbolKind::INTERFACE,
        ItemInfo::S4Method { .. } => SymbolKind::METHOD,
        ItemInfo::R6Class => SymbolKind::CLASS,
        ItemInfo::R6Method => SymbolKind::METHOD,
        ItemInfo::R6Field => SymbolKind::FIELD,
    }
}

#[cfg(test)]
mod workspace_tests {
    use {
        super::workspace,
        crate::{
            index::{Item, ItemInfo},
            lsp_types::{Position, Range},
        },
        analysis::{TextPosition, TextRange},
        std::{collections::HashMap, path::PathBuf},
    };

    fn zero_range() -> TextRange {
        TextRange {
            start: TextPosition {
                line_index: 0,
                character_index: 0,
            },
            end: TextPosition {
                line_index: 0,
                character_index: 0,
            },
        }
    }

    fn item(name: &str) -> Item {
        Item::new(
            name.to_owned(),
            None,
            zero_range(),
            zero_range(),
            None,
            ItemInfo::Function,
        )
    }

    fn names(query: &str, candidates: &[&str]) -> Vec<String> {
        let mut map: HashMap<PathBuf, Vec<Item>> = HashMap::new();
        map.insert(
            PathBuf::from("/pkg/R/main.R"),
            candidates.iter().map(|name| item(name)).collect(),
        );
        workspace(query, &map, &|_path, _range| {
            Range::new(Position::new(0, 0), Position::new(0, 0))
        })
        .into_iter()
        .map(|symbol| symbol.name)
        .collect()
    }

    #[test]
    fn subsequence_query_matches_missing_characters() {
        // the motivating case: "Istrumnt" should find "instrument"
        assert_eq!(
            names("istrumnt", &["instrument", "unrelated", "department"]),
            vec!["instrument"],
        );
    }

    #[test]
    fn results_are_ranked_by_match_quality() {
        // exact/prefix beat scattered subsequence regardless of map order
        let ranked = names(
            "inst",
            &["my_instrument", "reinstall", "install", "instrument", "inst"],
        );
        assert_eq!(
            ranked,
            vec!["inst", "install", "instrument", "reinstall", "my_instrument"],
        );
    }

    #[test]
    fn non_matching_symbols_are_excluded() {
        assert_eq!(names("xyz", &["instrument", "install"]), Vec::<String>::new());
    }
}
