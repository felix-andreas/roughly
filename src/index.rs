#[cfg(feature = "async-lsp")]
use crate::lsp_types::Url as Uri;
#[cfg(feature = "tower-lsp")]
use uri_ext::UriExt;
use {
    crate::{lsp_types::*, tree, utils},
    ropey::Rope,
    std::{
        collections::HashMap,
        path::{Path, PathBuf},
    },
    tree_sitter::Node,
};

pub trait SymbolsMap {
    fn filter_map<'a, T, I>(
        &'a self,
        key: impl Fn(&'a PathBuf, &'a [DocumentSymbol]) -> I,
        limit: usize,
    ) -> Vec<T>
    where
        I: Iterator<Item = T>;
}

impl SymbolsMap for HashMap<PathBuf, Vec<DocumentSymbol>> {
    fn filter_map<'a, T, I>(
        &'a self,
        key: impl Fn(&'a PathBuf, &'a [DocumentSymbol]) -> I,
        limit: usize,
    ) -> Vec<T>
    where
        I: Iterator<Item = T>,
    {
        self.iter()
            .flat_map(|(path, symbols)| key(path, symbols))
            .take(limit) // limit amount
            .collect::<Vec<_>>()
    }
}

pub fn get_workspace_symbols(
    query: &str,
    workspace_symbols: &impl SymbolsMap,
) -> Vec<WorkspaceSymbol> {
    workspace_symbols.filter_map(
        |path, symbols| {
            let uri = Uri::from_file_path(path).unwrap();
            symbols
                .iter()
                .filter(|symbol| utils::starts_with_lowercase(&symbol.name, query))
                .map(move |symbol| to_workspace_symbol(symbol, &uri))
        },
        32,
    )
}

pub fn to_workspace_symbol(symbol: &DocumentSymbol, uri: &Uri) -> WorkspaceSymbol {
    WorkspaceSymbol {
        name: symbol.name.to_string(),
        kind: symbol.kind,
        tags: None,
        container_name: None,
        location: OneOf::Left(Location {
            uri: uri.clone(),
            range: symbol.range,
        }),
        data: None,
    }
}

#[derive(Debug)]
pub struct IndexError;

pub fn index_full(base_path: &Path) -> Result<Vec<(PathBuf, Vec<DocumentSymbol>)>, IndexError> {
    let start = std::time::Instant::now();

    let mut n = 0;
    let paths = std::fs::read_dir(base_path)
        .and_then(|read_dir| {
            read_dir
                .map(|entries| entries.map(|entry| entry.path()))
                .collect::<Result<Vec<_>, std::io::Error>>()
        })
        .map_err(|error| {
            tracing::error!(?error, "failed to index");
            IndexError
        })?;

    let symbols = paths
        .into_iter()
        .filter(|path| {
            path.is_file() && path.extension().is_some_and(|ext| ext == "R" || ext == "r")
        })
        .map(|path| {
            let symbols = index_file(&path);
            n += symbols.len();
            (path, symbols)
        })
        .collect::<Vec<_>>();

    tracing::info!(
        symbols = n,
        elapsed = start.elapsed().as_millis(),
        "build full index",
    );
    Ok(symbols)
}

pub fn index_file(path: impl AsRef<Path>) -> Vec<DocumentSymbol> {
    let Ok(rope) = utils::read_to_rope(&path) else {
        tracing::error!(message = "indexing: couldn't read file", path = %path.as_ref().display());
        return vec![];
    };

    let tree = tree::parse_rope(&rope, None);
    index(tree.root_node(), &rope, false)
}

pub fn index(root: Node, rope: &Rope, recursive: bool) -> Vec<DocumentSymbol> {
    let mut symbols = vec![];

    for node in root.children(&mut root.walk()) {
        // TODO: also handle function_definition
        if node.kind() == "binary_operator" {
            let maybe_lhs = node.child_by_field_name("lhs");
            let maybe_op = node.child_by_field_name("operator");
            let maybe_rhs = node.child_by_field_name("rhs");

            // TODO: recurse lhs and rhs in else case?
            if let Some(lhs) = maybe_lhs
                && lhs.kind() == "identifier"
                && maybe_op.is_some_and(|op| op.kind() == "<-")
            {
                let (kind, detail, children) = maybe_rhs
                    .map(|rhs| match rhs.kind() {
                        "function_definition" => {
                            let function = rhs;
                            let maybe_parameters = function.child_by_field_name("parameters");
                            let maybe_body = function.child_by_field_name("body");

                            let detail = maybe_parameters.map(|parameters| {
                                format!(
                                    "fn({})",
                                    parameters
                                        .children_by_field_name("parameter", &mut parameters.walk())
                                        .map(|parameter| match parameter.child(0) {
                                            Some(name) => {
                                                rope.byte_slice(name.byte_range()).to_string()
                                            }
                                            None => "UNKNOWN".into(),
                                        })
                                        .collect::<Vec<String>>()
                                        .join(", ")
                                )
                            });
                            let children = maybe_body
                                .and_then(|body| recursive.then(|| index(body, rope, recursive)));

                            (SymbolKind::FUNCTION, detail, children)
                        }
                        "braced_expression" => {
                            let block_symbols = index(rhs, rope, recursive);
                            symbols.extend(block_symbols);

                            (SymbolKind::VARIABLE, None, None)
                        }
                        "integer" | "float" | "complex" => (SymbolKind::NUMBER, None, None),
                        "true" | "false" => (SymbolKind::BOOLEAN, None, None),
                        "string" => (SymbolKind::STRING, None, None),
                        "null" => (SymbolKind::NULL, None, None),
                        _ => (SymbolKind::VARIABLE, None, None),
                    })
                    .unwrap_or_else(|| (SymbolKind::VARIABLE, None, None));

                #[allow(deprecated)]
                symbols.push(DocumentSymbol {
                    name: rope.byte_slice(lhs.byte_range()).to_string(),
                    kind,
                    detail,
                    tags: None,
                    range: utils::rope_range_to_lsp_range(node.byte_range(), rope).unwrap(),
                    selection_range: utils::rope_range_to_lsp_range(lhs.byte_range(), rope)
                        .unwrap(),
                    children,
                    deprecated: None,
                })
            }
        }
    }
    symbols
}

#[cfg(test)]
mod test {
    use {
        super::index,
        crate::{lsp_types::SymbolKind, tree},
        async_lsp::lsp_types::DocumentSymbol,
        indoc::indoc,
        ropey::Rope,
    };

    fn setup(text: &str, recursive: bool) -> Vec<DocumentSymbol> {
        let rope = Rope::from_str(text);
        let tree = tree::parse_rope(&rope, None);
        index(tree.root_node(), &rope, recursive)
    }

    fn setup_recursive(text: &str) -> Vec<DocumentSymbol> {
        setup(text, true)
    }

    // TODO: when to use setup flat??
    // fn setup_flat(text: &str) -> Vec<DocumentSymbol> {
    //     setup(text, false)
    // }

    #[test]
    fn test_parse() {
        let symbols = setup_recursive(indoc! {r#"
            foo <- function(a, b = True) {
                a <- TRUE
                b <- FALSE
            }
            bar <- \(x, y, z) {
                a <- 1
                b <- "foo"
            }
            baz <- { "foo"; 3.14 }
        "#});

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
        assert_eq!(symbols[2].kind, SymbolKind::VARIABLE);
    }

    #[test]
    fn test_set_class() {
        let symbols = setup_recursive(indoc! {r#"
            setClass(
                "Person",
                slots = c(
                    name = "character",
                    age = "numeric"
                )
            )
            setClass(
                "Car",
                slots = c(
                    name = "character"
                )
            )
        "#});

        assert_eq!(symbols[0].name, "Person");
        assert_eq!(symbols[0].kind, SymbolKind::CLASS);
        {
            let children = symbols[0].children.as_ref().unwrap();
            assert_eq!(children[0].name, "name");
            assert_eq!(children[0].kind, SymbolKind::PROPERTY);
            assert_eq!(children[1].name, "age");
            assert_eq!(children[1].kind, SymbolKind::PROPERTY);
        }

        assert_eq!(symbols[1].name, "Car");
        assert_eq!(symbols[1].kind, SymbolKind::CLASS);
        {
            let children = symbols[1].children.as_ref().unwrap();
            assert_eq!(children[0].name, "name");
            assert_eq!(children[0].kind, SymbolKind::PROPERTY);
        }

        assert_eq!(symbols[2].name, "age");
        assert_eq!(symbols[2].kind, SymbolKind::INTERFACE);

        assert_eq!(symbols[3].name, "age<-");
        assert_eq!(symbols[3].kind, SymbolKind::INTERFACE);

        assert_eq!(symbols[4].name, "age (Person)");
        assert_eq!(symbols[4].kind, SymbolKind::METHOD);

        assert_eq!(symbols[5].name, "age<- (Person)");
        assert_eq!(symbols[5].kind, SymbolKind::METHOD);

        assert_eq!(symbols.len(), 9);
    }

    #[test]
    fn test_set_generic() {
        let symbols = setup_recursive(indoc! {r#"
            setGeneric("foo", function(x) standardGeneric("foo"))
            setGeneric("bar<-", function(x, value) standardGeneric("bar<-"))
        "#});

        assert_eq!(symbols[0].name, "foo");
        assert_eq!(symbols[0].kind, SymbolKind::INTERFACE);

        assert_eq!(symbols[1].name, "bar<-");
        assert_eq!(symbols[1].kind, SymbolKind::INTERFACE);

        assert_eq!(symbols.len(), 2);
    }

    #[test]
    fn test_set_method() {
        let symbols = setup_recursive(indoc! {r#"
            setMethod("foo", "Person", function(x) x@foo)
            setMethod(
                "bar<-",
                "Person",
                function(x, value) {
                    x@bar <- value
                    x
                }
            )
        "#});

        assert_eq!(symbols[0].name, "foo (Person)");
        assert_eq!(symbols[0].kind, SymbolKind::METHOD);

        assert_eq!(symbols[1].name, "bar<- (Person)");
        assert_eq!(symbols[1].kind, SymbolKind::METHOD);

        assert_eq!(symbols.len(), 2);
    }

    #[test]
    fn test_set_method_with_signature_arg() {
        let symbols = setup_recursive(indoc! {r#"
            setMethod(
                f = "baz",
                signature = "Person",
                definition = function(x) x@baz
            )
        "#});

        assert_eq!(symbols[0].name, "baz (Person)");
        assert_eq!(symbols[0].kind, SymbolKind::METHOD);
        assert_eq!(symbols.len(), 1);
    }

    #[test]
    fn test_set_method_with_vector_signature() {
        let symbols = setup_recursive(indoc! {r#"
            setMethod(
                "qux",
                c("Person", "Other"),
                function(x, y) x@qux + y@qux
            )
        "#});

        assert_eq!(symbols[0].name, "qux (Person, Other)");
        assert_eq!(symbols[0].kind, SymbolKind::METHOD);
        assert_eq!(symbols.len(), 1);
    }

    // TODO: implement this in a follow up pr
    // #[test]
    // fn test_r6_class() {
    //     let symbols = setup_recursive(indoc! {r#"
    //         Person <- R6Class("Person",
    //             public = list(
    //                 name = NULL,
    //                 age = NULL,
    //                 initialize = function(name, age) {
    //                     self$name <- name
    //                     self$age <- age
    //                 },
    //                 greet = function() {
    //                     cat(paste("Hello, my name is", self$name))
    //                 },
    //                 say_age = function() {
    //                     cat(paste("I am", self$age, "years old"))
    //                 },
    //                 .hidden = NULL
    //             ),
    //             private = list(
    //                 secret = NULL,
    //                 password = NULL,
    //                 reveal_secret = function() {
    //                     cat(self$secret)
    //                 }
    //             ),
    //             active = list(
    //                 full_name = function(value) {
    //                     if (missing(value)) paste(self$name, "Smith") else self$name <- value
    //                 }
    //             ),
    //             inherit = AnotherClass,
    //             portable = TRUE,
    //             cloneable = FALSE,
    //             lock_class = TRUE,
    //             lock_objects = FALSE
    //         )
    //     "#});

    //     assert_eq!(symbols[0].name, "Person");
    //     assert_eq!(symbols[0].kind, SymbolKind::CLASS);

    //     let children = symbols[0].children.as_ref().unwrap();
    //     // Public properties and methods
    //     assert!(
    //         children
    //             .iter()
    //             .any(|c| c.name == "name" && c.kind == SymbolKind::PROPERTY)
    //     );
    //     assert!(
    //         children
    //             .iter()
    //             .any(|c| c.name == "age" && c.kind == SymbolKind::PROPERTY)
    //     );
    //     assert!(
    //         children
    //             .iter()
    //             .any(|c| c.name == "initialize" && c.kind == SymbolKind::METHOD)
    //     );
    //     assert!(
    //         children
    //             .iter()
    //             .any(|c| c.name == "greet" && c.kind == SymbolKind::METHOD)
    //     );
    //     assert!(
    //         children
    //             .iter()
    //             .any(|c| c.name == "say_age" && c.kind == SymbolKind::METHOD)
    //     );
    //     assert!(
    //         children
    //             .iter()
    //             .any(|c| c.name == ".hidden" && c.kind == SymbolKind::PROPERTY)
    //     );
    //     // Private properties and methods
    //     assert!(
    //         children
    //             .iter()
    //             .any(|c| c.name == "secret" && c.kind == SymbolKind::PROPERTY)
    //     );
    //     assert!(
    //         children
    //             .iter()
    //             .any(|c| c.name == "password" && c.kind == SymbolKind::PROPERTY)
    //     );
    //     assert!(
    //         children
    //             .iter()
    //             .any(|c| c.name == "reveal_secret" && c.kind == SymbolKind::METHOD)
    //     );
    //     // Active bindings
    //     assert!(
    //         children
    //             .iter()
    //             .any(|c| c.name == "full_name" && c.kind == SymbolKind::PROPERTY)
    //     );
    //     // Inheritance and options are not symbol children, but you could check metadata if supported
    // }
}
