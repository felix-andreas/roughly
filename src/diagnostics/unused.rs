#![allow(unused)]

use {
    crate::diagnostics,
    ropey::Rope,
    std::collections::HashMap,
    tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity},
    tree_sitter::{Node, TreeCursor},
};

#[derive(Debug, Clone)]
struct Scope<'a> {
    variables: HashMap<String, VarInfo<'a>>,
    parent: Option<usize>,
}

#[derive(Debug, Clone)]
struct VarInfo<'a> {
    node: Node<'a>,
    is_used: bool,
    shadowed: Option<Box<VarInfo<'a>>>,
}

impl<'a> VarInfo<'a> {
    fn new(node: Node<'a>) -> Self {
        Self {
            node,
            is_used: false,
            shadowed: None,
        }
    }
}

#[derive(Debug, Clone)]
struct VariableTracker<'a> {
    scopes: Vec<Scope<'a>>,
    current_scope: usize,
}

impl<'a> VariableTracker<'a> {
    fn new() -> Self {
        let global_scope = Scope {
            variables: HashMap::new(),
            parent: None,
        };
        Self {
            scopes: vec![global_scope],
            current_scope: 0,
        }
    }

    fn push_scope(&mut self) -> usize {
        let parent = self.current_scope;
        let new_scope = Scope {
            variables: HashMap::new(),
            parent: Some(parent),
        };
        self.scopes.push(new_scope);
        let new_scope_idx = self.scopes.len() - 1;
        self.current_scope = new_scope_idx;
        new_scope_idx
    }

    fn pop_scope(&mut self) {
        if let Some(parent) = self.scopes[self.current_scope].parent {
            self.current_scope = parent;
        }
    }

    // Track variable declaration
    fn declare_variable(&mut self, name: String, node: Node<'a>) {
        // When redeclaring a variable in the same scope, keep track of the shadowed version
        if let Some(existing) = self.scopes[self.current_scope].variables.remove(&name) {
            // Create a new variable that shadows the existing one
            let mut new_var = VarInfo::new(node);
            // Store the existing variable as shadowed
            new_var.shadowed = Some(Box::new(existing));

            // Insert the new variable with shadowing information
            self.scopes[self.current_scope]
                .variables
                .insert(name, new_var);
        } else {
            // First declaration of this variable in this scope
            self.scopes[self.current_scope]
                .variables
                .insert(name, VarInfo::new(node));
        }
    }

    fn mark_variable_used(&mut self, name: &str) {
        let mut scope_idx = self.current_scope;
        loop {
            if self.scopes[scope_idx].variables.contains_key(name) {
                self.scopes[scope_idx]
                    .variables
                    .get_mut(name)
                    .unwrap()
                    .is_used = true;
                return;
            }

            if let Some(parent) = self.scopes[scope_idx].parent {
                scope_idx = parent;
            } else {
                break;
            }
        }
    }

    fn get_unused_variables(&self) -> Vec<(String, Node<'a>)> {
        let mut unused = Vec::new();

        for scope_idx in 1..self.scopes.len() {
            let scope = &self.scopes[scope_idx];
            for (name, info) in &scope.variables {
                // Check if the current variable is unused
                if !info.is_used {
                    unused.push((name.clone(), info.node));
                }

                // Check for shadowed variables that are unused
                let mut shadow_info = &info.shadowed;
                while let Some(shadowed) = shadow_info {
                    if !shadowed.is_used {
                        unused.push((name.clone(), shadowed.node));
                    }
                    shadow_info = &shadowed.shadowed;
                }
            }
        }

        unused
    }
}

pub fn analyze(node: Node, rope: &Rope) -> Vec<Diagnostic> {
    let node = unsafe { std::mem::transmute::<Node, Node<'static>>(node) };

    let mut tracker = VariableTracker::new();
    let mut cursor = node.walk();

    traverse(&mut cursor, rope, &mut tracker);

    let unused_vars = tracker.get_unused_variables();
    unused_vars
        .into_iter()
        .map(|(name, node)| Diagnostic {
            range: diagnostics::node_range(node),
            severity: Some(DiagnosticSeverity::WARNING),
            message: format!("unused variable '{}'", name),
            code: None,
            code_description: None,
            source: None,
            related_information: None,
            tags: None,
            data: None,
        })
        .collect()
}

fn traverse<'a>(cursor: &mut TreeCursor<'a>, rope: &Rope, tracker: &mut VariableTracker<'a>) {
    let node = cursor.node();

    match node.kind() {
        "function_definition" => {
            tracker.push_scope();

            if let Some(params_node) = node.child_by_field_name("parameters") {
                let mut param_cursor = params_node.walk();
                if param_cursor.goto_first_child() {
                    loop {
                        let child = param_cursor.node();
                        if child.kind() == "parameter" {
                            if let Some(name_node) = child.child_by_field_name("name") {
                                if name_node.kind() == "identifier" {
                                    let name = rope.byte_slice(name_node.byte_range()).to_string();
                                    tracker.declare_variable(name, name_node);
                                }
                            }
                        }

                        if !param_cursor.goto_next_sibling() {
                            break;
                        }
                    }
                }
            }

            if cursor.goto_first_child() {
                loop {
                    traverse(cursor, rope, tracker);
                    if !cursor.goto_next_sibling() {
                        break;
                    }
                }
                cursor.goto_parent();
            }

            tracker.pop_scope();
            return;
        }

        "call" => {
            let mut is_local_call = false;
            let mut call_cursor = node.walk();

            if call_cursor.goto_first_child() {
                let child = call_cursor.node();
                if child.kind() == "identifier" {
                    let name = rope.byte_slice(child.byte_range()).to_string();
                    if name == "local" {
                        is_local_call = true;
                    }
                }
            }

            if is_local_call {
                tracker.push_scope();
            }

            if cursor.goto_first_child() {
                loop {
                    traverse(cursor, rope, tracker);
                    if !cursor.goto_next_sibling() {
                        break;
                    }
                }
                cursor.goto_parent();
            }

            if is_local_call {
                tracker.pop_scope();
            }
            return;
        }

        // Check for variable assignments
        "binary_operator" => {
            if let (Some(lhs), Some(operator)) = (
                node.child_by_field_name("lhs"),
                node.child_by_field_name("operator"),
            ) {
                if lhs.kind() == "identifier" && (operator.kind() == "<-" || operator.kind() == "=")
                {
                    let name = rope.byte_slice(lhs.byte_range()).to_string();
                    // Only track as a variable declaration if we're in a local scope
                    if tracker.current_scope > 0 {
                        // For R, any assignment is a declaration/redeclaration
                        tracker.declare_variable(name, lhs);
                    }
                }
            }
        }

        "identifier" => {
            let parent = node.parent();
            if let Some(parent) = parent {
                if parent.kind() == "binary_operator" {
                    if let Some(lhs) = parent.child_by_field_name("lhs") {
                        if lhs.id() == node.id() {
                            return;
                        }
                    }
                } else if parent.kind() == "parameter" {
                    if let Some(name) = parent.child_by_field_name("name") {
                        if name.id() == node.id() {
                            return;
                        }
                    }
                }
            }

            let name = rope.byte_slice(node.byte_range()).to_string();
            tracker.mark_variable_used(&name);
        }
        _ => {}
    }

    if cursor.goto_first_child() {
        loop {
            traverse(cursor, rope, tracker);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

#[cfg(test)]
mod tests {
    use {super::*, crate::tree, std::collections::HashSet};

    fn get_unused_var_names(code: &str) -> HashSet<String> {
        let tree = tree::parse(code, None);
        let rope = Rope::from_str(code);
        let diagnostics = analyze(tree.root_node(), &rope);

        diagnostics
            .into_iter()
            .map(|d| {
                let message = d.message;
                message.replace("Unused variable '", "").replace("'", "")
            })
            .collect()
    }

    #[test]
    fn unused_local_variable() {
        let code = r#"
        function() {
            x <- 10
            y <- 20
            return(x)
        }
        "#;

        let unused_vars = get_unused_var_names(code);
        assert!(unused_vars.contains("y"));
        assert!(!unused_vars.contains("x"));
    }

    #[test]
    fn used_in_nested_function() {
        let code = r#"
        function() {
            x <- 10
            inner <- function() {
                return(x)
            }
            inner()
        }
        "#;

        let unused_vars = get_unused_var_names(code);
        assert!(!unused_vars.contains("x"));
        assert!(!unused_vars.contains("inner"));
    }

    #[test]
    fn shadowed_variable() {
        let code = r#"
        function() {
            x = 4 # <- this should be unused
            x = 5
            x
        }
        "#;

        let unused_vars = get_unused_var_names(code);
        assert!(
            unused_vars.contains("x"),
            "First declaration of x should be marked as unused due to shadowing"
        );

        // To verify that only one x is marked as unused (the first one)
        let diagnostics = analyze(tree::parse(code, None).root_node(), &Rope::from_str(code));
        assert_eq!(
            diagnostics.len(),
            1,
            "Should have exactly one unused variable diagnostic"
        );

        // Check the line number to make sure it's the first declaration that's marked as unused
        assert_eq!(
            diagnostics[0].range.start.line, 2,
            "The first declaration of x should be marked as unused (line 2)"
        );
    }

    #[test]
    fn local_scope() {
        let code = r#"
        function() {
            x <- 10
            local({
                y <- 20
                z <- 30
                print(y)
            })
            return(x)
        }
        "#;

        let unused_vars = get_unused_var_names(code);
        assert!(!unused_vars.contains("x"));
        assert!(!unused_vars.contains("y"));
        assert!(unused_vars.contains("z"));
    }

    #[test]
    fn function_parameters() {
        let code = r#"
        function(a, b, c) {
            print(a)
            return(c)
        }
        "#;

        let unused_vars = get_unused_var_names(code);
        assert!(!unused_vars.contains("a"));
        assert!(unused_vars.contains("b"));
        assert!(!unused_vars.contains("c"));
    }
    #[test]
    fn shadowed_parameters() {
        let code = r#"
        function(a) {
            a = 4
            print(a)
        }
        "#;

        let unused_vars = get_unused_var_names(code);
        assert!(
            unused_vars.contains("a"),
            "Parameter 'a' should be marked as unused since it's immediately shadowed"
        );

        let diagnostics = analyze(tree::parse(code, None).root_node(), &Rope::from_str(code));
        // Check the line number to confirm it's the parameter
        assert_eq!(
            diagnostics[0].range.start.line, 1,
            "The parameter 'a' should be marked as unused (line 1)"
        );
    }
}
