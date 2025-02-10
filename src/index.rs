use {
    crate::utils,
    dashmap::DashMap,
    ropey::Rope,
    std::path::Path,
    tower_lsp::lsp_types::*,
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
    symbols_map: &DashMap<Url, Vec<DocumentSymbol>>,
    limit: usize,
    maybe_ignore_uri: Option<&Url>,
) -> Vec<SymbolInformation> {
    let workspace_symbols: Vec<_> = symbols_map
        .iter()
        .flat_map(|ref_multi| {
            let (url, symbols) = ref_multi.pair();
            match maybe_ignore_uri {
                Some(ignore_uri) if ignore_uri == url => vec![],
                _ => filter_symbols(query, url, symbols),
            }
        })
        .take(limit) // limit amount
        .collect();
    log::info!("get workspace symbols {}", workspace_symbols.len());
    workspace_symbols
}

pub fn get_document_symbols(
    url: &Url,
    symbols_map: &DashMap<Url, Vec<DocumentSymbol>>,
) -> Vec<SymbolInformation> {
    let Some(symbols) = symbols_map.get(url) else {
        log::info!("failed to acquire symbols map");
        // todo: understand when this happens
        return vec![];
    };
    filter_symbols("", url, &symbols)
}

fn filter_symbols(query: &str, url: &Url, symbols: &[DocumentSymbol]) -> Vec<SymbolInformation> {
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
                    uri: url.to_owned(),
                    range: symbol.range,
                },
                container_name: None,
            }
        })
        .collect::<Vec<SymbolInformation>>()
}

pub fn index_full(symbols_map: &DashMap<Url, Vec<DocumentSymbol>>) -> Result<(), ()> {
    let start = std::time::Instant::now();
    let mut n = 0;
    let cwd = std::env::current_dir().unwrap();
    if let Ok(entries) = std::fs::read_dir(cwd.join("R")) {
        for entry in entries {
            if let Ok(entry) = entry {
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
                let url = Url::from_file_path(path)?;
                symbols_map.insert(url, symbols);
            }
        }
    }

    log::info!(
        "build new index ({n} symbols) in {} ms",
        start.elapsed().as_millis()
    );
    Ok(())
}

pub fn index_update(symbols_map: &DashMap<Url, Vec<DocumentSymbol>>, url: &Url) {
    let start = std::time::Instant::now();
    let Ok(path) = url.to_file_path() else {
        log::error!("failed to get file path for {url}");
        return;
    };
    log::info!(
        "update index for {path:?} in {} ms",
        start.elapsed().as_millis()
    );
    let symbols = index_file(path);
    symbols_map.insert(url.clone(), symbols);
}

pub fn index_file(path: impl AsRef<Path>) -> Vec<DocumentSymbol> {
    let Ok(text) = std::fs::read_to_string(&path) else {
        log::info!("indexing: couldn't read file: '{:?}'!", path.as_ref());
        return vec![];
    };
    index(&text)
}

fn index(text: &str) -> Vec<DocumentSymbol> {
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
        let line = newline_positions.partition_point(|&x| token_start > x) as u32;
        let range = Range::new(
            Position::new(line, token_start as u32),
            Position::new(line, name.end() as u32),
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
        let line = newline_positions.partition_point(|&x| token_start > x) as u32;
        let range = Range::new(
            Position::new(line, kind.start() as u32),
            Position::new(line, kind.end() as u32),
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

// DIAGNOSTICS

pub async fn compute_diagnostics(uri: Url, _: &Rope) {
    log::debug!("compute diagnostics for {uri}");
    // let newlines = regex!(r#"\n"#);
    // let newline_positions = newlines
    //     .captures_iter(&rope)
    //     .map(|captures| captures.get(0).unwrap().start())
    //     .collect::<Vec<usize>>();

    // let assignments = regex!(r#"(?m)(\s*)([\w\.]+)\s*<-"#);
    // // todo: consider join_all from futures
    // let diagnostics = assignments.captures_iter(&rope).map(|captures| {
    //     let name = captures.get(1).unwrap();
    //     let kind = captures.get(2).unwrap();
    //     let token_start = name.start();
    //     let line = newline_positions.partition_point(|&x| token_start > x) as u32;
    //     let range = Range::new(
    //         Position::new(line, token_start as u32),
    //         Position::new(line, name.end() as u32),
    //     );
    // });
    // let mut diagnostics = HashMap::from([(uri.clone(), Vec::new())]);
    // https://github.com/jfecher/ante/blob/5f7446375bc1c6c94b44a44bfb89777c1437aaf5/ante-ls/src/main.rs#L252
}

// LOCAL SYMBOLS
pub fn get_document_symbols_ng(tree: &Tree, rope: &Rope) -> Vec<DocumentSymbol> {
    log::info!("parse symbols tree");
    let root = tree.root_node();

    symbols_for_block(&root, rope)
}

pub fn symbols_for_block(root: &Node, rope: &Rope) -> Vec<DocumentSymbol> {
    let mut cursor = root.walk();
    let mut symbols = vec![];

    for node in root.children(&mut cursor) {
        match node.kind() {
            "binary_operator" => {
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
                                kind,
                                // note: kind of braced expression is last expression
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
                        utils::rope_range_to_lsp_range(lhs.start_byte()..lhs.end_byte(), rope)
                            .unwrap();
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
            _ => {}
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

// todo: consider resusing global parser (maybe behind Mutex??)
pub fn parse(text: &str, maybe_tree: Option<&Tree>) -> Tree {
    let mut parser = tree_sitter::Parser::new();
    let language = tree_sitter_r::LANGUAGE;
    parser
        .set_language(&language.into())
        .expect("Error loading R parser");
    let tree = parser.parse(text, maybe_tree).unwrap();
    tree
}

#[cfg(test)]
mod test {
    use {
        super::{get_document_symbols_ng, index, parse},
        indoc::indoc,
        ropey::Rope,
        tower_lsp::lsp_types::SymbolKind,
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
        let symbols = get_document_symbols_ng(&parse(text, None), &Rope::from_str(text));

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
        assert_eq!(symbols[2].kind, SymbolKind::NUMBER);
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
