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

pub fn index_dir(base_path: &Path) -> Result<Vec<(PathBuf, Vec<DocumentSymbol>)>, IndexError> {
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

pub fn index(root: Node, rope: &Rope, nested: bool) -> Vec<DocumentSymbol> {
    let mut symbols = vec![];

    // TODO: consider named children
    // TODO: consider tree::for_each_child
    for node in root.children(&mut root.walk()) {
        match node.kind() {
            "binary_operator" => {
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
                            "function_definition" => index_function(rhs, rope, nested),
                            "braced_expression" => {
                                let block_symbols = index(rhs, rope, nested);
                                symbols.extend(block_symbols);

                                (SymbolKind::VARIABLE, None, None)
                            }
                            "call" => todo!(),
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
            "braced_expression" => {
                symbols.extend(index(node, rope, nested));
            }
            "call" => {
                if let Some(symbol) = index_call(node, rope, nested) {
                    symbols.push(symbol)
                }
            }
            "function_definition" => {
                let (_, _, maybe_chidlren) = index_function(node, rope, nested);
                if let Some(children) = maybe_chidlren {
                    symbols.extend(children);
                }
            }
            "namespace_operator" => todo!(),
            // what about if, for, etc?
            // maybe need to implement recursion to find for edge cases?
            _ => {}
        }
    }
    symbols
}

fn index_function(
    function: Node,
    rope: &Rope,
    nested: bool,
) -> (SymbolKind, Option<String>, Option<Vec<DocumentSymbol>>) {
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
    let children = maybe_body.and_then(|body| nested.then(|| index(body, rope, nested)));
    (SymbolKind::FUNCTION, detail, children)
}

fn index_call(call: Node, rope: &Rope, nested: bool) -> Option<DocumentSymbol> {
    let maybe_function = call.child_by_field_name("function");
    let maybe_arguments = call.child_by_field_name("arguments");
    let (Some(function), Some(arguments)) = (maybe_function, maybe_arguments) else {
        return None;
    };

    let name = match function.kind() {
        "identifier" => rope.byte_slice(function.byte_range()).to_string(),
        "namespace_operator" => {
            let (Some(lhs), Some(rhs)) = (
                call.child_by_field_name("lhs"),
                call.child_by_field_name("rhs"),
            ) else {
                return None;
            };
            if lhs.kind() != "identifer" || rhs.kind() != "identifier" {
                return None;
            }
            let package = rope.byte_slice(lhs.byte_range()).to_string();
            if !["methods", "R6"].contains(&package.as_str()) {
                return None;
            }

            rope.byte_slice(rhs.byte_range()).to_string()
        }
        _ => return None,
    };

    let (kind, name, detail, children) = match name.as_str() {
        "setClass" => (
            SymbolKind::CLASS,
            "Class".into(),
            None,
            // TODO: maybe include slots?
            None,
        ),
        "setGeneric" => (SymbolKind::INTERFACE, "Generic".into(), None, None),
        "setMethod" => {
            // setMethod("foo", "Person", function(x) x@foo)
            // setMethod(f = "baz", signature = "Person", definition = function(x) x@baz)
            // setMethod("qux", c("Person", "Other"), function(x, y) x@qux + y@qux)

            // Helper to extract argument by name or position
            fn get_argument<'a>(
                arguments: Node<'a>,
                rope: &Rope,
                query: &str,
                pos: usize,
            ) -> Option<Node<'a>> {
                // Try named argument
                for argument in arguments.children_by_field_name("argument", &mut arguments.walk())
                {
                    if let Some(name) = argument.child_by_field_name("name") {
                        let name = rope.byte_slice(name.byte_range()).to_string();
                        if name == query {
                            return argument.child_by_field_name("value");
                        }
                    }
                }

                // TODO: maybe need to use naemd children because of comments?
                // Fallback to positional
                arguments
                    .children_by_field_name("argument", &mut arguments.walk())
                    .nth(pos)
                    .and_then(|argument| argument.child_by_field_name("value"))
            }

            let function_name_node = get_argument(arguments, rope, "f", 0);
            let signature_node = get_argument(arguments, rope, "signature", 1);

            let name = function_name_node
                .and_then(|n| {
                    if n.kind() == "string" {
                        Some(
                            rope.byte_slice(n.byte_range())
                                .to_string()
                                .trim_matches('"')
                                .to_string(),
                        )
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| "UNKNOWN".to_string());

            let signature = signature_node.and_then(|n| {
                match n.kind() {
                    "string" => Some(vec![
                        n.child_by_field_name("content")
                            .map(|content| rope.byte_slice(content.byte_range()).to_string())
                            .unwrap_or_default(),
                    ]),
                    "call" => {
                        // c("Person", "Other")
                        let mut sigs = vec![];
                        for child in n.children(&mut n.walk()) {
                            if child.kind() == "string" {
                                sigs.push(
                                    child
                                        .child_by_field_name("content")
                                        .map(|content| {
                                            rope.byte_slice(content.byte_range()).to_string()
                                        })
                                        .unwrap_or_default(),
                                );
                            }
                        }
                        if !sigs.is_empty() { Some(sigs) } else { None }
                    }
                    _ => None,
                }
            });

            let method_name = format!(
                "{} ({})",
                name,
                signature
                    .map(|sig| sig.join(", "))
                    .unwrap_or_else(|| "UNKNWON".into())
            );

            (SymbolKind::METHOD, method_name, None, None)
        }
        "R6Class" => todo!(),
        _ => return None,
    };

    let range = utils::rope_range_to_lsp_range(call.byte_range(), rope).unwrap();
    #[allow(deprecated)]
    Some(DocumentSymbol {
        name,
        kind,
        detail,
        tags: None,
        range,
        selection_range: range,
        children,
        deprecated: None,
    })
}

#[cfg(test)]
mod tests {
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

    fn setup_nested(text: &str) -> Vec<DocumentSymbol> {
        setup(text, true)
    }

    fn setup_flat(text: &str) -> Vec<DocumentSymbol> {
        setup(text, false)
    }

    #[test]
    fn assignments() {
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

        {
            let symbols = setup_nested(text);

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

        {
            let symbols = setup_flat(text);

            assert_eq!(symbols[0].name, "foo");
            assert_eq!(symbols[0].kind, SymbolKind::FUNCTION);
            assert_eq!(symbols[0].children, None);

            assert_eq!(symbols[1].name, "bar");
            assert_eq!(symbols[1].kind, SymbolKind::FUNCTION);
            assert_eq!(symbols[1].children, None);

            assert_eq!(symbols[2].name, "baz");
            assert_eq!(symbols[2].kind, SymbolKind::VARIABLE);
            assert_eq!(symbols[2].children, None);
        }
    }

    #[test]
    fn s4_set_class() {
        let symbols = setup_nested(indoc! {r#"
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
    fn s4_set_generic() {
        let symbols = setup_nested(indoc! {r#"
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
    fn s4_set_method() {
        let symbols = setup_flat(indoc! {r#"
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
    fn s4_set_method_with_signature_arg() {
        let symbols = setup_flat(indoc! {r#"
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
    fn s4_set_method_with_vector_signature() {
        let symbols = setup_flat(indoc! {r#"
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
