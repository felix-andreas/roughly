use crate::{
    index::{Item, ItemInfo, SymbolsMap},
    lsp_types::{DocumentSymbol, Location, OneOf, SymbolKind, Url as Uri, WorkspaceSymbol},
    utils,
};

pub fn document(items: &[Item]) -> Vec<DocumentSymbol> {
    items.iter().map(to_document_symbol).collect()
}

pub fn workspace(query: &str, workspace_symbols: &impl SymbolsMap) -> Vec<WorkspaceSymbol> {
    workspace_symbols.filter_map(
        |path, symbols| {
            // Use unwrap here as paths should be valid file paths
            // TODO: Consider handling invalid paths more gracefully
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
                .filter(|(symbol, _)| utils::starts_with_lowercase(&symbol.display_name(), query))
                .map(move |(symbol, container_name)| WorkspaceSymbol {
                    name: symbol.display_name(),
                    kind: to_symbol_kind(&symbol.info),
                    tags: None,
                    container_name: container_name.map(str::to_string),
                    location: OneOf::Left(Location {
                        uri: uri.clone(),
                        range: symbol.range,
                    }),
                    data: None,
                })
        },
        128,
    )
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
