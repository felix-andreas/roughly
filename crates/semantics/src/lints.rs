//! Style lints: syntax-level checks over the parse tree plus the
//! naming-driven unused-parameter lint.
//!
//! The legacy stack also carried a `missing-comma` lint compensating for
//! tree-sitter silently accepting `f(1 2)`; the hand parser rejects that
//! input with a syntax error (as R itself does), so the lint no longer
//! exists — the config key is still accepted for compatibility and has no
//! effect.

use crate::diagnostics::{Diagnostic, Severity};
use crate::{Db, SourceFile, item_naming, item_spans, parse};
use syntax::ast::AstNode;
use syntax::{SyntaxKind, SyntaxNode, TextRange};

/// A lint's configured level: keep its built-in severity, force one, or
/// disable it. The `[lint]` table keys each level by the lint's stable code
/// (`assignment-operator = "off"`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LintLevel {
    #[default]
    Default,
    Off,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
pub enum NameStyle {
    #[serde(alias = "camelCase")]
    Camel,
    #[serde(alias = "snake_case")]
    Snake,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct LintConfig {
    /// `naming-style` is configured by its style value (`None` disables it).
    pub naming_style: Option<NameStyle>,
    pub assignment_operator: LintLevel,
    pub boolean_shorthand: LintLevel,
    /// Obsolete: a missing argument comma is a parse error on this stack.
    /// Accepted so existing configs keep loading; never read.
    pub missing_comma: LintLevel,
    pub trailing_comma: LintLevel,
    /// Default-off: R signatures legitimately carry ignored formals (an S3
    /// method must match its generic), so a project opts in explicitly.
    pub unused_parameter: LintLevel,
    /// Default-off: a package may `importFrom` a name for re-export or a side
    /// effect, and usage detection is a conservative token scan.
    pub unused_import: LintLevel,
}

/// Every lint finding of one file, in position order. Pure syntax walks plus
/// per-item naming output; cheap enough to run unmemoized per publication.
pub fn lint_file(db: &dyn Db, file: SourceFile, config: &LintConfig) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let root = parse(db, file).syntax_node();
    syntax_lints(&root, config, &mut diagnostics);
    if let Some(severity) = opt_in_severity(config.unused_parameter) {
        for span in item_spans(db, file) {
            let Some(naming) = item_naming(db, span.item) else {
                continue;
            };
            for unused in &naming.unused_parameters {
                diagnostics.push(Diagnostic {
                    range: TextRange::new(
                        unused.range.start() + span.range.start(),
                        unused.range.end() + span.range.start(),
                    ),
                    severity: severity.clone(),
                    code: "unused-parameter",
                    message: format!("parameter `{}` is never used.", unused.name),
                    related: Vec::new(),
                });
            }
        }
    }
    diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start(), diagnostic.range.end()));
    diagnostics
}

fn syntax_lints(root: &SyntaxNode, config: &LintConfig, diagnostics: &mut Vec<Diagnostic>) {
    // Inherited state, maintained by enter/leave depth counters: name-style
    // checks apply only inside function definitions, and `#:` annotation
    // regions are type syntax, not R identifiers.
    let mut function_depth = 0usize;
    let mut annotation_depth = 0usize;
    for event in root.preorder_with_tokens() {
        match event {
            rowan::WalkEvent::Enter(element) => match element {
                rowan::NodeOrToken::Node(node) => match node.kind() {
                    SyntaxKind::FUNCTION_DEF => function_depth += 1,
                    SyntaxKind::ANNOTATION => annotation_depth += 1,
                    SyntaxKind::BINARY_EXPR => {
                        binary_lints(&node, config, function_depth > 0, diagnostics);
                    }
                    SyntaxKind::ARGUMENT_LIST => {
                        let call = node
                            .parent()
                            .is_some_and(|parent| parent.kind() == SyntaxKind::CALL_EXPR);
                        if call {
                            trailing_comma_lint(&node, config, diagnostics);
                        }
                    }
                    SyntaxKind::PARAMETER => {
                        parameter_name_lint(&node, config, diagnostics);
                    }
                    _ => {}
                },
                rowan::NodeOrToken::Token(token) => {
                    if token.kind() == SyntaxKind::IDENT && annotation_depth == 0 {
                        let message = match token.text() {
                            "T" => Some("Use TRUE, not T, for Boolean values"),
                            "F" => Some("Use FALSE, not F, for Boolean values"),
                            _ => None,
                        };
                        if let Some(message) = message {
                            push_lint(
                                diagnostics,
                                config.boolean_shorthand,
                                Severity::Warning,
                                "boolean-shorthand",
                                token.text_range(),
                                message,
                            );
                        }
                    }
                }
            },
            rowan::WalkEvent::Leave(element) => {
                if let rowan::NodeOrToken::Node(node) = element {
                    match node.kind() {
                        SyntaxKind::FUNCTION_DEF => function_depth -= 1,
                        SyntaxKind::ANNOTATION => annotation_depth -= 1,
                        _ => {}
                    }
                }
            }
        }
    }
}

fn binary_lints(
    node: &SyntaxNode,
    config: &LintConfig,
    inside_function: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(binary) = syntax::ast::BinaryExpr::cast(node.clone()) else {
        return;
    };
    let Some(operator) = binary.operator() else {
        return;
    };
    let range = expression_range(node);
    if operator.kind() == SyntaxKind::EQ {
        push_lint(
            diagnostics,
            config.assignment_operator,
            Severity::Warning,
            "assignment-operator",
            range,
            "Use <-, not =, for assignment",
        );
    }
    if inside_function
        && let Some(style) = config.naming_style
        && matches!(operator.kind(), SyntaxKind::LESS_MINUS | SyntaxKind::EQ)
        && let Some(target) = binary
            .lhs()
            .filter(|target| target.kind() == SyntaxKind::NAME)
    {
        let name = target.text().to_string();
        if let Some(message) = naming_style_message(&name, style, "Variable") {
            diagnostics.push(Diagnostic {
                range,
                severity: Severity::Warning,
                code: "naming-style",
                message,
                related: Vec::new(),
            });
        }
    }
}

fn trailing_comma_lint(
    argument_list: &SyntaxNode,
    config: &LintConfig,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut last_comma = None;
    for element in argument_list.children_with_tokens() {
        match element {
            rowan::NodeOrToken::Token(token) if token.kind() == SyntaxKind::COMMA => {
                last_comma = Some(token.text_range());
            }
            rowan::NodeOrToken::Node(node) if node.kind() == SyntaxKind::ARGUMENT => {
                last_comma = None;
            }
            _ => {}
        }
    }
    if let Some(range) = last_comma {
        push_lint(
            diagnostics,
            config.trailing_comma,
            Severity::Error,
            "trailing-comma",
            range,
            "Unexpected comma after last argument",
        );
    }
}

fn parameter_name_lint(
    parameter: &SyntaxNode,
    config: &LintConfig,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(style) = config.naming_style else {
        return;
    };
    let Some(name_token) = parameter
        .children_with_tokens()
        .filter_map(|element| element.into_token())
        .find(|token| token.kind() == SyntaxKind::IDENT)
    else {
        return;
    };
    if let Some(message) = naming_style_message(name_token.text(), style, "Parameter") {
        diagnostics.push(Diagnostic {
            range: name_token.text_range(),
            severity: Severity::Warning,
            code: "naming-style",
            message,
            related: Vec::new(),
        });
    }
}

/// The naming-style finding for one identifier, shared by the variable and
/// parameter sites: `None` when the name already matches the configured style.
fn naming_style_message(actual: &str, style: NameStyle, subject: &str) -> Option<String> {
    let expected = match style {
        NameStyle::Camel => to_camel_case(actual),
        NameStyle::Snake => to_snake_case(actual),
    };
    if actual == expected {
        return None;
    }
    let style = match style {
        NameStyle::Camel => "camelCase",
        NameStyle::Snake => "snake_case",
    };
    Some(format!(
        "{subject} `{actual}` should have {style} name, e.g. {expected}"
    ))
}

fn push_lint(
    diagnostics: &mut Vec<Diagnostic>,
    level: LintLevel,
    default_severity: Severity,
    code: &'static str,
    range: TextRange,
    message: impl Into<String>,
) {
    let severity = match level {
        LintLevel::Off => return,
        LintLevel::Default => default_severity,
        LintLevel::Warn => Severity::Warning,
        LintLevel::Error => Severity::Error,
    };
    diagnostics.push(Diagnostic {
        range,
        severity,
        code,
        message: message.into(),
        related: Vec::new(),
    });
}

/// The opted-in severity of a default-off lint: `Default` behaves as `Off`.
fn opt_in_severity(level: LintLevel) -> Option<Severity> {
    match level {
        LintLevel::Default | LintLevel::Off => None,
        LintLevel::Warn => Some(Severity::Warning),
        LintLevel::Error => Some(Severity::Error),
    }
}

/// A node's range without the trailing trivia some expression nodes swallow
/// (a trailing comment or newline attaches inside).
fn expression_range(node: &SyntaxNode) -> TextRange {
    let mut end = node.text_range().end();
    let mut token = node.last_token();
    while let Some(current) = token {
        if current.kind().is_trivia() {
            end = current.text_range().start();
            token = current
                .prev_token()
                .filter(|previous| previous.text_range().start() >= node.text_range().start());
        } else {
            break;
        }
    }
    TextRange::new(node.text_range().start(), end)
}

fn to_camel_case(name: &str) -> String {
    let mut camel = String::new();
    let mut characters = name.chars();
    if let Some(first) = characters.next() {
        camel.push(first);
    }
    let mut uppercase_next = false;
    for character in characters {
        if character == '_' {
            uppercase_next = true;
            continue;
        }
        if uppercase_next {
            uppercase_next = false;
            camel.extend(character.to_uppercase());
        } else {
            camel.push(character);
        }
    }
    camel
}

fn to_snake_case(name: &str) -> String {
    let mut snake = String::new();
    let mut previous = '_';
    for (index, character) in name.chars().enumerate() {
        if character.is_uppercase() {
            if index != 0 && previous != '_' {
                snake.push('_');
            }
            snake.extend(character.to_lowercase());
        } else {
            snake.push(character);
        }
        previous = character;
    }
    snake
}
