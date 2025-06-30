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
                    if state.check_case {
                        if let Some(case) = config.case {
                            let correct_case = match case {
                                Case::Camel => utils::to_camel_case(&raw),
                                Case::Snake => utils::to_snake_case(&raw),
                            };
                            if raw != correct_case {
                                diagnostics.push(diagnostics::warning(
                                    node,
                                    format!(
                                        "Variable `{}` should have {} name, e.g. {}",
                                        raw,
                                        match case {
                                            Case::Camel => "camelCase",
                                            Case::Snake => "snake_case",
                                        },
                                        correct_case
                                    ),
                                ));
                            }
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
                {
                    if let Some(case) = config.case {
                        let raw = rope.byte_slice(name.byte_range()).to_string();
                        let correct_case = match case {
                            Case::Camel => utils::to_camel_case(&raw),
                            Case::Snake => utils::to_snake_case(&raw),
                        };
                        if raw != correct_case {
                            diagnostics.push(diagnostics::warning(
                                name,
                                format!(
                                    "Parameter `{}` should have {} name, e.g. {}",
                                    raw,
                                    match case {
                                        Case::Camel => "camelCase",
                                        Case::Snake => "snake_case",
                                    },
                                    correct_case
                                ),
                            ));
                        }
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
