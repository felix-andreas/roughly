use {
    crate::{
        diagnostics::{DiagnosticsError, field},
        lsp_types::{Diagnostic, DiagnosticSeverity},
        utils,
    },
    async_lsp::lsp_types::DiagnosticTag,
    ropey::Rope,
    std::collections::HashMap,
    tree_sitter::{Node, TreeCursor},
};

#[derive(Debug, Clone)]
struct Scope<'a> {
    variables: HashMap<String, Variable<'a>>,
    parent: Option<usize>,
}

#[derive(Debug, Clone)]
struct Variable<'a> {
    node: Node<'a>,
    is_used: bool,
    shadowed: Option<Box<Variable<'a>>>,
}

impl<'a> Variable<'a> {
    fn new(node: Node<'a>) -> Self {
        Self {
            node,
            is_used: false,
            shadowed: None,
        }
    }
}

#[derive(Debug, Clone)]
struct ScopeTree<'a> {
    scopes: Vec<Scope<'a>>,
    current_scope: usize,
}

impl<'a> ScopeTree<'a> {
    fn new() -> Self {
        Self {
            scopes: vec![Scope {
                variables: HashMap::new(),
                parent: None,
            }],
            current_scope: 0,
        }
    }

    fn push(&mut self) -> usize {
        let parent = self.current_scope;
        self.scopes.push(Scope {
            variables: HashMap::new(),
            parent: Some(parent),
        });
        self.current_scope = self.scopes.len() - 1;
        self.current_scope
    }

    fn pop(&mut self) {
        if let Some(parent) = self.scopes[self.current_scope].parent {
            self.current_scope = parent;
        }
    }

    fn declare(&mut self, name: String, node: Node<'a>) {
        let current_scope = &mut self.scopes[self.current_scope];

        if let Some(existing) = current_scope.variables.get_mut(&name) {
            existing.shadowed = Some(Box::new(std::mem::replace(existing, Variable::new(node))));
        } else {
            current_scope.variables.insert(name, Variable::new(node));
        }
    }

    fn mark_unused(&mut self, name: &str) {
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

    // fn finalize_usage(&mut self) {
    //     // Process all scopes to mark variables that are truly used
    //     // First collect all truly used names
    //     let mut all_truly_used = Vec::new();

    //     for scope in &self.scopes {
    //         for (name, info) in &scope.variables {
    //             if info.is_used {
    //                 all_truly_used.push(name.clone());
    //             }
    //         }
    //     }

    //     // Then propagate usage for all collected names
    //     for name in all_truly_used {
    //         self.propagate_usage(&name);
    //     }
    // }

    // fn propagate_usage(&mut self, name: &str) {
    //     let mut scope_idx = self.current_scope;

    //     while let Some(scope) = self.scopes.get_mut(scope_idx) {
    //         if let Some(var_info) = scope.variables.get_mut(name) {
    //             var_info.is_used = true;

    //             // Also handle any shadowed versions
    //             let mut shadow = &mut var_info.shadowed;
    //             while let Some(shadowed) = shadow {
    //                 shadowed.is_used = true;
    //                 shadow = &mut shadowed.shadowed;
    //             }

    //             return;
    //         }

    //         match scope.parent {
    //             Some(parent) => scope_idx = parent,
    //             None => break,
    //         }
    //     }
    // }
}

pub fn analyze(node: Node, rope: &Rope) -> Result<Vec<Diagnostic>, DiagnosticsError> {
    let mut scopes = ScopeTree::new();
    traverse(&mut node.walk(), rope, &mut scopes)?;
    // scopes.finalize_usage();

    Ok(scopes
        .get_unused_variables()
        .into_iter()
        .map(|(name, node)| Diagnostic {
            range: utils::node_range(node),
            severity: Some(DiagnosticSeverity::WARNING),
            message: format!("unused variable `{name}`"),
            code: None,
            code_description: None,
            source: None,
            related_information: None,
            tags: Some(vec![DiagnosticTag::UNNECESSARY]),
            data: None,
        })
        .collect())
}

fn traverse<'a>(
    cursor: &mut TreeCursor<'a>,
    rope: &Rope,
    scopes: &mut ScopeTree<'a>,
) -> Result<(), DiagnosticsError> {
    let node = cursor.node();

    match node.kind() {
        "function_definition" => {
            scopes.push();

            let params = field(node, "parameters")?;

            for param in params.children_by_field_name("parameter", &mut cursor.clone()) {
                let name = field(param, "name")?;
                if name.kind() == "identifier" {
                    let raw = rope.byte_slice(name.byte_range()).to_string();
                    scopes.declare(raw, name);
                }
            }

            let body = field(node, "body")?;
            traverse(&mut body.walk(), rope, scopes)?;

            scopes.pop();
        }

        "call" => {
            let function = field(node, "function")?;

            // Process function first to mark it as used if it's an identifier
            if function.kind() == "identifier" {
                let name = rope.byte_slice(function.byte_range()).to_string();
                scopes.mark_unused(&name);
            } else {
                traverse(&mut function.walk(), rope, scopes)?;
            }

            let new_scope = function.kind() == "identifier"
                && rope.byte_slice(function.byte_range()) == "local";

            if new_scope {
                scopes.push();
            }

            traverse(&mut field(node, "arguments")?.walk(), rope, scopes)?;

            if new_scope {
                scopes.pop();
            }
        }

        "binary_operator" => {
            let lhs = field(node, "lhs")?;
            let operator = field(node, "operator")?;
            let rhs = field(node, "rhs")?;

            if operator.kind() == "<-" || operator.kind() == "=" && lhs.kind() == "identifier" {
                traverse(&mut rhs.walk(), rope, scopes)?;
                let name = rope.byte_slice(lhs.byte_range()).to_string();
                scopes.declare(name, lhs);
            } else {
                traverse(&mut lhs.walk(), rope, scopes)?;
                traverse(&mut rhs.walk(), rope, scopes)?;
            }
        }

        "identifier" => {
            let name = rope.byte_slice(node.byte_range()).to_string();
            scopes.mark_unused(&name);
        }

        _ => {
            if cursor.goto_first_child() {
                loop {
                    traverse(cursor, rope, scopes)?;
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
    use {super::*, crate::tree};

    fn get_unused_names(code: &str) -> Vec<String> {
        let mut parser = tree::new_parser();
        let tree = tree::parse(&mut parser, code, None);
        let rope = Rope::from_str(code);

        let mut scopes = ScopeTree::new();
        traverse(&mut tree.root_node().walk(), &rope, &mut scopes).unwrap();
        // scopes.finalize_usage();

        scopes
            .get_unused_variables()
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }

    fn contains(unused: &[String], name: &str) -> bool {
        unused.iter().any(|unused| unused == name)
    }

    #[test]
    fn global_scope_variables_not_warned() {
        let code = r#"
        a <- 1
        b <- 2
        c <- 3
        "#;

        let unused = get_unused_names(code);
        assert_eq!(unused.len(), 0);
    }

    #[test]
    fn basic_used_variable() {
        let code = r#"
        function() {
            a <- 1
            b <- a
            b
        }
        "#;

        let unused = get_unused_names(code);
        assert!(!contains(&unused, "a"));
        assert!(!contains(&unused, "b"));
        assert_eq!(0, unused.len());
    }

    #[test]
    fn basic_unused_variable() {
        let code = r#"
        function() {
            a <- 1
            b_unused <- 2
            a
        }
        "#;

        let unused = get_unused_names(code);
        assert!(!contains(&unused, "a"));
        assert!(contains(&unused, "b_unused"));
    }

    #[test]
    fn variable_used_in_nested_function() {
        let code = r#"
        function() {
            a1 <- 1
            b1 <- function() {
                a1
            }
            b1()
        }
        "#;

        let unused = get_unused_names(code);
        assert!(!contains(&unused, "a1"));
        assert!(!contains(&unused, "b1"));
    }

    #[test]
    fn shadowed_variable() {
        let code = r#"
        function() {
            a_unused <- 1 # <- this should be unused
            a_unused <- 2 # <- this should be unused
            a <- 3
            a
        }
        "#;

        let unused = get_unused_names(code);
        assert!(contains(&unused, "a_unused"));
        assert_eq!(unused.len(), 2);
    }

    #[test]
    fn shadowed_variable_used() {
        let code = r#"
        function() {
            a <- 1 # <- this should be used
            a <- a + 1 # <- this should be used
            a <- a + 1
            a
        }
        "#;

        let unused = get_unused_names(code);
        assert_eq!(unused.len(), 0);
    }

    #[test]
    fn nested_shadowing() {
        let code = r#"
        function() {
            a <- 1
            a <- a + 1 # this should be unused
            local({
                a <- 2
                a
            })
        }
        "#;

        let unused = get_unused_names(code);
        assert!(contains(&unused, "a"));
        assert_eq!(unused.len(), 1);
    }

    // #[test]
    // fn chained_unused_variables() {
    //     let code = r#"
    //     function() {
    //         a_unused <- 1
    //         b_unused <- a_unused
    //         c_unused <- b_unused
    //     }
    //     "#;

    //     let unused = get_unused_names(code);
    //     assert!(contains(&unused, "a_unused"));
    //     assert!(contains(&unused, "b_unused"));
    //     assert!(contains(&unused, "c_unused"));
    // }

    #[test]
    fn chained_used_variables() {
        let code = r#"
        function() {
            a <- 1
            b <- a
            c_unused <- b
            b
        }
        "#;

        let unused = get_unused_names(code);
        assert!(!contains(&unused, "a"));
        assert!(!contains(&unused, "b"));
        assert!(contains(&unused, "c_unused"));
    }

    #[test]
    fn local_scope_variables() {
        let code = r#"
        function() {
            a1 <- 1
            local({
                a2 <- 2
                b2_unused <- 3
                print(a2)
            })
            a1
        }
        "#;

        let unused = get_unused_names(code);
        assert!(!contains(&unused, "a1"));
        assert!(!contains(&unused, "a2"));
        assert!(contains(&unused, "b2_unused"));
    }

    #[test]
    fn basic_parameters() {
        let code = r#"
        function(a, b_unused, c) {
            print(a)
            c
        }
        "#;

        let unused = get_unused_names(code);
        assert!(!contains(&unused, "a"));
        assert!(contains(&unused, "b_unused"));
        assert!(!contains(&unused, "c"));
    }

    #[test]
    fn parameters_shadowed() {
        let code = r#"
        function(a_unused) {
            b <- 1
            print(b)
        }
        "#;

        let unused = get_unused_names(code);
        assert!(contains(&unused, "a_unused"));
    }

    #[test]
    fn nested_scopes() {
        let code = r#"
        function() {
            a1 <- 1
            local({
                a2 <- 2
                local({
                    a3_unused <- 3
                    print(a1)
                    print(a2)
                })
                b2_unused <- 4
            })
            b1_unused <- 5
            print(a1)
        }
        "#;

        let unused = get_unused_names(code);
        assert!(!contains(&unused, "a1"));
        assert!(!contains(&unused, "a2"));
        assert!(contains(&unused, "a3_unused"));
        assert!(contains(&unused, "b2_unused"));
        assert!(contains(&unused, "b1_unused"));
    }

    #[test]
    fn nested_functions() {
        let code = r#"
        function() {
            a1 <- 1
            b1_unused <- 2
            c1 <- function() {
                a2 <- 3
                b2 <- function() {
                    a3_unused <- 4
                    print(a1)
                    print(a2)
                }
                b2
            }
            d1 <- c1()
            d1()
        }
        "#;

        let unused = get_unused_names(code);
        assert!(!contains(&unused, "a1"));
        assert!(contains(&unused, "b1_unused"));
        assert!(!contains(&unused, "a2"));
        assert!(contains(&unused, "a3_unused"));
        assert!(!contains(&unused, "c1"));
        assert!(!contains(&unused, "b2"));
        assert!(!contains(&unused, "d1"));
    }

    #[test]
    fn multiple_shadowing_levels() {
        let code = r#"
        function() {
            a_unused <- 1
            a_unused <- 2
            a_unused <- 3
            a_unused <- 4
            b <- 5
            print(b)
        }
        "#;

        let unused = get_unused_names(code);
        assert!(contains(&unused, "a_unused"));
        assert!(!contains(&unused, "b"));
        assert_eq!(unused.len(), 4);
    }

    #[test]
    fn conditional_usage() {
        let code = r#"
        function() {
            a <- 1
            b <- 2
            c_unused <- 3
            if (TRUE) {
                print(a)
            } else {
                print(b)
            }
        }
        "#;

        let unused = get_unused_names(code);
        assert!(!contains(&unused, "a"));
        assert!(!contains(&unused, "b"));
        assert!(contains(&unused, "c_unused"));
    }

    #[test]
    fn complex_nested_scopes() {
        let code = r#"
        function(a1, b1, c1_unused) {
            a2 <- 1
            b2 <- 2
            local({
                a3_unused <- 3
                b3_unused <- 4
                print(a1)
                print(a2)
            })

            c2 <- function(a4) {
                b4 <- a4 + b2
                print(b1)
                b4
            }

            d2 <- c2(5)
            print(d2)
        }
        "#;

        let unused = get_unused_names(code);
        assert!(!contains(&unused, "a1"));
        assert!(!contains(&unused, "b1"));
        assert!(contains(&unused, "c1_unused"));
        assert!(!contains(&unused, "a2"));
        assert!(!contains(&unused, "b2"));
        assert!(contains(&unused, "a3_unused"));
        assert!(contains(&unused, "b3_unused"));
        assert!(!contains(&unused, "c2"));
        assert!(!contains(&unused, "a4"));
        assert!(!contains(&unused, "b4"));
        assert!(!contains(&unused, "d2"));
    }
}
