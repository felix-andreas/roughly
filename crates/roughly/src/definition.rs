use {
    crate::{
        index::SymbolsMap,
        lsp_types::{GotoDefinitionResponse, Location, Url as Uri},
        tree::{self, field, kind},
        utils,
    },
    ropey::Rope,
    tree_sitter::{Node, Tree},
};

pub fn goto(
    uri: &Uri,
    line: usize,
    col: usize,
    rope: &Rope,
    tree: &Tree,
    symbols_map: &impl SymbolsMap,
) -> Option<GotoDefinitionResponse> {
    // note: R allows to call strings like "function_name"().
    // also useful for S4:  e.g. to goto "Person" definition in
    // setMethod("age", "Person", ...) or setClass("Employee", contains = "Person")
    let (start, byte_range) = tree::try_get_identifier_or_string(tree, line, col)?;
    let name = rope.byte_slice(byte_range).to_string();

    tracing::debug!(?name, start = start.kind(), "goto definition");

    // TODO: to implement this correctly we would need to infer the type of rhs expr
    // However, we could just search all fields and methods for possible matches.
    if tree::is_rhs_of_extract_or_namespace(start) {
        return None;
    }

    if let Some(local) = find_previous_definition(start, rope, &name) {
        return Some(GotoDefinitionResponse::Scalar(Location::new(
            uri.clone(),
            utils::node_range(local),
        )));
    }

    let is_global_scope = tree::find_containing_function(start).is_none();

    let globals = symbols_map.filter_map(
        |path, symbols| {
            let other_uri = Uri::from_file_path(path).unwrap();
            // note that s4 and r6 are not found by find_previous_definition.
            // therefore if is same file and global scope we must ignore later defintions
            let ignore_later = is_global_scope && *uri == other_uri;
            let name = &name;
            symbols
                .iter()
                .filter(move |symbol| {
                    symbol.name == *name
                        && (!ignore_later
                            || symbol.range.end < utils::point_to_position(start.start_position()))
                })
                .map(move |symbol| Location {
                    uri: other_uri.clone(),
                    range: symbol.range,
                })
        },
        128,
    );

    match globals.len() {
        0 | 1 => globals
            .into_iter()
            .next()
            .map(GotoDefinitionResponse::Scalar),
        _ => Some(GotoDefinitionResponse::Array(globals)),
    }
}

pub fn find_previous_definition<'a>(start: Node<'a>, rope: &Rope, name: &str) -> Option<Node<'a>> {
    let mut node = start;
    loop {
        let is_descendent;
        if let Some(sibling) = node.prev_sibling() {
            node = sibling;
            is_descendent = false;
        } else if let Some(parent) = node.parent() {
            node = parent;
            is_descendent = true;
        } else {
            break None;
        }

        let maybe_definition = match node.kind_id() {
            kind::PARAMETERS => node
                .children_by_field_name("parameter", &mut node.walk())
                .filter_map(|parameter| parameter.child_by_field_id(field::NAME))
                .find(|child| rope.byte_slice(child.byte_range()) == name),
            kind::BINARY_OPERATOR => {
                let maybe_lhs = node.child_by_field_id(field::LHS);
                let maybe_op = node.child_by_field_id(field::OPERATOR);
                maybe_lhs.filter(|lhs| {
                    lhs.kind_id() == kind::IDENTIFIER
                        && maybe_op.is_some_and(|op| {
                            [kind::EQUAL, kind::LEFT_ASSIGN].contains(&op.kind_id())
                        })
                        && rope.byte_slice(lhs.byte_range()) == name
                        // If `is_descendent` is true, the identifier is part of the current assignment
                        // (e.g., `x <- x + 1`) and we must continue searching in previous expressions.
                        // However, if the LHS is exactly the start node, we allow it, since LSP will
                        // search for references if the definition location matches the start location.
                        && (!is_descendent || lhs.id() == start.id())
                })
            }
            _ => None,
        };

        if maybe_definition.is_some() {
            return maybe_definition;
        }
    }
}

#[cfg(test)]
mod tests {
    use {super::*, crate::tree, indoc::indoc, ropey::Rope};

    fn setup_a(src: &str, line: usize, col: usize) -> Option<(usize, usize)> {
        let rope = Rope::from_str(src);
        let tree = tree::parse(&mut tree::new_parser(), src, None);
        let (start, byte_range) = tree::try_get_identifier_or_string(&tree, line, col)?;
        let name = rope.byte_slice(byte_range).to_string();
        find_previous_definition(start, &rope, &name).map(|node| {
            let point = node.start_position();
            (point.row, point.column)
        })
    }

    fn setup_b(src: &str, line: usize, col: usize) -> Option<String> {
        let rope = Rope::from_str(src);
        let tree = tree::parse(&mut tree::new_parser(), src, None);
        tree::try_get_identifier_or_string(&tree, line, col)
            .map(|(_, byte_range)| rope.byte_slice(byte_range).to_string())
    }

    #[test]
    fn try_get_identifier_base() {
        let src = indoc! {r#"
            123
            x <- 1
                x
            setMethod("age", "Person", \(x) x@age)
        "#};
        assert_eq!(setup_b(src, 0, 0), None);
        assert_eq!(setup_b(src, 1, 0), Some("x".into()));
        assert_eq!(setup_b(src, 2, 0), None);
        assert_eq!(setup_b(src, 3, 17), Some("Person".into()));
        // assert_eq!(setup_b(src, 3, 18), Some("Person".into()));
        assert_eq!(setup_b(src, 3, 24), Some("Person".into()));
    }

    #[test]
    fn try_get_identifier_handles_program_start() {
        let src = indoc! {r#"
            x <- 1
        "#};
        // note that `descendant_for_point_range` might return the program node
        // Cursor at very start of program, should find 'x'
        assert_eq!(setup_b(src, 0, 0).unwrap(), "x");
    }

    #[test]
    fn try_get_identifier_returns_none_for_empty() {
        let src = "";
        assert_eq!(setup_b(src, 0, 0), None);
    }

    #[test]
    fn try_get_identifier_within_function_parameters() {
        let src = indoc! {r#"
            function(a, b) {
                a + b
            }
        "#};
        // Position at 'a' in parameters
        assert_eq!(setup_b(src, 0, 9), Some("a".to_string()));
        // Position at 'b' in parameters
        assert_eq!(setup_b(src, 0, 12), Some("b".to_string()));
    }

    #[test]
    fn finds_global_definition() {
        let src = indoc! {r#"
            x <- 1
            x
        "#};
        assert_eq!(setup_a(src, 1, 0), Some((0, 0)));
    }

    #[test]
    fn finds_self_referening() {
        let src = indoc! {r#"
            x <- 1
            x <- x + 1
        "#};
        assert_eq!(setup_a(src, 0, 0), Some((0, 0)));
        assert_eq!(setup_a(src, 1, 5), Some((0, 0)));
    }

    #[test]
    fn finds_parameter_definition() {
        let src = indoc! {r#"
            function(x, y) {
                x + y
            }
        "#};
        assert_eq!(setup_a(src, 1, 4), Some((0, 9)));
        assert_eq!(setup_a(src, 1, 8), Some((0, 12)));
    }

    #[test]
    fn finds_local_assignment_with_left_assign() {
        let src = indoc! {r#"
            function() {
                var <- 1
                var
            }
        "#};
        assert_eq!(setup_a(src, 2, 4), Some((1, 4)));
    }

    #[test]
    fn finds_local_assignment_with_equal() {
        let src = indoc! {r#"
            function() {
                var = 1
                var
            }
        "#};
        assert_eq!(setup_a(src, 2, 4), Some((1, 4)));
    }

    #[test]
    fn returns_none_for_undefined() {
        let src = indoc! {r#"
            function() {
                x
            }
        "#};
        assert_eq!(setup_a(src, 1, 4), None);
    }

    #[test]
    fn skips_non_matching_nodes() {
        let src = indoc! {r#"
            function() {
                a <- 1
                b <- 2
                a + b
            }
        "#};
        assert_eq!(setup_a(src, 3, 4), Some((1, 4)));
        assert_eq!(setup_a(src, 3, 8), Some((2, 4)));
    }

    #[test]
    fn finds_definition_in_correct_scope() {
        let src = indoc! {r#"
            function() {
                x <- 1
                x
                {
                    x <- 2
                    x
                }
            }
        "#};
        // Should find the inner x definition
        assert_eq!(setup_a(src, 2, 4), Some((1, 4)));
        assert_eq!(setup_a(src, 5, 8), Some((4, 8)));
    }

    #[test]
    fn multiple_expressions() {
        let src = indoc! {r#"
            x <- 1
            x
            x@field
            x$item
            x['field']
            x::item
            x + y
            fn(x)
        "#};
        assert_eq!(setup_a(src, 1, 0), Some((0, 0)));
        assert_eq!(setup_a(src, 2, 0), Some((0, 0)));
        assert_eq!(setup_a(src, 3, 0), Some((0, 0)));
        assert_eq!(setup_a(src, 4, 0), Some((0, 0)));
        assert_eq!(setup_a(src, 5, 0), Some((0, 0)));
        assert_eq!(setup_a(src, 6, 0), Some((0, 0)));
        assert_eq!(setup_a(src, 7, 3), Some((0, 0)));
    }

    #[test]
    fn finds_definition_with_nested_functions() {
        let src = indoc! {r#"
            x <- 1
            function() {
                function() {
                    x
                    x <- 1
                    x
                }
                x
                x <- 1
                function() {
                    x
                    x <- 1
                    x
                }
                x
            }
            x
        "#};
        println!("foo");
        assert_eq!(setup_a(src, 3, 8), Some((0, 0)));
        println!("foo");
        assert_eq!(setup_a(src, 5, 8), Some((4, 8)));
        assert_eq!(setup_a(src, 7, 4), Some((0, 0)));
        assert_eq!(setup_a(src, 10, 8), Some((8, 4)));
        assert_eq!(setup_a(src, 12, 8), Some((11, 8)));
        assert_eq!(setup_a(src, 14, 4), Some((8, 4)));
        assert_eq!(setup_a(src, 16, 0), Some((0, 0)));
    }
}
