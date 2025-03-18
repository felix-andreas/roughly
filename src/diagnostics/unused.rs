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
        Self {
            scopes: vec![Scope {
                variables: HashMap::new(),
                parent: None,
            }],
            current_scope: 0,
        }
    }

    fn push_scope(&mut self) -> usize {
        let parent = self.current_scope;
        self.scopes.push(Scope {
            variables: HashMap::new(),
            parent: Some(parent),
        });
        self.current_scope = self.scopes.len() - 1;
        self.current_scope
    }

    fn pop_scope(&mut self) {
        self.current_scope = self.scopes[self.current_scope].parent.unwrap_or(0);
    }

    fn declare_variable(&mut self, name: String, node: Node<'a>) {
        let current_scope = &mut self.scopes[self.current_scope];
        if let Some(existing) = current_scope.variables.remove(&name) {
            let mut new_var = VarInfo::new(node);
            new_var.shadowed = Some(Box::new(existing));
            current_scope.variables.insert(name, new_var);
        } else {
            current_scope.variables.insert(name, VarInfo::new(node));
        }
    }

    fn mark_variable_used(&mut self, name: &str) {
        let mut scope_idx = self.current_scope;
        loop {
            if let Some(var_info) = self.scopes[scope_idx].variables.get_mut(name) {
                var_info.is_used = true;
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

        for scope in &self.scopes {
            for (name, info) in &scope.variables {
                if !info.is_used {
                    unused.push((name.clone(), info.node));
                }

                let mut current_shadow = &info.shadowed;
                while let Some(shadowed) = current_shadow {
                    if !shadowed.is_used {
                        unused.push((name.clone(), shadowed.node));
                    }
                    current_shadow = &shadowed.shadowed;
                }
            }
        }

        unused
    }
}

pub fn analyze(node: Node, rope: &Rope) -> Vec<Diagnostic> {
    let mut tracker = VariableTracker::new();
    let mut cursor = node.walk();

    traverse(&mut cursor, rope, &mut tracker);

    tracker
        .get_unused_variables()
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

            if let Some(body_node) = node.child_by_field_name("body") {
                let mut body_cursor = body_node.walk();
                traverse(&mut body_cursor, rope, tracker);
            }

            tracker.pop_scope();
            return;
        }

        "call" => {
            let is_local_call = node
                .child_by_field_name("function")
                .is_some_and(|function_node| {
                    function_node.kind() == "identifier"
                        && rope.byte_slice(function_node.byte_range()) == "local"
                });

            if is_local_call {
                tracker.push_scope();

                if let Some(args_node) = node.child_by_field_name("arguments") {
                    let mut args_cursor = args_node.walk();
                    traverse(&mut args_cursor, rope, tracker);
                }

                tracker.pop_scope();
                return;
            }
        }

        "binary_operator" => {
            if let (Some(lhs), Some(operator)) = (
                node.child_by_field_name("lhs"),
                node.child_by_field_name("operator"),
            ) {
                let is_assignment = operator.kind() == "<-" || operator.kind() == "=";
                if lhs.kind() == "identifier" && is_assignment {
                    let name = rope.byte_slice(lhs.byte_range()).to_string();
                    tracker.declare_variable(name, lhs);

                    if let Some(rhs) = node.child_by_field_name("rhs") {
                        let mut rhs_cursor = rhs.walk();
                        traverse(&mut rhs_cursor, rope, tracker);
                    }

                    return;
                }
            }
        }

        "identifier" => {
            if let Some(parent) = node.parent() {
                match parent.kind() {
                    "binary_operator"
                        if parent
                            .child_by_field_name("lhs")
                            .is_some_and(|lhs| lhs.id() == node.id()) =>
                    {
                        if parent
                            .child_by_field_name("operator")
                            .is_some_and(|op| op.kind() == "<-" || op.kind() == "=")
                        {
                            return;
                        }
                    }
                    "parameter"
                        if parent
                            .child_by_field_name("name")
                            .is_some_and(|name| name.id() == node.id()) =>
                    {
                        return;
                    }
                    _ => {}
                }
            }

            let name = rope.byte_slice(node.byte_range()).to_string();
            tracker.mark_variable_used(&name);
            return;
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
                message.replace("unused variable '", "").replace("'", "")
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
            x = 1 # <- this should be unused
            x = 2 # <- this should be unsued
            x = 3
            x
        }
        "#;

        let unused_vars = get_unused_var_names(code);
        assert!(
            unused_vars.contains("x"),
            "First declaration of x should be marked as unused due to shadowing"
        );

        let diagnostics = analyze(tree::parse(code, None).root_node(), &Rope::from_str(code));
        assert_eq!(diagnostics.len(), 2);

        // Sort diagnostics by line number to ensure consistent test assertion
        let mut sorted_diagnostics = diagnostics;
        sorted_diagnostics.sort_by_key(|d| d.range.start.line);

        assert_eq!(sorted_diagnostics[0].range.start.line, 2);
        assert_eq!(sorted_diagnostics[1].range.start.line, 3);
    }

    #[test]
    fn chained_unsued() {
        let code = r#"
        function() {
            a <- 1
            b <- a
            c <- b
        }
        "#;

        let unused_vars = get_unused_var_names(code);
        assert!(unused_vars.contains("a"));
        assert!(unused_vars.contains("b"));
        assert!(unused_vars.contains("c"));
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
        assert_eq!(
            diagnostics[0].range.start.line, 1,
            "The parameter 'a' should be marked as unused (line 1)"
        );
    }
}
