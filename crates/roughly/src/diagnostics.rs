mod fast;
mod syntax;
mod unused;
mod typing;

#[cfg(test)]
mod typing_integration_tests;

use {
    crate::{
        config::{self, Case},
        lsp_types::{Diagnostic, DiagnosticSeverity},
        utils,
    },
    ropey::Rope,
    thiserror::Error,
    tree_sitter::Node,
};

#[derive(Debug, Clone, Copy)]
pub struct Config {
    case: Case,
    experimental: bool,
}

impl Config {
    pub fn from_config(config: config::Config, experimental: bool) -> Self {
        Config {
            case: config.case,
            experimental,
        }
    }
}

pub fn analyze(node: Node, rope: &Rope, config: Config, full: bool) -> Vec<Diagnostic> {
    let mut diagnostics = syntax::analyze(node, rope);
    let has_syntax_errors = !diagnostics.is_empty();

    diagnostics.extend(fast::analyze(node, rope, config));

    if full && !has_syntax_errors {
        #[allow(clippy::collapsible_if)]
        if config.experimental {
            match unused::analyze(node, rope) {
                Ok(diags) => diagnostics.extend(diags),
                Err(error) => {
                    tracing::warn!("error while diagnostics {error}");
                }
            }
            
            // Add type checking diagnostics
            match typing::analyze(node, rope) {
                Ok(diags) => diagnostics.extend(diags),
                Err(error) => {
                    tracing::warn!("error while type checking {error}");
                }
            }
        }
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
