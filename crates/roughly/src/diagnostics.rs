mod fast;
mod syntax;
mod unused;

use {
    crate::{
        config::Case,
        lsp_types::{Diagnostic, DiagnosticSeverity},
        typing_diagnostics, utils,
    },
    ropey::Rope,
    serde::Deserialize,
    thiserror::Error,
    tree_sitter::Node,
};

#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Config {
    pub naming_style: Option<Case>,
    pub experimental_unused: bool,
    pub experimental_typing: bool,
}

pub fn analyze(node: Node, rope: &Rope, config: Config, full: bool) -> Vec<Diagnostic> {
    let mut diagnostics = syntax::analyze(node, rope);
    let has_syntax_errors = !diagnostics.is_empty();

    diagnostics.extend(fast::analyze(node, rope, config));

    if config.experimental_unused && full && !has_syntax_errors {
        match unused::analyze(node, rope) {
            Ok(diags) => diagnostics.extend(diags),
            Err(error) => {
                tracing::warn!("error while diagnostics {error}");
            }
        }
    }

    if config.experimental_typing && full {
        diagnostics.extend(typing_diagnostics::analyze_rope(rope));
    }

    diagnostics
}

pub fn analyze_fast(node: Node, rope: &Rope, config: Config) -> Vec<Diagnostic> {
    analyze(node, rope, config, false)
}

pub fn analyze_full(node: Node, rope: &Rope, config: Config) -> Vec<Diagnostic> {
    analyze(node, rope, config, true)
}

fn error(node: Node, message: String) -> Diagnostic {
    diag(node, message, DiagnosticSeverity::ERROR)
}

fn warning(node: Node, message: String) -> Diagnostic {
    diag(node, message, DiagnosticSeverity::WARNING)
}

fn diag(node: Node, message: String, severity: DiagnosticSeverity) -> Diagnostic {
    Diagnostic {
        message,
        severity: Some(severity),
        range: utils::node_range(node),
        code: None,
        code_description: None,
        source: None,
        related_information: None,
        tags: None,
        data: None,
    }
}

#[derive(Error, Debug)]
pub enum DiagnosticsError {
    #[error("Syntax error: Unexpected {kind} at line {line}, column {col}")]
    SyntaxError {
        kind: &'static str,
        line: usize,
        col: usize,
    },
}

pub fn field<'a>(node: Node<'a>, field_name: &'static str) -> Result<Node<'a>, DiagnosticsError> {
    node.child_by_field_name(field_name)
        .ok_or(DiagnosticsError::SyntaxError {
            kind: node.kind(),
            line: node.start_position().row,
            col: node.start_position().column,
        })
}

pub fn field_optional<'a>(node: Node<'a>, field_name: &'static str) -> Option<Node<'a>> {
    node.child_by_field_name(field_name)
}
