use {
    crate::{
        analysis::LintConfig,
        diagnostic::{Diagnostic, Lint},
        document::Document,
        tree::{field, kind},
    },
    ropey::Rope,
    tree_sitter::{Node, TreeCursor},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
pub enum NameStyle {
    #[serde(alias = "camelCase")]
    Camel,
    #[serde(alias = "snake_case")]
    Snake,
}

#[derive(Debug, Clone, Copy, Default)]
struct TraversalState {
    check_trailing_commas: bool,
    check_name_style: bool,
}

pub fn analyze(document: &Document, config: LintConfig) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    traverse(
        &mut document.tree().root_node().walk(),
        &mut diagnostics,
        document.rope(),
        config,
        TraversalState::default(),
    );
    diagnostics
}

fn traverse(
    cursor: &mut TreeCursor<'_>,
    diagnostics: &mut Vec<Diagnostic>,
    rope: &Rope,
    config: LintConfig,
    mut state: TraversalState,
) {
    let node = cursor.node();

    match node.kind_id() {
        kind::ARGUMENTS => {
            let mut last_comma = None;

            if cursor.goto_first_child() {
                let mut last_argument: Option<Node<'_>> = None;

                loop {
                    let child = cursor.node();

                    match child.kind_id() {
                        kind::ARGUMENT => {
                            if let Some(previous_argument) = last_argument
                                && last_comma.is_none()
                            {
                                diagnostics.push(Diagnostic::lint_error(
                                    Lint::MissingComma,
                                    previous_argument.range(),
                                    "Expected comma after argument",
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

                if let Some(trailing_comma) = last_comma
                    && state.check_trailing_commas
                {
                    diagnostics.push(Diagnostic::lint_error(
                        Lint::TrailingComma,
                        trailing_comma.range(),
                        "Unexpected comma after last argument",
                    ));
                }
            }

            state.check_trailing_commas = false;
        }
        kind::BINARY_OPERATOR => {
            if let Some(name_style) = config.naming_style
                && state.check_name_style
                && let Some(left_hand_side) = node.child_by_field_id(field::LHS)
                && left_hand_side.kind_id() == kind::IDENTIFIER
                && let Some(operator) = node.child_by_field_id(field::OPERATOR)
                && [kind::LEFT_ASSIGN, kind::EQUAL].contains(&operator.kind_id())
            {
                let actual_name = node_text(left_hand_side, rope);
                let expected_name = match name_style {
                    NameStyle::Camel => to_camel_case(&actual_name),
                    NameStyle::Snake => to_snake_case(&actual_name),
                };

                if actual_name != expected_name {
                    diagnostics.push(Diagnostic::lint_warning(
                        Lint::NamingStyle,
                        node.range(),
                        format!(
                            "Variable `{actual_name}` should have {} name, e.g. {expected_name}",
                            match name_style {
                                NameStyle::Camel => "camelCase",
                                NameStyle::Snake => "snake_case",
                            },
                        ),
                    ));
                }
            }

            if let Some(operator) = node.child_by_field_id(field::OPERATOR)
                && operator.kind_id() == kind::EQUAL
            {
                diagnostics.push(Diagnostic::lint_warning(
                    Lint::AssignmentOperator,
                    node.range(),
                    "Use <-, not =, for assignment",
                ));
            }
        }
        kind::CALL => {
            state.check_trailing_commas = true;
        }
        kind::FUNCTION_DEFINITION => {
            state.check_name_style = true;
        }
        kind::IDENTIFIER => {
            let name = node_text(node, rope);
            let message = match name.as_str() {
                "T" => Some("Use TRUE, not T, for Boolean values"),
                "F" => Some("Use FALSE, not F, for Boolean values"),
                _ => None,
            };

            if let Some(message) = message {
                diagnostics.push(Diagnostic::lint_warning(
                    Lint::BooleanShorthand,
                    node.range(),
                    message,
                ));
            }
        }
        kind::PARAMETER => {
            if let Some(name_style) = config.naming_style
                && state.check_name_style
                && let Some(name) = node.child_by_field_id(field::NAME)
                && name.kind_id() == kind::IDENTIFIER
            {
                let actual_name = node_text(name, rope);
                let expected_name = match name_style {
                    NameStyle::Camel => to_camel_case(&actual_name),
                    NameStyle::Snake => to_snake_case(&actual_name),
                };

                if actual_name != expected_name {
                    diagnostics.push(Diagnostic::lint_warning(
                        Lint::NamingStyle,
                        name.range(),
                        format!(
                            "Parameter `{actual_name}` should have {} name, e.g. {expected_name}",
                            match name_style {
                                NameStyle::Camel => "camelCase",
                                NameStyle::Snake => "snake_case",
                            },
                        ),
                    ));
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

fn node_text(node: Node<'_>, rope: &Rope) -> String {
    rope.byte_slice(node.byte_range()).to_string()
}

fn to_camel_case(name: &str) -> String {
    let mut camel_name = String::new();
    let mut characters = name.chars();

    if let Some(first_character) = characters.next() {
        camel_name.push(first_character);
    }

    let mut uppercase_next = false;
    for character in characters {
        if character == '_' {
            uppercase_next = true;
            continue;
        }

        if uppercase_next {
            uppercase_next = false;
            camel_name.extend(character.to_uppercase());
        } else {
            camel_name.push(character);
        }
    }

    camel_name
}

fn to_snake_case(name: &str) -> String {
    let mut snake_name = String::new();
    let mut previous_character = '_';

    for (index, character) in name.chars().enumerate() {
        if character.is_uppercase() {
            if index != 0 && previous_character != '_' {
                snake_name.push('_');
            }
            snake_name.extend(character.to_lowercase());
        } else {
            snake_name.push(character);
        }

        previous_character = character;
    }

    snake_name
}
