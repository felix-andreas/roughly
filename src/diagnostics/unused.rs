use {
    crate::diagnostics::{self, DiagnosticsError, field},
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
        if let Some(parent) = self.scopes[self.current_scope].parent {
            self.current_scope = parent;
        }
    }

    fn declare_variable(&mut self, name: String, node: Node<'a>) {
        let current_scope = &mut self.scopes[self.current_scope];

        if let Some(existing) = current_scope.variables.get_mut(&name) {
            existing.shadowed = Some(Box::new(std::mem::replace(existing, VarInfo::new(node))));
        } else {
            current_scope.variables.insert(name, VarInfo::new(node));
        }
    }

    fn mark_variable_used(&mut self, name: &str) {
        let mut scope_idx = self.current_scope;

        while let Some(scope) = self.scopes.get_mut(scope_idx) {
            if let Some(var_info) = scope.variables.get_mut(name) {
                var_info.is_used = true;
                return;
            }

            match scope.parent {
                Some(parent) => scope_idx = parent,
                None => break,
            }
        }
    }

    fn get_unused_variables(&self) -> Vec<(String, Node<'a>)> {
        let mut unused = Vec::new();

        // Skip global scope (index 0)
        for scope in self.scopes.iter().skip(1) {
            for (name, info) in &scope.variables {
                if !info.is_used {
                    unused.push((name.clone(), info.node));
                }

                let mut maybe_shadow = &info.shadowed;
                while let Some(shadowed) = maybe_shadow {
                    if !shadowed.is_used {
                        unused.push((name.clone(), shadowed.node));
                    }
                    maybe_shadow = &shadowed.shadowed;
                }
            }
        }

        unused
    }
}

pub fn analyze(node: Node, rope: &Rope) -> Result<Vec<Diagnostic>, DiagnosticsError> {
    let mut tracker = VariableTracker::new();
    traverse(&mut node.walk(), rope, &mut tracker)?;

    Ok(tracker
        .get_unused_variables()
        .into_iter()
        .map(|(name, node)| Diagnostic {
            range: diagnostics::node_range(node),
            severity: Some(DiagnosticSeverity::WARNING),
            message: format!("unused variable `{}`", name),
            code: None,
            code_description: None,
            source: None,
            related_information: None,
            tags: None,
            data: None,
        })
        .collect())
}

fn traverse<'a>(
    cursor: &mut TreeCursor<'a>,
    rope: &Rope,
    tracker: &mut VariableTracker<'a>,
) -> Result<(), DiagnosticsError> {
    let node = cursor.node();

    match node.kind() {
        "function_definition" => {
            tracker.push_scope();

            let params = field(node, "parameters")?;

            for param in params.children_by_field_name("parameter", &mut cursor.clone()) {
                let name = field(param, "name")?;
                if name.kind() == "identifier" {
                    let raw = rope.byte_slice(name.byte_range()).to_string();
                    tracker.declare_variable(raw, name);
                }
            }

            let body = field(node, "body")?;
            traverse(&mut body.walk(), rope, tracker)?;

            tracker.pop_scope();
        }

        "call" => {
            let function = field(node, "function")?;

            // Process function first to mark it as used if it's an identifier
            if function.kind() == "identifier" {
                let name = rope.byte_slice(function.byte_range()).to_string();
                tracker.mark_variable_used(&name);
            } else {
                traverse(&mut function.walk(), rope, tracker)?;
            }

            let new_scope = function.kind() == "identifier"
                && rope.byte_slice(function.byte_range()) == "local";

            if new_scope {
                tracker.push_scope();
            }

            traverse(&mut field(node, "arguments")?.walk(), rope, tracker)?;

            if new_scope {
                tracker.pop_scope();
            }
        }

        "binary_operator" => {
            let lhs = field(node, "lhs")?;
            let operator = field(node, "operator")?;
            let rhs = field(node, "rhs")?;

            let is_assignment = operator.kind() == "<-" || operator.kind() == "=";
            if is_assignment {
                traverse(&mut rhs.walk(), rope, tracker)?;
                if lhs.kind() == "identifier" {
                    let name = rope.byte_slice(lhs.byte_range()).to_string();
                    tracker.declare_variable(name, lhs);
                }
            } else {
                traverse(&mut lhs.walk(), rope, tracker)?;
                traverse(&mut rhs.walk(), rope, tracker)?;
            }
        }

        "identifier" => {
            let name = rope.byte_slice(node.byte_range()).to_string();
            tracker.mark_variable_used(&name);
        }

        _ => {
            if cursor.goto_first_child() {
                loop {
                    traverse(cursor, rope, tracker)?;
                    if !cursor.goto_next_sibling() {
                        cursor.goto_parent();
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use {super::*, crate::tree, std::collections::HashSet};

    fn get_unused_var_names(code: &str) -> HashSet<String> {
        let tree = tree::parse(code, None);
        let rope = Rope::from_str(code);
        let diagnostics = analyze(tree.root_node(), &rope);

        diagnostics
            .unwrap()
            .into_iter()
            .map(|diag| {
                let message = diag.message;
                // todo: this is an abonimation, fix this
                message.replace("unused variable `", "").replace("`", "")
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
            x <- 1 # <- this should be unused
            x <- 2 # <- this should be unsued
            x <- 3
            x
        }
        "#;

        let unused_vars = get_unused_var_names(code);
        assert!(
            unused_vars.contains("x"),
            "First declaration of x should be marked as unused due to shadowing"
        );

        let diagnostics =
            analyze(tree::parse(code, None).root_node(), &Rope::from_str(code)).unwrap();
        assert_eq!(diagnostics.len(), 2);

        // Sort diagnostics by line number to ensure consistent test assertion
        let mut sorted_diagnostics = diagnostics;
        sorted_diagnostics.sort_by_key(|d| d.range.start.line);

        assert_eq!(sorted_diagnostics[0].range.start.line, 2);
        assert_eq!(sorted_diagnostics[1].range.start.line, 3);
    }

    #[test]
    fn shadowed_variable_used() {
        let code = r#"
        function() {
            x <- 1 # <- this should be used
            x <- x + 1 # <- this should be used
            x <- x + 1
            x
        }
        "#;

        let unused_vars = get_unused_var_names(code);
        assert_eq!(unused_vars.len(), 0,);
    }

    #[test]
    fn dont_warn_global_scope() {
        let code = r#"
        x <- 1
        y <- 1
        z <- 1
        "#;

        let unused_vars = get_unused_var_names(code);
        assert_eq!(unused_vars.len(), 0,);
    }

    // note: this would require to tracked used variables backwards starting from
    // the last expression and all possible return calls
    // #[test]
    // fn chained_unsued() {
    //     let code = r#"
    //     function() {
    //         a <- 1
    //         b <- a
    //         c <- b
    //     }
    //     "#;

    //     let unused_vars = get_unused_var_names(code);
    //     assert!(unused_vars.contains("a"));
    //     assert!(unused_vars.contains("b"));
    //     assert!(unused_vars.contains("c"));
    // }

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
            a <- 4
            print(a)
        }
        "#;

        let unused_vars = get_unused_var_names(code);
        assert!(
            unused_vars.contains("a"),
            "Parameter 'a' should be marked as unused since it's immediately shadowed"
        );

        let diagnostics =
            analyze(tree::parse(code, None).root_node(), &Rope::from_str(code)).unwrap();
        assert_eq!(
            diagnostics[0].range.start.line, 1,
            "The parameter 'a' should be marked as unused (line 1)"
        );
    }
    #[test]
    fn nested_scopes() {
        let code = r#"
        function() {
            a <- 10
            local({
                b <- 20
                local({
                    c <- 30
                    print(a)
                    print(b)
                })
                d <- 40
            })
            e <- 50
            print(e)
        }
        "#;

        let unused_vars = get_unused_var_names(code);
        assert!(!unused_vars.contains("a"));
        assert!(!unused_vars.contains("b"));
        assert!(unused_vars.contains("c"));
        assert!(unused_vars.contains("d"));
        assert!(!unused_vars.contains("e"));
    }

    #[test]
    fn nested_functions() {
        let code = r#"
        function() {
            outer <- 1
            unused <- 2
            f1 <- function() {
                mid <- 3
                f2 <- function() {
                    inner <- 4
                    print(outer)
                    print(mid)
                }
                return(f2)
            }
            nested <- f1()
            nested()
        }
        "#;

        let unused_vars = get_unused_var_names(code);
        assert!(!unused_vars.contains("outer"));
        assert!(unused_vars.contains("unused"));
        assert!(!unused_vars.contains("mid"));
        assert!(unused_vars.contains("inner"));
        assert!(!unused_vars.contains("f1"));
        assert!(!unused_vars.contains("f2"));
        assert!(!unused_vars.contains("nested"));
    }

    #[test]
    fn multiple_shadowing_levels() {
        let code = r#"
        function() {
            x <- 1
            x <- 2
            x <- 3
            x <- 4
            x <- 5
            print(x)
        }
        "#;

        let diagnostics =
            analyze(tree::parse(code, None).root_node(), &Rope::from_str(code)).unwrap();
        assert_eq!(
            diagnostics.len(),
            4,
            "Should have 4 unused shadowed variables"
        );
    }

    #[test]
    fn conditional_usage() {
        let code = r#"
        function() {
            a <- 1
            b <- 2
            c <- 3
            if (TRUE) {
                print(a)
            } else {
                print(b)
            }
        }
        "#;

        let unused_vars = get_unused_var_names(code);
        assert!(!unused_vars.contains("a"));
        assert!(!unused_vars.contains("b"));
        assert!(unused_vars.contains("c"));
    }

    #[test]
    fn complex_nested_scopes() {
        let code = r#"
        function(param1, param2, param3) {
            outer1 <- 10
            outer2 <- 20
            local({
                inner1 <- 30
                inner2 <- 40
                print(param1)
                print(outer1)
            })

            f <- function(x) {
                z <- x + outer2
                print(param2)
                return(z)
            }

            result <- f(5)
            print(result)
        }
        "#;

        let unused_vars = get_unused_var_names(code);
        assert!(!unused_vars.contains("param1"));
        assert!(!unused_vars.contains("param2"));
        assert!(unused_vars.contains("param3"));
        assert!(!unused_vars.contains("outer1"));
        assert!(!unused_vars.contains("outer2"));
        assert!(unused_vars.contains("inner1"));
        assert!(unused_vars.contains("inner2"));
        assert!(!unused_vars.contains("f"));
        assert!(!unused_vars.contains("x"));
        assert!(!unused_vars.contains("z"));
        assert!(!unused_vars.contains("result"));
    }
}
