use {
    ropey::Rope,
    std::collections::HashMap,
    tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range},
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
}

impl<'a> VarInfo<'a> {
    fn new(node: Node<'a>) -> Self {
        Self {
            node,
            is_used: false,
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

    fn declare_variable(&mut self, name: String, node: Node<'a>) {
        self.scopes[self.current_scope]
            .variables
            .insert(name, VarInfo::new(node));
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
                if !info.is_used {
                    unused.push((name.clone(), info.node));
                }
            }
        }

        unused
    }
}

pub fn find_unused_variables(node: Node, rope: &Rope) -> Vec<Diagnostic> {
    let node = unsafe { std::mem::transmute::<Node, Node<'static>>(node) };

    let mut tracker = VariableTracker::new();
    let mut cursor = node.walk();

    analyze_node(&mut cursor, rope, &mut tracker);

    let unused_vars = tracker.get_unused_variables();
    unused_vars
        .into_iter()
        .map(|(name, node)| Diagnostic {
            range: node_range(node),
            severity: Some(DiagnosticSeverity::WARNING),
            message: format!("Unused variable '{}'", name),
            code: None,
            code_description: None,
            source: Some("roughly".to_string()),
            related_information: None,
            tags: None,
            data: None,
        })
        .collect()
}

fn analyze_node<'a>(cursor: &mut TreeCursor<'a>, rope: &Rope, tracker: &mut VariableTracker<'a>) {
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
                    analyze_node(cursor, rope, tracker);
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
                    analyze_node(cursor, rope, tracker);
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

        "binary_operator" => {
            if let (Some(lhs), Some(operator)) = (
                node.child_by_field_name("lhs"),
                node.child_by_field_name("operator"),
            ) {
                if lhs.kind() == "identifier" && (operator.kind() == "<-" || operator.kind() == "=")
                {
                    let name = rope.byte_slice(lhs.byte_range()).to_string();
                    if tracker.current_scope > 0 {
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
            analyze_node(cursor, rope, tracker);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

fn node_range(node: Node) -> Range {
    Range {
        start: Position {
            line: node.start_position().row as u32,
            character: node.start_position().column as u32,
        },
        end: Position {
            line: node.end_position().row as u32,
            character: node.end_position().column as u32,
        },
    }
}

#[cfg(test)]
mod tests {
    use {super::*, crate::tree, std::collections::HashSet};

    fn get_unused_var_names(code: &str) -> HashSet<String> {
        let tree = tree::parse(code, None);
        let rope = Rope::from_str(code);
        let diagnostics = find_unused_variables(tree.root_node(), &rope);

        diagnostics
            .into_iter()
            .map(|d| {
                let message = d.message;
                message.replace("Unused variable '", "").replace("'", "")
            })
            .collect()
    }

    #[test]
    fn test_unused_local_variable() {
        let code = r#"
        test <- function() {
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
    fn test_used_in_nested_function() {
        let code = r#"
        test <- function() {
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
    fn test_local_scope() {
        let code = r#"
        test <- function() {
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
    fn test_function_parameters() {
        let code = r#"
        test <- function(a, b, c) {
            print(a)
            return(c)
        }
        "#;

        let unused_vars = get_unused_var_names(code);
        assert!(!unused_vars.contains("a"));
        assert!(unused_vars.contains("b"));
        assert!(!unused_vars.contains("c"));
    }
}
