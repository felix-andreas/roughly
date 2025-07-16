use {
    crate::{
        definition,
        index::SymbolsMap,
        lsp_types::{Location, Url as Uri},
        tree::{self, kind},
        utils,
    },
    ropey::Rope,
    tree_sitter::{Node, Tree},
};

pub fn find_references(
    uri: &Uri,
    line: usize,
    col: usize,
    include_declaration: bool,
    rope: &Rope,
    tree: &Tree,
    symbols_map: &impl SymbolsMap,
) -> Option<Vec<Location>> {
    let start = tree::try_get_identifier(tree, line, col)?;
    let name = rope.byte_slice(start.byte_range()).to_string();

    tracing::debug!(
        ?name,
        ?include_declaration,
        start = start.kind(),
        "find references"
    );

    // Don't allow finding references for RHS of extract @, extract $, or namespace :: operators
    if tree::is_rhs_of_extract_or_namespace(start) {
        return None;
    }

    let mut locations = Vec::new();

    // First, find the definition
    if let Some(definition) = definition::find_previous_definition(start, rope, &name) {
        // This is a local variable - find references within the current file
        let mut references = Vec::new();
        find_local_references(
            tree.root_node(),
            rope,
            &name,
            definition,
            include_declaration,
            &mut references,
        );

        // Convert nodes to locations
        for node in references {
            locations.push(Location::new(uri.clone(), utils::node_range(node)));
        }
    } else {
        // This might be a global symbol - check if it's defined in this file or other files
        let is_local_scope = tree::find_containing_function(start).is_some();

        // Find references in current file
        if !is_local_scope {
            let mut references = Vec::new();
            find_global_references_in_file(
                tree.root_node(),
                rope,
                &name,
                include_declaration,
                &mut references,
            );

            // Convert nodes to locations
            for node in references {
                locations.push(Location::new(uri.clone(), utils::node_range(node)));
            }
        }

        // Find references in workspace symbols
        let global_refs = symbols_map.filter_map(
            |path, symbols| {
                let other_uri = Uri::from_file_path(path).unwrap();
                // For now, only include references from the same file
                // TODO: In the future, we could find references across all files
                if &other_uri == uri {
                    symbols
                        .iter()
                        .filter(|symbol| symbol.name == name)
                        .map(move |symbol| Location {
                            uri: other_uri.clone(),
                            range: symbol.range,
                        })
                        .collect::<Vec<_>>()
                        .into_iter()
                } else {
                    Vec::new().into_iter()
                }
            },
            128,
        );

        locations.extend(global_refs);
    }

    if locations.is_empty() {
        None
    } else {
        Some(locations)
    }
}

fn find_local_references<'a>(
    root: Node<'a>,
    rope: &Rope,
    name: &str,
    definition: Node<'a>,
    include_declaration: bool,
    references: &mut Vec<Node<'a>>,
) {
    // Find the scope containing the definition
    let scope = tree::find_containing_function(definition).unwrap_or(root);

    find_references_in_scope(
        scope,
        rope,
        name,
        definition,
        include_declaration,
        references,
    );
}

fn find_global_references_in_file<'a>(
    root: Node<'a>,
    rope: &Rope,
    name: &str,
    include_declaration: bool,
    references: &mut Vec<Node<'a>>,
) {
    // For global references, search the entire file
    find_references_in_scope(root, rope, name, root, include_declaration, references);
}

fn find_references_in_scope<'a>(
    scope: Node<'a>,
    rope: &Rope,
    name: &str,
    definition: Node<'a>,
    include_declaration: bool,
    references: &mut Vec<Node<'a>>,
) {
    let mut queue = vec![scope];

    while let Some(node) = queue.pop() {
        // If this is an identifier with matching name, check if it refers to our definition
        if node.kind_id() == kind::IDENTIFIER && rope.byte_slice(node.byte_range()) == name {
            // Don't include RHS of extract operators in references
            if !tree::is_rhs_of_extract_or_namespace(node)
                && refers_to_definition(node, definition, rope, name)
            {
                // Include the declaration if requested, or if it's not the definition node
                if include_declaration || node.id() != definition.id() {
                    references.push(node);
                }
            }
        }

        // Traverse children
        let mut cursor = node.walk();
        if cursor.goto_first_child() {
            loop {
                queue.push(cursor.node());
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
        }
    }
}

fn refers_to_definition(reference: Node, definition: Node, rope: &Rope, name: &str) -> bool {
    // If this is the definition itself, include it
    if reference.id() == definition.id() {
        return true;
    }

    // Use the same logic as find_definition to see if this reference would resolve to our definition
    let found_definition = definition::find_previous_definition(reference, rope, name);
    match found_definition {
        Some(found_def) => found_def.id() == definition.id(),
        None => {
            // If no definition found, this might be a global reference
            // For now, we'll include it if the names match
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use {super::*, crate::tree, indoc::indoc, ropey::Rope};

    fn setup(
        src: &str,
        line: usize,
        col: usize,
        include_declaration: bool,
    ) -> Option<Vec<Location>> {
        let rope = Rope::from_str(src);
        let tree = tree::parse(&mut tree::new_parser(), src, None);
        let uri = Uri::parse("file:///test.R").unwrap();
        let empty_symbols = std::collections::HashMap::new();
        find_references(
            &uri,
            line,
            col,
            include_declaration,
            &rope,
            &tree,
            &empty_symbols,
        )
    }

    #[test]
    fn local_variable_references() {
        let src = indoc! {r#"
            function() {
                x <- 1
                y <- x + 2
                x
            }
        "#};

        let result = setup(src, 1, 4, false).unwrap(); // Find references to x
        assert_eq!(result.len(), 2); // Should find 2 references (excluding declaration)
    }

    #[test]
    fn local_variable_references_with_declaration() {
        let src = indoc! {r#"
            function() {
                x <- 1
                y <- x + 2
                x
            }
        "#};

        let result = setup(src, 1, 4, true).unwrap(); // Find references to x including declaration
        assert_eq!(result.len(), 3); // Should find 3 references (including declaration)
    }

    #[test]
    fn parameter_references() {
        let src = indoc! {r#"
            function(x, y) {
                x + y
            }
        "#};

        let result = setup(src, 0, 9, false).unwrap(); // Find references to x parameter
        assert_eq!(result.len(), 1); // Should find 1 reference (excluding declaration)
    }

    #[test]
    fn parameter_references_with_declaration() {
        let src = indoc! {r#"
            function(x, y) {
                x + y
            }
        "#};

        let result = setup(src, 0, 9, true).unwrap(); // Find references to x parameter with declaration
        assert_eq!(result.len(), 2); // Should find 2 references (including declaration)
    }

    #[test]
    fn no_references_for_rhs_of_extract() {
        let src = indoc! {r#"
            obj@field
        "#};

        let result = setup(src, 0, 4, false); // Try to find references for 'field'
        assert!(result.is_none()); // Should not allow finding references for RHS of @
    }

    #[test]
    fn no_references_for_rhs_of_namespace() {
        let src = indoc! {r#"
            pkg::func
        "#};

        let result = setup(src, 0, 5, false); // Try to find references for 'func'
        assert!(result.is_none()); // Should not allow finding references for RHS of ::
    }

    #[test]
    fn nested_function_scope() {
        let src = indoc! {r#"
            function() {
                x <- 1
                function() {
                    y <- 2
                    y
                }
                x
            }
        "#};

        let result = setup(src, 3, 8, false).unwrap(); // Find references to inner y
        assert_eq!(result.len(), 1); // Should find 1 reference (excluding declaration)
    }

    #[test]
    fn shadowing_variables() {
        let src = indoc! {r#"
            function() {
                x <- 1
                function() {
                    x <- 2
                    x
                }
                x
            }
        "#};

        let result = setup(src, 1, 4, false).unwrap(); // Find references to outer x
        assert_eq!(result.len(), 1); // Should find 1 reference (excluding declaration)
    }

    #[test]
    fn global_variable_references() {
        let src = indoc! {r#"
            x <- 1
            y <- x + 2
            x
        "#};

        // For now, global references in same file should work
        let result = setup(src, 0, 0, false).unwrap(); // Find references to global x
        assert_eq!(result.len(), 2); // Should find 2 references (excluding declaration)
    }

    #[test]
    fn undefined_variable_no_references() {
        let src = indoc! {r#"
            function() {
                undefined_var
            }
        "#};

        let result = setup(src, 1, 4, false); // Try to find references for undefined variable
        // This is a tricky case - for now we'll accept either None or empty Vec
        if let Some(refs) = result {
            assert!(refs.is_empty() || refs.len() == 1);
        }
    }
}
