use {
    crate::{
        analysis::LintConfig,
        analysis::LintLevel,
        diagnostic::{Diagnostic, DiagnosticCode, Lint, Severity},
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

// Pushes one lint finding, honoring its configured level: `off` drops it, `warn`/`error` force the
// severity, and the default keeps the lint's built-in severity. `naming-style` is configured by its
// style value instead (`None` disables it), so it always passes `Default` here.
fn push_lint(
    diagnostics: &mut Vec<Diagnostic>,
    config: LintConfig,
    lint: Lint,
    default_severity: Severity,
    range: tree_sitter::Range,
    message: impl Into<String>,
) {
    let level = match lint {
        Lint::MissingComma => config.missing_comma,
        Lint::TrailingComma => config.trailing_comma,
        Lint::AssignmentOperator => config.assignment_operator,
        Lint::BooleanShorthand => config.boolean_shorthand,
        Lint::NamingStyle => LintLevel::Default,
        // Not a syntax lint: emitted from naming output (`naming::unused_parameter_diagnostics`),
        // never through this tree-walk path.
        Lint::UnusedParameter => LintLevel::Off,
    };
    let severity = match level {
        LintLevel::Off => return,
        LintLevel::Default => default_severity,
        LintLevel::Warn => Severity::Warning,
        LintLevel::Error => Severity::Error,
    };
    diagnostics.push(Diagnostic {
        severity,
        code: DiagnosticCode::Lint(lint),
        message: message.into(),
        range,
        related: Vec::new(),
    });
}

// Iterative DFS with the shared cursor and one inherited state per ancestor depth. The linter also
// runs on tree-sitter error-recovery output, which nests as deeply as the source does, so a
// per-node recursion would overflow the stack on deeply nested documents.
fn traverse(
    cursor: &mut TreeCursor<'_>,
    diagnostics: &mut Vec<Diagnostic>,
    rope: &Rope,
    config: LintConfig,
    state: TraversalState,
) {
    let mut inherited = vec![state];
    loop {
        let mut state = *inherited.last().expect("root state present");
        visit_node(cursor, diagnostics, rope, config, &mut state);
        if cursor.goto_first_child() {
            inherited.push(state);
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return;
            }
            inherited.pop();
        }
    }
}

// One node's lint checks. Mutations to `state` apply to the node's descendants only; the cursor may
// scan the node's children (the arguments arm) but always returns to the node itself.
fn visit_node(
    cursor: &mut TreeCursor<'_>,
    diagnostics: &mut Vec<Diagnostic>,
    rope: &Rope,
    config: LintConfig,
    state: &mut TraversalState,
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
                                push_lint(
                                    diagnostics,
                                    config,
                                    Lint::MissingComma,
                                    Severity::Error,
                                    previous_argument.range(),
                                    "Expected comma after argument",
                                );
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
                    push_lint(
                        diagnostics,
                        config,
                        Lint::TrailingComma,
                        Severity::Error,
                        trailing_comma.range(),
                        "Unexpected comma after last argument",
                    );
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
                if let Some((range, message)) =
                    naming_style_finding(&actual_name, name_style, "Variable", node.range())
                {
                    diagnostics.push(Diagnostic::lint_warning(Lint::NamingStyle, range, message));
                }
            }

            if let Some(operator) = node.child_by_field_id(field::OPERATOR)
                && operator.kind_id() == kind::EQUAL
            {
                push_lint(
                    diagnostics,
                    config,
                    Lint::AssignmentOperator,
                    Severity::Warning,
                    node.range(),
                    "Use <-, not =, for assignment",
                );
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
                push_lint(
                    diagnostics,
                    config,
                    Lint::BooleanShorthand,
                    Severity::Warning,
                    node.range(),
                    message,
                );
            }
        }
        kind::PARAMETER => {
            if let Some(name_style) = config.naming_style
                && state.check_name_style
                && let Some(name) = node.child_by_field_id(field::NAME)
                && name.kind_id() == kind::IDENTIFIER
            {
                let actual_name = node_text(name, rope);
                if let Some((range, message)) =
                    naming_style_finding(&actual_name, name_style, "Parameter", name.range())
                {
                    diagnostics.push(Diagnostic::lint_warning(Lint::NamingStyle, range, message));
                }
            }
        }
        _ => {}
    }
}

// The naming-style finding for one identifier, shared by the variable and parameter sites: `None`
// when the name already matches the configured style.
fn naming_style_finding(
    actual_name: &str,
    name_style: NameStyle,
    subject: &str,
    range: tree_sitter::Range,
) -> Option<(tree_sitter::Range, String)> {
    let expected_name = match name_style {
        NameStyle::Camel => to_camel_case(actual_name),
        NameStyle::Snake => to_snake_case(actual_name),
    };
    if actual_name == expected_name {
        return None;
    }
    let style = match name_style {
        NameStyle::Camel => "camelCase",
        NameStyle::Snake => "snake_case",
    };
    Some((
        range,
        format!("{subject} `{actual_name}` should have {style} name, e.g. {expected_name}"),
    ))
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

#[cfg(test)]
mod tests {
    use {
        super::analyze,
        crate::{
            analysis::{LintConfig, LintLevel},
            diagnostic::Severity,
            document::Document,
            tree::new_parser,
        },
    };

    fn lint_with(config: LintConfig, source: &str) -> Vec<crate::diagnostic::Diagnostic> {
        let mut parser = new_parser().expect("parser");
        let document = Document::parse(&mut parser, source).expect("parse");
        analyze(&document, config)
    }

    #[test]
    fn a_lint_level_disables_and_escalates() {
        let source = "x = 1\n";
        let default_run = lint_with(LintConfig::default(), source);
        assert_eq!(default_run.len(), 1, "{default_run:?}");
        assert_eq!(default_run[0].severity, Severity::Warning);

        let off = lint_with(
            LintConfig {
                assignment_operator: LintLevel::Off,
                ..LintConfig::default()
            },
            source,
        );
        assert!(off.is_empty(), "{off:?}");

        let escalated = lint_with(
            LintConfig {
                assignment_operator: LintLevel::Error,
                ..LintConfig::default()
            },
            source,
        );
        assert_eq!(escalated.len(), 1);
        assert_eq!(escalated[0].severity, Severity::Error);
    }
}
