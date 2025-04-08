use {
    crate::{
        lsp_types::*,
        utils::{self, UriExt},
    },
    dashmap::DashMap,
    ropey::Rope,
    std::path::Path,
    tree_sitter::{Node, Tree},
};

macro_rules! regex {
    ($re:literal $(,)?) => {{
        use {regex::Regex, std::sync::OnceLock};

        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new($re).unwrap())
    }};
}

pub fn get_workspace_symbols(
    query: &str,
    symbols_map: &DashMap<Uri, Vec<DocumentSymbol>>,
    limit: usize,
    maybe_ignore_uri: Option<&Uri>,
) -> Vec<SymbolInformation> {
    let workspace_symbols: Vec<_> = symbols_map
        .iter()
        .flat_map(|ref_multi| {
            let (uri, symbols) = ref_multi.pair();
            match maybe_ignore_uri {
                Some(ignore_uri) if ignore_uri == uri => vec![],
                _ => filter_symbols(query, uri, symbols),
            }
        })
        .take(limit) // limit amount
        .collect();
    tracing::info!("get workspace symbols {}", workspace_symbols.len());
    workspace_symbols
}

pub fn get_document_symbols(
    uri: &Uri,
    symbols_map: &DashMap<Uri, Vec<DocumentSymbol>>,
) -> Vec<SymbolInformation> {
    let Some(symbols) = symbols_map.get(uri) else {
        tracing::info!("indexing: failed to acquire symbols map");
        // todo: understand when this happens
        return vec![];
    };
    filter_symbols("", uri, &symbols)
}

fn filter_symbols(query: &str, uri: &Uri, symbols: &[DocumentSymbol]) -> Vec<SymbolInformation> {
    symbols
        .iter()
        .filter(|symbol| {
            query.is_empty()
                || symbol
                    .name
                    .to_lowercase()
                    .starts_with(&query.to_lowercase())
        })
        .map(|symbol| {
            #[allow(deprecated)]
            SymbolInformation {
                name: symbol.name.to_string(),
                kind: symbol.kind,
                tags: None,
                deprecated: None,
                location: Location {
                    uri: uri.to_owned(),
                    range: symbol.range,
                },
                container_name: None,
            }
        })
        .collect::<Vec<SymbolInformation>>()
}

#[derive(Debug)]
pub struct IndexError;

pub fn index_full(symbols_map: &DashMap<Uri, Vec<DocumentSymbol>>) -> Result<(), IndexError> {
    let start = std::time::Instant::now();
    let mut n = 0;
    let cwd = std::env::current_dir().unwrap();
    if let Ok(entries) = std::fs::read_dir(cwd.join("R")) {
        for entry in entries.flatten() {
            let path = entry.path();

            if !path.is_file() {
                continue;
            }

            let Some(os_str) = path.extension() else {
                continue;
            };
            let Some("r" | "R") = os_str.to_str() else {
                continue;
            };

            let symbols = index_file(&path);
            n += symbols.len();
            let uri = utils::uri_from_file_path(&path).ok_or(IndexError)?;
            symbols_map.insert(uri, symbols);
        }
    }

    tracing::info!(
        "build new index ({n} symbols) in {} ms",
        start.elapsed().as_millis()
    );
    Ok(())
}

pub fn index_update(symbols_map: &DashMap<Uri, Vec<DocumentSymbol>>, uri: &Uri, text: &str) {
    let path = uri.to_file_path();

    let start = std::time::Instant::now();
    let symbols = index(text);
    tracing::info!(
        "update index for {path:?} in {} ms",
        start.elapsed().as_millis()
    );

    symbols_map.insert(uri.clone(), symbols);
}

pub fn index_update_file(symbols_map: &DashMap<Uri, Vec<DocumentSymbol>>, uri: &Uri) {
    let path = uri.to_file_path();

    let Ok(text) = std::fs::read_to_string(&path) else {
        tracing::info!("indexing: couldn't read file: '{:?}'!", path.as_ref());
        return;
    };

    let symbols = index(&text);
    symbols_map.insert(uri.clone(), symbols);
}

pub fn index_file(path: impl AsRef<Path>) -> Vec<DocumentSymbol> {
    let Ok(text) = std::fs::read_to_string(&path) else {
        tracing::info!("indexing: couldn't read file: '{:?}'!", path.as_ref());
        return vec![];
    };
    index(&text)
}

pub fn index(text: &str) -> Vec<DocumentSymbol> {
    let newlines = regex!(r#"\n"#);
    let newline_positions = newlines
        .captures_iter(text)
        .map(|captures| captures.get(0).unwrap().start())
        .collect::<Vec<usize>>();

    // todo: consider removing comments (gets tricky positions :/)
    // could replace comments with whitespace

    let globals = regex!(r#"(?m)^([\w\.]+)\s*<-\s*(\\\(|function|\S)"#);
    let symbols_globals = globals.captures_iter(text).map(|captures| {
        let name = captures.get(1).unwrap();
        let kind = captures.get(2).unwrap();
        let token_start = name.start();
        let line = newline_positions.partition_point(|&x| token_start > x);
        let line_start = if line == 0 {
            0
        } else {
            newline_positions[line - 1] + 1
        };

        let range = Range::new(
            Position::new(line as u32, (token_start - line_start) as u32),
            Position::new(line as u32, (name.end() - line_start) as u32),
        );

        #[allow(deprecated)]
        DocumentSymbol {
            name: name.as_str().to_string(),
            detail: None,
            kind: match kind.as_str() {
                "\\(" | "function" => SymbolKind::FUNCTION,
                _ => SymbolKind::VARIABLE,
            },
            tags: None,
            deprecated: None,
            range,
            selection_range: range,
            children: None,
        }
    });

    let s4 = regex!(
        r#"(?m)(setClass|setGeneric|setMethod)\s*\(\s*["']([\w\.<-]+)["']\s*,\s*(?:["']([\w\.]+)["'])?"#
    );
    let symbolds_s4 = s4.captures_iter(text).map(|captures| {
        let kind = captures.get(1).unwrap();
        let first = captures.get(2).unwrap();
        let second = captures.get(3);

        let token_start = kind.start();
        let line = newline_positions.partition_point(|&x| token_start > x);
        let line_start = if line == 0 {
            0
        } else {
            newline_positions[line - 1] + 1
        };

        let range = Range::new(
            Position::new(line as u32, (kind.start() - line_start) as u32),
            Position::new(line as u32, (kind.end() - line_start) as u32),
        );

        let (name, kind) = match kind.as_str() {
            "setClass" => (first.as_str().to_string(), SymbolKind::CLASS),
            "setGeneric" => (first.as_str().to_string(), SymbolKind::INTERFACE),
            "setMethod" => (
                format!(
                    "{} ({})",
                    first.as_str(),
                    second.map(|m| m.as_str()).unwrap_or("UNKNOWN")
                ),
                SymbolKind::METHOD,
            ),
            _ => ("UNKNOWN".to_string(), SymbolKind::VARIABLE),
        };

        #[allow(deprecated)]
        DocumentSymbol {
            name,
            detail: None,
            kind,
            tags: None,
            deprecated: None,
            range,
            selection_range: range,
            children: None,
        }
    });

    symbols_globals.chain(symbolds_s4).collect()
}

// LOCAL SYMBOLS
pub fn get_document_symbols_ng(tree: &Tree, rope: &Rope) -> Vec<DocumentSymbol> {
    tracing::info!("parse symbols tree");
    let root = tree.root_node();

    symbols_for_block(&root, rope)
}

pub fn symbols_for_block(root: &Node, rope: &Rope) -> Vec<DocumentSymbol> {
    let mut cursor = root.walk();
    let mut symbols = vec![];

    for node in root.children(&mut cursor) {
        if node.kind() == "binary_operator" {
            let lhs = node.child(0).unwrap();
            let op = node.child(1).unwrap();
            let rhs = node.child(2).unwrap();
            // todo: fix this
            if lhs.kind() == "identifier" && op.kind() == "<-" {
                let (kind, detail, children) = match rhs.kind() {
                    "function_definition" => {
                        let (children, detail) = parse_function(&rhs, rope);
                        (SymbolKind::FUNCTION, detail, Some(children))
                    }
                    "program" | "braced_expression" => {
                        let block_symbols = symbols_for_block(&rhs, rope);
                        let kind = block_symbols
                            .last()
                            .map(|symbol| symbol.kind)
                            .unwrap_or(SymbolKind::NULL);
                        symbols.extend(block_symbols);

                        (
                            kind, // note: kind of braced expression is last expression
                            None, None,
                        )
                    }
                    "integer" | "float" | "complex" => (SymbolKind::NUMBER, None, None),
                    "true" | "false" => (SymbolKind::BOOLEAN, None, None),
                    "string" => (SymbolKind::STRING, None, None),
                    "null" => (SymbolKind::NULL, None, None),
                    _ => (SymbolKind::VARIABLE, None, None),
                };
                let range =
                    utils::rope_range_to_lsp_range(lhs.start_byte()..lhs.end_byte(), rope).unwrap();
                symbols.push(
                    #[allow(deprecated)]
                    DocumentSymbol {
                        name: rope
                            .byte_slice(lhs.start_byte()..lhs.end_byte())
                            .to_string(),
                        kind,
                        detail,
                        tags: None,
                        range,
                        selection_range: range,
                        children,
                        deprecated: None,
                    },
                )
            }
        }
    }
    symbols
}

pub fn parse_function(function: &Node, rope: &Rope) -> (Vec<DocumentSymbol>, Option<String>) {
    let (parameters, body) = (function.child(1).unwrap(), function.child(2).unwrap());
    let symbols = symbols_for_block(&body, rope);
    let mut cursor = parameters.walk();
    let detail = parameters
        .children_by_field_name("parameter", &mut cursor)
        .map(|parameter| match parameter.child(0) {
            Some(name) => rope
                .byte_slice(name.start_byte()..name.end_byte())
                .to_string(),
            None => "UNKNOWN".into(),
        })
        .collect::<Vec<String>>()
        .join(", ");
    (symbols, Some(detail))
}

#[cfg(test)]
mod test {
    use {
        super::{get_document_symbols_ng, index},
        crate::{lsp_types::SymbolKind, tree},
        indoc::indoc,
        ropey::Rope,
    };

    #[test]
    fn test_parse() {
        let text = indoc! {r#"
            foo <- function(a, b = True) {
                a <- TRUE
                b <- FALSE
            }
            bar <- \(x, y, z) {
                a <- 1
                b <- "foo"
            }
            baz <- { "foo"; 3.14 }
        "#};
        let symbols = get_document_symbols_ng(&tree::parse(text, None), &Rope::from_str(text));

        assert_eq!(symbols[0].name, "foo");
        assert_eq!(symbols[0].kind, SymbolKind::FUNCTION);
        {
            let children = symbols[0].children.as_ref().unwrap();
            assert_eq!(children[0].name, "a");
            assert_eq!(children[0].kind, SymbolKind::BOOLEAN);
            assert_eq!(children[1].name, "b");
            assert_eq!(children[1].kind, SymbolKind::BOOLEAN);
        }

        assert_eq!(symbols[1].name, "bar");
        assert_eq!(symbols[1].kind, SymbolKind::FUNCTION);
        {
            let children = symbols[1].children.as_ref().unwrap();
            assert_eq!(children[0].name, "a");
            assert_eq!(children[0].kind, SymbolKind::NUMBER);
            assert_eq!(children[1].name, "b");
            assert_eq!(children[1].kind, SymbolKind::STRING);
        }

        assert_eq!(symbols[2].name, "baz");
        // TODO: make this work
        // assert_eq!(symbols[2].kind, SymbolKind::NUMBER);
    }

    #[test]
    fn test_indexing() {
        let symbols = index(indoc! {r#"
			foo <- function() {}
			bar <- \(x) {}
			baz <- 4
            setClass("Person",
                slots = c(
                    name = "character",
                    age = "numeric"
                )
            )
            setClass(
            "Car", slots = c(
                name = 
            "character"))
            setGeneric("age", function(x) standardGeneric("age"))
            setGeneric("age<-", function(x, value) standardGeneric("age<-"))
            setMethod("age", "Person", function(x) x@age)
            setMethod("age<-", "Person", function(x, value) {
                x@age <- value
                x
            })
		"#});

        assert_eq!(symbols[0].name, "foo");
        assert_eq!(symbols[0].kind, SymbolKind::FUNCTION);

        assert_eq!(symbols[1].name, "bar");
        assert_eq!(symbols[1].kind, SymbolKind::FUNCTION);

        assert_eq!(symbols[2].name, "baz");
        assert_eq!(symbols[2].kind, SymbolKind::VARIABLE);

        assert_eq!(symbols[3].name, "Person");
        assert_eq!(symbols[3].kind, SymbolKind::CLASS);

        assert_eq!(symbols[4].name, "Car");
        assert_eq!(symbols[4].kind, SymbolKind::CLASS);

        assert_eq!(symbols[5].name, "age");
        assert_eq!(symbols[5].kind, SymbolKind::INTERFACE);

        assert_eq!(symbols[6].name, "age<-");
        assert_eq!(symbols[6].kind, SymbolKind::INTERFACE);

        assert_eq!(symbols[7].name, "age (Person)");
        assert_eq!(symbols[7].kind, SymbolKind::METHOD);

        assert_eq!(symbols[8].name, "age<- (Person)");
        assert_eq!(symbols[8].kind, SymbolKind::METHOD);

        assert_eq!(symbols.len(), 9);
    }
}
