use {
    crate::{lsp_types::Range, tree, utils},
    ropey::Rope,
    std::{
        collections::HashMap,
        path::{Path, PathBuf},
    },
    tree_sitter::{Node, Parser},
};

#[derive(Debug, Clone)]
pub struct Item {
    pub name: String,
    pub detail: Option<String>,
    pub range: Range,
    pub selection_range: Range,
    pub children: Option<Vec<Item>>,
    pub info: ItemInfo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemInfo {
    Unknown,
    // Primitives
    Integer,
    Float,
    Complex,
    Bool,
    String,
    Null,
    Function,
    // S4
    S4Class,
    S4Generic,
    S4Method { signature: String },
    // R6
    R6Class,
    R6Method,
    R6Field,
}

impl Item {
    pub fn new(
        name: String,
        detail: Option<String>,
        range: Range,
        selection_range: Range,
        children: Option<Vec<Item>>,
        info: ItemInfo,
    ) -> Item {
        Item {
            name,
            detail,
            range,
            selection_range,
            children,
            info,
        }
    }

    pub fn display_name(&self) -> String {
        match &self.info {
            ItemInfo::S4Method { signature } => format!("{} ({})", self.name, signature),
            _ => self.name.clone(),
        }
    }
}

pub trait SymbolsMap {
    fn filter_map<'a, T, I>(
        &'a self,
        key: impl Fn(&'a PathBuf, &'a [Item]) -> I,
        limit: usize,
    ) -> Vec<T>
    where
        I: Iterator<Item = T>;
}

impl SymbolsMap for HashMap<PathBuf, Vec<Item>> {
    fn filter_map<'a, T, I>(
        &'a self,
        key: impl Fn(&'a PathBuf, &'a [Item]) -> I,
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

#[derive(Debug)]
pub struct IndexError;

pub fn index_dir(
    base_path: &Path,
    parser: &mut Parser,
) -> Result<Vec<(PathBuf, Vec<Item>)>, IndexError> {
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
            let symbols = index_file(&path, parser);
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

pub fn index_file(path: impl AsRef<Path>, parser: &mut Parser) -> Vec<Item> {
    let Ok(rope) = utils::read_to_rope(&path) else {
        tracing::error!(message = "indexing: couldn't read file", path = %path.as_ref().display());
        return vec![];
    };

    let tree = tree::parse_rope(parser, &rope, None);
    index(tree.root_node(), &rope, false, false)
}

pub fn index(root: Node, rope: &Rope, nested: bool, other: bool) -> Vec<Item> {
    let mut symbols = vec![];

    for node in root.named_children(&mut root.walk()) {
        match node.kind() {
            "binary_operator" => {
                let maybe_lhs = node.child_by_field_name("lhs");
                let maybe_op = node.child_by_field_name("operator");
                let maybe_rhs = node.child_by_field_name("rhs");

                if let Some(lhs) = maybe_lhs
                    && lhs.kind() == "identifier"
                    && maybe_op.is_some_and(|op| op.kind() == "<-")
                {
                    let (info, detail, children) = maybe_rhs
                        .map(|rhs| match rhs.kind() {
                            "function_definition" => index_function(rhs, rope, nested),
                            "braced_expression" => {
                                let block_symbols = index(rhs, rope, nested, other);
                                symbols.extend(block_symbols);

                                (ItemInfo::Unknown, None, None)
                            }
                            "call" => {
                                if let Some(symbol) = index_call(rhs, rope, nested) {
                                    (symbol.info, symbol.detail, symbol.children)
                                } else {
                                    (ItemInfo::Unknown, None, None)
                                }
                            }
                            "integer" => (ItemInfo::Integer, None, None),
                            "float" => (ItemInfo::Float, None, None),
                            "complex" => (ItemInfo::Complex, None, None),
                            "true" | "false" => (ItemInfo::Bool, None, None),
                            "string" => (ItemInfo::String, None, None),
                            "null" => (ItemInfo::Null, None, None),
                            _ => (ItemInfo::Unknown, None, None),
                        })
                        .unwrap_or_else(|| (ItemInfo::Unknown, None, None));

                    let name = rope.byte_slice(lhs.byte_range()).to_string();
                    let range = utils::node_range(node);
                    let selection_range = utils::node_range(lhs);
                    symbols.push(Item::new(
                        name,
                        detail,
                        range,
                        selection_range,
                        children,
                        info,
                    ))
                } else if nested {
                    // TODO: implement recursive indexing for lhs and rhs in braced expressions
                    // when nested=true to capture more complex symbol definitions
                }
            }
            "braced_expression" => {
                symbols.extend(index(node, rope, nested, other));
            }
            "call" => {
                if let Some(symbol) = index_call(node, rope, nested) {
                    symbols.push(symbol)
                }
            }
            "function_definition" if nested => {
                let (_, _, maybe_chidlren) = index_function(node, rope, nested);
                if let Some(children) = maybe_chidlren {
                    symbols.extend(children);
                }
            }
            "if_statement" => {
                if let Some(consequence) = node.child_by_field_name("consequence") {
                    symbols.extend(index(consequence, rope, nested, other));
                }

                if let Some(alternative) = node.child_by_field_name("alternative") {
                    symbols.extend(index(alternative, rope, nested, other));
                }
            }
            "for_statement" | "repeat_statement" | "while_statement" if other => {
                if let Some(body) = node.child_by_field_name("body") {
                    symbols.extend(index(body, rope, nested, other));
                }
            }
            _ => {}
        }
    }
    symbols
}

fn index_function(
    function: Node,
    rope: &Rope,
    nested: bool,
) -> (ItemInfo, Option<String>, Option<Vec<Item>>) {
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
                    None => "Unknown".into(),
                })
                .collect::<Vec<String>>()
                .join(", ")
        )
    });
    let children = maybe_body.and_then(|body| nested.then(|| index(body, rope, nested, false)));
    (ItemInfo::Function, detail, children)
}

fn index_call(call: Node, rope: &Rope, nested: bool) -> Option<Item> {
    let maybe_function = call.child_by_field_name("function");
    let maybe_arguments = call.child_by_field_name("arguments");
    let (Some(function), Some(arguments)) = (maybe_function, maybe_arguments) else {
        return None;
    };

    let name = match function.kind() {
        "identifier" => rope.byte_slice(function.byte_range()).to_string(),
        "namespace_operator" => {
            let (Some(lhs), Some(rhs)) = (
                function.child_by_field_name("lhs"),
                function.child_by_field_name("rhs"),
            ) else {
                return None;
            };

            if lhs.kind() != "identifier" || rhs.kind() != "identifier" {
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

    let range = utils::node_range(call);
    match name.as_str() {
        "setClass" => {
            // setClass("Person", slots = c(name = "character", age = "numeric"))
            let class_name = get_argument(arguments, rope, "Class", 0)
                .and_then(|argument| {
                    (argument.kind() == "string").then(|| {
                        argument
                            .child_by_field_name("content")
                            .map(|content| rope.byte_slice(content.byte_range()).to_string())
                            .unwrap_or_default()
                    })
                })
                .unwrap_or_else(|| "Unknown".to_string());

            Some(Item::new(
                class_name,
                None,
                range,
                range,
                None,
                ItemInfo::S4Class,
            ))
        }
        "setGeneric" => {
            // setGeneric("foo", function(x) standardGeneric("foo"))
            let generic_name = get_argument(arguments, rope, "name", 0)
                .and_then(|argument| {
                    if argument.kind() == "string" {
                        argument
                            .child_by_field_name("content")
                            .map(|content| rope.byte_slice(content.byte_range()).to_string())
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| "Unknown".to_string());
            Some(Item::new(
                generic_name,
                None,
                range,
                range,
                None,
                ItemInfo::S4Generic,
            ))
        }
        "setMethod" => {
            // setMethod("foo", "Person", function(x) x@foo)
            // setMethod(f = "baz", signature = "Person", definition = function(x) x@baz)
            // setMethod("qux", c("Person", "Other"), function(x, y) x@qux + y@qux)
            let method_name = get_argument(arguments, rope, "f", 0)
                .and_then(|argument| {
                    if argument.kind() == "string" {
                        Some(
                            argument
                                .child_by_field_name("content")
                                .map(|content| rope.byte_slice(content.byte_range()).to_string())
                                .unwrap_or_default(),
                        )
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| "Unknown".to_string());

            let signature = get_argument(arguments, rope, "signature", 1)
                .and_then(|argument| {
                    match argument.kind() {
                        "string" => Some(
                            argument
                                .child_by_field_name("content")
                                .map(|content| rope.byte_slice(content.byte_range()).to_string())
                                .unwrap_or_default(),
                        ),
                        "call" => {
                            // c("Person", "Other") or c(signature = "Person", other = "Other")
                            argument.child_by_field_name("arguments").map(|arguments| {
                                arguments
                                    .children_by_field_name("argument", &mut arguments.walk())
                                    .map(|argument| {
                                        argument
                                            .child_by_field_name("value")
                                            .and_then(|value| {
                                                (value.kind() == "string").then(|| {
                                                    value
                                                        .child_by_field_name("content")
                                                        .map(|content| {
                                                            rope.byte_slice(content.byte_range())
                                                                .to_string()
                                                        })
                                                        .unwrap_or_default()
                                                })
                                            })
                                            .unwrap_or_else(|| "Unknown".to_string())
                                    })
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            })
                        }
                        _ => None,
                    }
                })
                .unwrap_or_else(|| "Unknown".to_string());

            Some(Item::new(
                method_name,
                None,
                range,
                range,
                None,
                ItemInfo::S4Method { signature },
            ))
        }
        "R6Class" => {
            // R6Class("Person", ...)
            // Extract class name (first argument, named "classname" or positional)
            let class_name = get_argument(arguments, rope, "classname", 0)
                .and_then(|argument| {
                    if argument.kind() == "string" {
                        argument
                            .child_by_field_name("content")
                            .map(|content| rope.byte_slice(content.byte_range()).to_string())
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| "Unknown".to_string());

            let mut members = Vec::new();
            for (field, position) in [("public", 1), ("private", 2), ("active", 3)] {
                let Some(list) = get_argument(arguments, rope, field, position) else {
                    continue;
                };

                // list_node is expected to be a call to list(...)
                if list.kind() != "call" {
                    continue;
                }

                let Some(args) = list.child_by_field_name("arguments") else {
                    continue;
                };

                for member in args.children_by_field_name("argument", &mut args.walk()) {
                    let Some(name) = member.child_by_field_name("name") else {
                        continue;
                    };

                    if name.kind() != "identifier" {
                        continue;
                    }

                    let name = rope.byte_slice(name.byte_range()).to_string();
                    let value = member.child_by_field_name("value");
                    let (info, detail, children) = if let Some(value) = value
                        && value.kind() == "function_definition"
                    {
                        let (_, detail, children) = index_function(value, rope, nested);
                        (
                            if field == "active" {
                                ItemInfo::R6Field
                            } else {
                                ItemInfo::R6Method
                            },
                            detail,
                            children,
                        )
                    } else {
                        (ItemInfo::R6Field, None, None)
                    };

                    let range = utils::node_range(member);
                    members.push(Item::new(name, detail, range, range, children, info));
                }
            }

            Some(Item::new(
                class_name,
                None,
                range,
                range,
                Some(members),
                ItemInfo::R6Class,
            ))
        }
        _ => None,
    }
}

// note: this function shouldn't be used for keyword-only arguments (arguments after ...)
pub fn get_argument<'a>(
    arguments: Node<'a>,
    rope: &Rope,
    query: &str,
    pos: usize,
) -> Option<Node<'a>> {
    // Try named argument
    for argument in arguments.children_by_field_name("argument", &mut arguments.walk()) {
        if let Some(name) = argument.child_by_field_name("name") {
            let name = rope.byte_slice(name.byte_range()).to_string();
            if name == query {
                return argument.child_by_field_name("value");
            }
        }
    }

    // Fallback to positional
    arguments
        .children_by_field_name("argument", &mut arguments.walk())
        .nth(pos)
        .and_then(|argument| {
            // Only return if there is no name (i.e., it's a positional argument)
            if argument.child_by_field_name("name").is_none() {
                argument.child_by_field_name("value")
            } else {
                None
            }
        })
}
