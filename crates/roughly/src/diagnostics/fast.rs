use {
    crate::{
        config::Case,
        diagnostics::{self, Config},
        lsp_types::Diagnostic,
        tree::kind,
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
        let kind_id = node.kind_id();

        match kind_id {
            kind::ARGUMENTS => {
                let mut last_comma = None;
                if cursor.goto_first_child() {
                    let mut last_argument = None;
                    loop {
                        let child = cursor.node();
                        match child.kind_id() {
                            kind::ARGUMENT => {
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
                            kind::COMMA => {
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
            kind::BINARY_OPERATOR => {
                if let Some(case) = config.case
                    && state.check_case
                {
                    let maybe_lhs = node.child_by_field_name("lhs");
                    let maybe_operator = node.child_by_field_name("operator");
                    if let Some(lhs) = maybe_lhs
                        && lhs.kind_id() == kind::IDENTIFIER
                        && let Some(operator) = maybe_operator
                        && [kind::LEFT_ASSIGN, kind::EQUAL].contains(&operator.kind_id())
                    {
                        let actual = rope.byte_slice(lhs.byte_range()).to_string();
                        let expected = match case {
                            Case::Camel => utils::to_camel_case(&actual),
                            Case::Snake => utils::to_snake_case(&actual),
                        };
                        if actual != expected {
                            diagnostics.push(diagnostics::warning(
                                node,
                                format!(
                                    "Variable `{}` should have {} name, e.g. {}",
                                    actual,
                                    match case {
                                        Case::Camel => "camelCase",
                                        Case::Snake => "snake_case",
                                    },
                                    expected
                                ),
                            ));
                        }
                    }
                }

                if let Some(operator) = node.child_by_field_name("operator")
                    && operator.kind_id() == kind::EQUAL
                {
                    diagnostics.push(diagnostics::warning(
                        node,
                        "Use <-, not =, for assignment".into(),
                    ));
                }
            }
            kind::CALL => state.check_trailing_commas(true),
            kind::FUNCTION_DEFINITION => state.check_case(true),
            kind::IDENTIFIER => {
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
            kind::PARAMETER => {
                if let Some(case) = config.case
                    && state.check_case
                {
                    let maybe_name = node.child_by_field_name("name");
                    if let Some(name) = maybe_name
                        && name.kind_id() == kind::IDENTIFIER
                    {
                        let actual = rope.byte_slice(name.byte_range()).to_string();
                        let expected = match case {
                            Case::Camel => utils::to_camel_case(&actual),
                            Case::Snake => utils::to_snake_case(&actual),
                        };
                        if actual != expected {
                            diagnostics.push(diagnostics::warning(
                                name,
                                format!(
                                    "Parameter `{}` should have {} name, e.g. {}",
                                    actual,
                                    match case {
                                        Case::Camel => "camelCase",
                                        Case::Snake => "snake_case",
                                    },
                                    expected
                                ),
                            ));
                        }
                    }
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
