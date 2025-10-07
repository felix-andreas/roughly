use {
    crate::{
        index::{ItemInfo, SymbolsMap},
        lsp_types::{
            ParameterInformation, Position, SignatureHelp, SignatureInformation,
        },
        tree::{field, kind},
    },
    ropey::Rope,
    tree_sitter::{Node, Point, Tree},
};

pub fn get(
    position: Position,
    rope: &Rope,
    tree: &Tree,
    symbols_map: &impl SymbolsMap,
) -> Option<SignatureHelp> {
    let point = Point::new(position.line as usize, position.character as usize);
    let node = tree.root_node().descendant_for_point_range(point, point)?;
    
    let (call_node, active_parameter) = find_enclosing_call(node, point, rope)?;
    let function_name = get_function_name(call_node, rope)?;
    
    tracing::debug!(?function_name, ?active_parameter, "signature help");
    
    let signature = find_function_signature(&function_name, symbols_map)?;
    
    Some(SignatureHelp {
        signatures: vec![signature],
        active_signature: Some(0),
        active_parameter: Some(active_parameter),
    })
}

fn find_enclosing_call<'a>(mut node: Node<'a>, point: Point, _rope: &Rope) -> Option<(Node<'a>, u32)> {
    // Walk up the tree to find the enclosing function call
    loop {
        if node.kind_id() == kind::CALL {
            let active_parameter = calculate_active_parameter(node, point)?;
            return Some((node, active_parameter));
        }
        
        // If we're in arguments, check the parent for the call
        if node.kind_id() == kind::ARGUMENTS {
            if let Some(parent) = node.parent() {
                if parent.kind_id() == kind::CALL {
                    let active_parameter = calculate_active_parameter(parent, point)?;
                    return Some((parent, active_parameter));
                }
            }
        }
        
        if let Some(parent) = node.parent() {
            node = parent;
        } else {
            break;
        }
    }
    
    None
}

fn get_function_name(call_node: Node, rope: &Rope) -> Option<String> {
    let function_node = call_node.child_by_field_id(field::FUNCTION)?;
    
    match function_node.kind_id() {
        kind::IDENTIFIER => {
            Some(rope.byte_slice(function_node.byte_range()).to_string())
        }
        kind::NAMESPACE_OPERATOR => {
            // Handle package::function case
            let rhs = function_node.child_by_field_id(field::RHS)?;
            if rhs.kind_id() == kind::IDENTIFIER {
                Some(rope.byte_slice(rhs.byte_range()).to_string())
            } else {
                None
            }
        }
        kind::EXTRACT_OPERATOR => {
            // Handle obj$method case
            let rhs = function_node.child_by_field_id(field::RHS)?;
            if rhs.kind_id() == kind::IDENTIFIER {
                Some(rope.byte_slice(rhs.byte_range()).to_string())
            } else {
                None
            }
        }
        _ => None,
    }
}

fn calculate_active_parameter(call_node: Node, point: Point) -> Option<u32> {
    let arguments = call_node.child_by_field_id(field::ARGUMENTS)?;
    
    // If we're before the opening parenthesis, we're at parameter 0
    if point.row < arguments.start_position().row ||
       (point.row == arguments.start_position().row && point.column < arguments.start_position().column) {
        return Some(0);
    }
    
    // If we're after the closing parenthesis, we're past all parameters
    if point.row > arguments.end_position().row ||
       (point.row == arguments.end_position().row && point.column > arguments.end_position().column) {
        return None;
    }
    
    let mut parameter_index = 0;
    let mut cursor = arguments.walk();
    
    if cursor.goto_first_child() {
        loop {
            let node = cursor.node();
            
            // Skip non-argument nodes (like parentheses and commas)
            if node.kind_id() == kind::COMMA {
                parameter_index += 1;
            } else if is_argument_node(node) {
                // Check if the point is within this argument
                if point_in_range_inclusive(point, node.range()) {
                    return Some(parameter_index);
                }
                // If we're past this argument, continue to the next
                if point.row > node.end_position().row ||
                   (point.row == node.end_position().row && point.column > node.end_position().column) {
                    // Don't increment here, the comma will do it
                } else {
                    // We must be before this argument
                    return Some(parameter_index);
                }
            }
            
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    
    // If we're at the end but inside the parentheses, we're at the last parameter
    Some(parameter_index)
}

fn is_argument_node(node: Node) -> bool {
    // Arguments can be expressions, named arguments, etc.
    // We consider any node that's not a comma or parenthesis as an argument
    !matches!(node.kind_id(), kind::COMMA | kind::LPAREN | kind::RPAREN)
}

fn point_in_range_inclusive(point: Point, range: tree_sitter::Range) -> bool {
    (point.row > range.start_point.row || 
     (point.row == range.start_point.row && point.column >= range.start_point.column)) &&
    (point.row < range.end_point.row ||
     (point.row == range.end_point.row && point.column <= range.end_point.column))
}

fn find_function_signature(function_name: &str, symbols_map: &impl SymbolsMap) -> Option<SignatureInformation> {
    let symbols = symbols_map.filter_map(
        |_, symbols| {
            symbols
                .iter()
                .filter(|symbol| symbol.name == function_name && symbol.info == ItemInfo::Function)
        },
        1, // We only need the first match
    );
    
    if let Some(_symbol) = symbols.into_iter().next() {
        // For now, create a basic signature without detailed parameter info
        // In a real implementation, we'd parse the function definition to extract parameter details
        Some(SignatureInformation {
            label: format!("{}(...)", function_name),
            documentation: None,
            parameters: Some(vec![
                ParameterInformation {
                    label: crate::lsp_types::ParameterLabel::Simple("...".to_string()),
                    documentation: None,
                }
            ]),
            active_parameter: None,
        })
    } else {
        // Create a basic signature even if we don't have symbol information
        Some(SignatureInformation {
            label: format!("{}(...)", function_name),
            documentation: None,
            parameters: Some(vec![
                ParameterInformation {
                    label: crate::lsp_types::ParameterLabel::Simple("...".to_string()),
                    documentation: None,
                }
            ]),
            active_parameter: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use {super::*, crate::tree, indoc::indoc, ropey::Rope, std::collections::HashMap};

    fn setup(text: &str, line: u32, character: u32) -> Option<SignatureHelp> {
        let rope = Rope::from_str(text);
        let tree = tree::parse(&mut tree::new_parser(), text, None);
        let position = Position::new(line, character);
        get(position, &rope, &tree, &HashMap::new())
    }

    #[test]
    fn simple_function_call() {
        let text = indoc! {r#"
            sum(1, 2, 3)
        "#};
        
        // Test at different positions within the call
        let result = setup(text, 0, 4); // Inside "sum("
        assert!(result.is_some());
        let signature_help = result.unwrap();
        assert_eq!(signature_help.signatures.len(), 1);
        assert_eq!(signature_help.signatures[0].label, "sum(...)");
        assert_eq!(signature_help.active_parameter, Some(0));
    }

    #[test]
    fn function_call_with_active_parameter() {
        let text = indoc! {r#"
            sum(1, 2, 3)
        "#};
        
        // Test at position after first comma
        let result = setup(text, 0, 7); // After "sum(1, "
        assert!(result.is_some());
        let signature_help = result.unwrap();
        assert_eq!(signature_help.active_parameter, Some(1));
        
        // Test at position after second comma
        let result = setup(text, 0, 10); // After "sum(1, 2, "
        assert!(result.is_some());
        let signature_help = result.unwrap();
        assert_eq!(signature_help.active_parameter, Some(2));
    }

    #[test]
    fn nested_function_call() {
        let text = indoc! {r#"
            sum(mean(1, 2), 3)
        "#};
        
        // Test inside inner function call
        let result = setup(text, 0, 9); // Inside "mean(1, "
        assert!(result.is_some());
        let signature_help = result.unwrap();
        assert_eq!(signature_help.signatures[0].label, "mean(...)");
        assert_eq!(signature_help.active_parameter, Some(0));
        
        // Test in outer function call
        let result = setup(text, 0, 16); // After "sum(mean(1, 2), "
        assert!(result.is_some());
        let signature_help = result.unwrap();
        assert_eq!(signature_help.signatures[0].label, "sum(...)");
        assert_eq!(signature_help.active_parameter, Some(1));
    }

    #[test]
    fn namespace_function_call() {
        let text = indoc! {r#"
            base::sum(1, 2)
        "#};
        
        let result = setup(text, 0, 10); // Inside "base::sum("
        assert!(result.is_some());
        let signature_help = result.unwrap();
        assert_eq!(signature_help.signatures[0].label, "sum(...)");
        assert_eq!(signature_help.active_parameter, Some(0));
    }

    #[test]
    fn method_call() {
        let text = indoc! {r#"
            obj$method(1, 2)
        "#};
        
        let result = setup(text, 0, 11); // Inside "obj$method("
        assert!(result.is_some());
        let signature_help = result.unwrap();
        assert_eq!(signature_help.signatures[0].label, "method(...)");
        assert_eq!(signature_help.active_parameter, Some(0));
    }

    #[test]
    fn no_signature_help_outside_call() {
        let text = indoc! {r#"
            x <- 1
            sum(1, 2)
        "#};
        
        // Test outside any function call
        let result = setup(text, 0, 2); // Inside "x <- 1"
        assert!(result.is_none());
    }

    #[test]
    fn multiline_function_call() {
        let text = indoc! {r#"
            sum(
                1,
                2,
                3
            )
        "#};
        
        // Test on second line
        let result = setup(text, 1, 8); // After "    1,"
        assert!(result.is_some());
        let signature_help = result.unwrap();
        assert_eq!(signature_help.signatures[0].label, "sum(...)");
        assert_eq!(signature_help.active_parameter, Some(1));
        
        // Test on third line
        let result = setup(text, 2, 8); // After "    2,"
        assert!(result.is_some());
        let signature_help = result.unwrap();
        assert_eq!(signature_help.active_parameter, Some(2));
    }

    #[test]
    fn empty_function_call() {
        let text = indoc! {r#"
            sum()
        "#};
        
        let result = setup(text, 0, 4); // Inside "sum()"
        assert!(result.is_some());
        let signature_help = result.unwrap();
        assert_eq!(signature_help.signatures[0].label, "sum(...)");
        assert_eq!(signature_help.active_parameter, Some(0));
    }
}