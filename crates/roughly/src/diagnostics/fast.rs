use {
    crate::{
        config::Case,
        diagnostics::{self, Config},
        lsp_types::Diagnostic,
        utils,
    },
    ropey::Rope,
    tree_sitter::{Node, TreeCursor},
};

// note: we want these diagnostics even if there are syntax errors, therefore
// we cannot make any assumtions if certain fields exist or not
pub fn analyze(node: Node, rope: &Rope, config: Config) -> Vec<Diagnostic> {
    #[derive(Debug, Clone, Copy, Default)]
    struct State {
        check_trailing_commas: bool,
        check_case: bool,
    }

    impl State {
        fn check_trailing_commas(&mut self, check: bool) {
            self.check_trailing_commas = check;
        }
        fn check_case(&mut self, check: bool) {
            self.check_case = check;
        }
    }

    fn traverse(
        cursor: &mut TreeCursor,
        diagnostics: &mut Vec<Diagnostic>,
        rope: &Rope,
        config: Config,
        mut state: State,
    ) {
        let node = cursor.node();

        match node.kind() {
            "arguments" => {
                let mut last_comma = None;
                if cursor.goto_first_child() {
                    let mut last_argument = None;
                    loop {
                        let child = cursor.node();
                        match child.kind() {
                            "argument" => {
                                if let Some(last_argment) = last_argument
                                    && last_comma.is_none()
                                {
                                    diagnostics.push(diagnostics::error(
                                        last_argment,
                                        "Expected comma after argument".into(),
                                    ));
                                }
                                last_argument = Some(child);
                                last_comma = None;
                            }
                            "comma" => {
                                last_comma = Some(child);
                            }
                            _ => {}
                        }

                        if !cursor.goto_next_sibling() {
                            cursor.goto_parent();
                            break;
                        }
                    }

                    // note: we only check trailing commas for call not subset
                    if let Some(last_comma) = last_comma
                        && state.check_trailing_commas
                    {
                        diagnostics.push(diagnostics::error(
                            last_comma,
                            "Unexpected comma after last argument".into(),
                        ));
                    }
                }

                state.check_trailing_commas(false);
            }
            "binary_operator" => {
                if let (Some(lhs), Some(operator)) = (
                    node.child_by_field_name("lhs"),
                    node.child_by_field_name("operator"),
                ) && lhs.kind() == "identifier"
                    && operator.kind() == "<-"
                {
                    let raw = rope.byte_slice(lhs.byte_range()).to_string();
                    if state.check_case && config.case_linting {
                        let correct_case = match config.case {
                            Case::Camel => utils::to_camel_case(&raw),
                            Case::Snake => utils::to_snake_case(&raw),
                        };
                        if raw != correct_case {
                            diagnostics.push(diagnostics::warning(
                                node,
                                format!(
                                    "Variable `{}` should have {} name, e.g. {}",
                                    raw,
                                    match config.case {
                                        Case::Camel => "camelCase",
                                        Case::Snake => "snake_case",
                                    },
                                    correct_case
                                ),
                            ));
                        }
                    }
                }

                if let Some(operator) = node.child_by_field_name("operator")
                    && operator.kind() == "="
                {
                    diagnostics.push(diagnostics::warning(
                        node,
                        "Use <-, not =, for assignment".into(),
                    ));
                }
            }
            "call" => state.check_trailing_commas(true),
            "function_definition" => state.check_case(true),
            "parameter" => {
                if let Some(name) = node.child_by_field_name("name")
                    && name.kind() == "identifier"
                    && config.case_linting
                {
                    let raw = rope.byte_slice(name.byte_range()).to_string();
                    let correct_case = match config.case {
                        Case::Camel => utils::to_camel_case(&raw),
                        Case::Snake => utils::to_snake_case(&raw),
                    };
                    if raw != correct_case {
                        diagnostics.push(diagnostics::warning(
                            name,
                            format!(
                                "Parameter `{}` should have {} name, e.g. {}",
                                raw,
                                match config.case {
                                    Case::Camel => "camelCase",
                                    Case::Snake => "snake_case",
                                },
                                correct_case
                            ),
                        ));
                    }
                }
            }
            "identifier" => {
                let name = rope.byte_slice(node.byte_range()).to_string();
                let maybe_message = match name.as_str() {
                    "T" => Some("Use TRUE, not T, for Boolean values".into()),
                    "F" => Some("Use FALSE, not F, for Boolean values".into()),
                    _ => None,
                };
                if let Some(message) = maybe_message {
                    diagnostics.push(diagnostics::warning(node, message));
                }
            }
            _ => {}
        }

        if cursor.goto_first_child() {
            loop {
                traverse(cursor, diagnostics, rope, config, state);

                if !cursor.goto_next_sibling() {
                    cursor.goto_parent();
                    break;
                }
            }
        }
    }

    let mut diagnostics = Vec::new();
    traverse(
        &mut node.walk(),
        &mut diagnostics,
        rope,
        config,
        State::default(),
    );
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Case, tree};
    use ropey::Rope;

    fn analyze_code(code: &str, case: Case, case_linting: bool) -> Vec<Diagnostic> {
        let mut parser = tree::new_parser();
        let tree = tree::parse(&mut parser, code, None);
        let rope = Rope::from_str(code);
        let config = Config {
            case,
            case_linting,
            experimental: false,
        };
        
        analyze(tree.root_node(), &rope, config)
    }

    #[test]
    fn case_linting_disabled_by_default() {
        let code = r#"
        myVariable <- 10
        my_function <- function(someParam) {
            anotherVar <- someParam + 1
            anotherVar
        }
        "#;

        let diagnostics = analyze_code(code, Case::Snake, false);
        
        // Should not have any case-related warnings when disabled
        let case_warnings = diagnostics.iter().any(|d| {
            d.message.contains("should have") && (d.message.contains("camelCase") || d.message.contains("snake_case"))
        });
        assert!(!case_warnings, "No case warnings should be present when case_linting is disabled");
    }

    #[test]
    fn case_linting_snake_case_enabled() {
        let code = r#"
        myVariable <- 10
        my_function <- function(someParam) {
            anotherVar <- someParam + 1
            anotherVar
        }
        "#;

        let diagnostics = analyze_code(code, Case::Snake, true);
        
        // Should have warnings for camelCase parameters and variables inside functions when snake_case is enforced
        // Note: Global variables are not checked for case
        let param_warning = diagnostics.iter().any(|d| {
            d.message.contains("Parameter") && d.message.contains("someParam") && d.message.contains("snake_case")
        });
        let another_var_warning = diagnostics.iter().any(|d| {
            d.message.contains("Variable") && d.message.contains("anotherVar") && d.message.contains("snake_case")
        });
        
        assert!(param_warning, "Should warn about someParam not being snake_case");
        assert!(another_var_warning, "Should warn about anotherVar not being snake_case");
        
        // Global variables should not be checked for case
        let global_var_warning = diagnostics.iter().any(|d| {
            d.message.contains("myVariable")
        });
        assert!(!global_var_warning, "Global variables should not be checked for case");
    }

    #[test]
    fn case_linting_camel_case_enabled() {
        let code = r#"
        my_variable <- 10
        my_function <- function(some_param) {
            another_var <- some_param + 1
            another_var
        }
        "#;

        let diagnostics = analyze_code(code, Case::Camel, true);
        
        // Should have warnings for snake_case parameters and variables inside functions when camelCase is enforced
        // Note: Global variables are not checked for case
        let param_warning = diagnostics.iter().any(|d| {
            d.message.contains("Parameter") && d.message.contains("some_param") && d.message.contains("camelCase")
        });
        let another_var_warning = diagnostics.iter().any(|d| {
            d.message.contains("Variable") && d.message.contains("another_var") && d.message.contains("camelCase")
        });
        
        assert!(param_warning, "Should warn about some_param not being camelCase");
        assert!(another_var_warning, "Should warn about another_var not being camelCase");
        
        // Global variables should not be checked for case
        let global_var_warning = diagnostics.iter().any(|d| {
            d.message.contains("my_variable")
        });
        assert!(!global_var_warning, "Global variables should not be checked for case");
    }

    #[test]
    fn case_linting_correct_case_no_warnings() {
        let snake_code = r#"
        my_variable <- 10
        my_function <- function(some_param) {
            another_var <- some_param + 1
            another_var
        }
        "#;

        let camel_code = r#"
        myVariable <- 10
        myFunction <- function(someParam) {
            anotherVar <- someParam + 1
            anotherVar
        }
        "#;

        let snake_diagnostics = analyze_code(snake_code, Case::Snake, true);
        let camel_diagnostics = analyze_code(camel_code, Case::Camel, true);
        
        // Should not have case warnings when using correct case style
        let snake_case_warnings = snake_diagnostics.iter().any(|d| {
            d.message.contains("should have") && (d.message.contains("camelCase") || d.message.contains("snake_case"))
        });
        let camel_case_warnings = camel_diagnostics.iter().any(|d| {
            d.message.contains("should have") && (d.message.contains("camelCase") || d.message.contains("snake_case"))
        });
        
        assert!(!snake_case_warnings, "No case warnings for correct snake_case usage");
        assert!(!camel_case_warnings, "No case warnings for correct camelCase usage");
    }
}
