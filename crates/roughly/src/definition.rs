use {
    crate::{
        index::SymbolsMap,
        lsp_types::{GotoDefinitionResponse, Location, Position, Url as Uri},
        tree::{field, kind},
        utils,
    },
    ropey::Rope,
    tree_sitter::{Node, Point},
};

pub fn goto(
    uri: &Uri,
    position: Position,
    rope: &Rope,
    tree: &tree_sitter::Tree,
    symbols_map: &impl SymbolsMap,
) -> Option<GotoDefinitionResponse> {
    let point = Point::new(position.line as usize, position.character as usize);
    let node = tree.root_node().descendant_for_point_range(point, point)?;
    let parent = node.parent()?; // note: at least parent must be program
    let name = rope.byte_slice(node.byte_range()).to_string();

    tracing::debug!(
        ?name,
        node = node.kind(),
        parent = parent.kind(),
        "goto definition"
    );

    if node.kind_id() != kind::IDENTIFIER {
        return None;
    };

    // todo: consider searching in R6 if $-extract
    if matches!(
        parent.kind_id(),
        kind::EXTRACT_OPERATOR | kind::NAMESPACE_OPERATOR
    ) && parent
        .child_by_field_id(field::RHS)
        .is_some_and(|rhs| rhs.id() == node.id())
    {
        return None;
    }

    if let Some(local) = find_previous_definition(node, rope, &name) {
        return Some(GotoDefinitionResponse::Scalar(Location::new(
            uri.clone(),
            utils::node_range(local),
        )));
    }

    let globals = symbols_map.filter_map(
        |path, symbols| {
            let uri = Uri::from_file_path(path).unwrap();
            symbols
                .iter()
                .flat_map(|symbol| {
                    std::iter::once(symbol).chain(symbol.children.as_ref().into_iter().flatten())
                })
                .filter(|symbol| symbol.name == name)
                .map(move |symbol| Location {
                    uri: uri.clone(),
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

fn find_previous_definition<'a>(start: Node<'a>, rope: &Rope, name: &str) -> Option<Node<'a>> {
    let mut node = start;
    loop {
        node = match node.prev_sibling().or_else(|| node.parent()) {
            Some(next) => next,
            None => break None,
        };

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

    fn setup(src: &str, line: usize, col: usize) -> Option<(usize, usize)> {
        let point = Point::new(line, col);
        let rope = Rope::from_str(src);
        let mut parser = tree::new_parser();
        let tree = tree::parse(&mut parser, src, None);
        let start = tree.root_node().descendant_for_point_range(point, point)?;
        let name = rope.byte_slice(start.byte_range()).to_string();
        find_previous_definition(start, &rope, &name).map(|node| {
            let point = node.start_position();
            (point.row, point.column)
        })
    }

    #[test]
    fn finds_global_definition() {
        let src = indoc! {r#"
            x <- 1
            x
        "#};
        assert_eq!(setup(src, 1, 0), Some((0, 0)));
    }

    #[test]
    fn finds_self_referening() {
        let src = indoc! {r#"
            x <- 1
            x <- x + 1
        "#};
        assert_eq!(setup(src, 1, 0), Some((0, 0)));
    }

    #[test]
    fn finds_parameter_definition() {
        let src = indoc! {r#"
            function(x, y) {
                x + y
            }
        "#};
        assert_eq!(setup(src, 1, 4), Some((0, 9)));
        assert_eq!(setup(src, 1, 8), Some((0, 12)));
    }

    #[test]
    fn finds_local_assignment_with_left_assign() {
        let src = indoc! {r#"
            function() {
                var <- 1
                var
            }
        "#};
        assert_eq!(setup(src, 2, 4), Some((1, 4)));
    }

    #[test]
    fn finds_local_assignment_with_equal() {
        let src = indoc! {r#"
            function() {
                var = 1
                var
            }
        "#};
        assert_eq!(setup(src, 2, 4), Some((1, 4)));
    }

    #[test]
    fn returns_none_for_undefined() {
        let src = indoc! {r#"
            function() {
                x
            }
        "#};
        assert_eq!(setup(src, 1, 4), None);
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
        assert_eq!(setup(src, 3, 4), Some((1, 4)));
        assert_eq!(setup(src, 3, 8), Some((2, 4)));
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
        assert_eq!(setup(src, 2, 4), Some((1, 4)));
        assert_eq!(setup(src, 5, 8), Some((4, 8)));
    }

    // TODO: add extract operator with lhs
    // add complex code where 8 different locations are tested
}
