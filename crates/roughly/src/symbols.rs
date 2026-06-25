use {
    crate::{
        index::{Item, ItemInfo, SymbolsMap},
        lsp_types::{DocumentSymbol, Location, OneOf, SymbolKind, Url as Uri, WorkspaceSymbol},
    },
    analysis::ide::{MatchScore, search_match},
};

pub fn document(items: &[Item]) -> Vec<DocumentSymbol> {
    items
        .iter()
        .filter(|item| !item.name.is_empty()) // lsp: doens't allow empty names
        .map(to_document_symbol)
        .collect()
}

const WORKSPACE_SYMBOL_LIMIT: usize = 128;

pub fn workspace(query: &str, workspace_symbols: &impl SymbolsMap) -> Vec<WorkspaceSymbol> {
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
                                range: symbol.range,
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

pub fn to_document_symbol(item: &Item) -> DocumentSymbol {
    DocumentSymbol {
        name: item.display_name(),
        kind: to_symbol_kind(&item.info),
        detail: item.detail.clone(),
        tags: None,
        range: item.range,
        selection_range: item.selection_range,
        children: item
            .children
            .as_ref()
            .map(|children| children.iter().map(to_document_symbol).collect()),
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
            lsp_types::Range,
        },
        std::{collections::HashMap, path::PathBuf},
    };

    fn item(name: &str) -> Item {
        Item::new(
            name.to_owned(),
            None,
            Range::default(),
            Range::default(),
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
        workspace(query, &map)
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
