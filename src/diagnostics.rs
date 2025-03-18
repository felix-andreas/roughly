mod fast;
mod syntax;
mod unused;

use {
    crate::{
        cli::{self, LogLevel},
        config::{self, Case},
        tree,
    },
    console::style,
    ignore::Walk,
    ropey::Rope,
    std::path::PathBuf,
    tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range},
    tree_sitter::Node,
};

#[derive(Debug, Clone, Copy)]
pub struct Config {
    case: Case,
}

impl Config {
    pub fn from_config(config: config::Config) -> Self {
        Config { case: config.case }
    }
}

pub fn run(maybe_files: Option<&[PathBuf]>) -> Result<(), DiagnosticsError> {
    let root: Vec<PathBuf> = vec![".".into()];
    let files = maybe_files.unwrap_or(&root);

    let paths_with_config = files
        .iter()
        .map(|file| {
            let config = match config::Config::from_path(file) {
                Ok(config) => config,
                Err(error) => {
                    cli::error(&error.to_string());
                    return Err(DiagnosticsError);
                }
            };

            let paths = Walk::new(file)
                .filter_map(|entry| match entry {
                    Ok(entry) => {
                        let path = entry.into_path();
                        path.extension()
                            .is_some_and(|ext| ext == "R" || ext == "r")
                            .then_some(Ok(path))
                    }
                    Err(error) => {
                        cli::error(&error.to_string());
                        Some(Err(DiagnosticsError))
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;

            Ok((paths, config))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut n_files = 0;
    let mut n_errors = 0;
    for (paths, config) in paths_with_config {
        let config = Config { case: config.case };
        for path in paths {
            n_files += 1;
            let old = match std::fs::read_to_string(&path) {
                Ok(old) => old,
                Err(err) => {
                    n_errors += 1;
                    cli::error(&format!("failed to read: {}", path.display()));
                    eprintln!("{err}");
                    continue;
                }
            };
            let tree = tree::parse(&old, None);
            let rope = Rope::from_str(&old);

            for diagnostic in analyze_fast(tree.root_node(), &rope, config) {
                n_errors += 1;
                cli::log(
                    match diagnostic.severity {
                        Some(DiagnosticSeverity::INFORMATION) => LogLevel::Info,
                        Some(DiagnosticSeverity::WARNING) => LogLevel::Warning,
                        Some(DiagnosticSeverity::ERROR) => LogLevel::Error,
                        _ => LogLevel::Info,
                    },
                    &diagnostic.message,
                );
                let range = diagnostic.range;
                let padding_arrow = range.end.line.to_string().len();
                eprintln!(
                    "{}{} {}:{}:{}",
                    " ".repeat(padding_arrow),
                    style("-->").bold().blue(),
                    path.display(),
                    range.start.line,
                    range.start.character
                );

                let line_start = usize::max(1, range.start.line as usize) - 1;
                let lines = {
                    let start = rope.line_to_char(line_start);
                    let end =
                        rope.line_to_char(range.end.line as usize) + range.end.character as usize;
                    rope.slice(start..end)
                };
                let width = padding_arrow + 1;
                for (i, line) in lines.lines().enumerate() {
                    eprint!(
                        "{} {}",
                        style(format!("{:<width$}|", line_start + i)).blue().bold(),
                        line
                    );
                }
                eprintln!();

                let width_message = u32::max(
                    1,
                    if range.end.character > range.start.character {
                        range.end.character - range.start.character
                    } else {
                        range.start.character - range.end.character
                    },
                );
                eprintln!(
                    "{}{}  {}",
                    " ".repeat(width),
                    " ".repeat(usize::min(
                        range.start.character as usize,
                        range.end.character as usize
                    )),
                    {
                        let arrow = style("^".repeat(width_message as usize)).bold();
                        match diagnostic.severity {
                            Some(DiagnosticSeverity::INFORMATION) => arrow.blue(),
                            Some(DiagnosticSeverity::WARNING) => arrow.yellow(),
                            Some(DiagnosticSeverity::ERROR) => arrow.red(),
                            _ => arrow,
                        }
                    }
                );
                eprintln!(
                    "{}{}  {}",
                    " ".repeat(width),
                    " ".repeat(usize::min(
                        range.start.character as usize,
                        range.end.character as usize
                    )),
                    {
                        let message = style(&diagnostic.message).bold();
                        match diagnostic.severity {
                            Some(DiagnosticSeverity::INFORMATION) => message.blue(),
                            Some(DiagnosticSeverity::WARNING) => message.yellow(),
                            Some(DiagnosticSeverity::ERROR) => message.red(),
                            _ => message,
                        }
                    }
                );

                eprintln!("\n")
            }
        }
    }

    if n_files == 0 {
        cli::warning("No R files found under the given path(s)");
        return Err(DiagnosticsError);
    }

    if n_errors == 0 {
        Ok(())
    } else {
        Err(DiagnosticsError)
    }
}

#[derive(Debug)]
pub struct DiagnosticsError;

pub fn analyze_fast(node: Node, rope: &Rope, config: Config) -> Vec<Diagnostic> {
    let mut diagnostics = syntax::analyze(node, rope);
    diagnostics.extend(fast::analyze(node, rope, config));
    diagnostics
}

pub fn analyze_full(node: Node, rope: &Rope, config: Config) -> Vec<Diagnostic> {
    let mut diagnostics = analyze_fast(node, rope, config);
    diagnostics.extend(unused::analyze(node, rope));
    diagnostics
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
        range: node_range(node),
        code: None,
        code_description: None,
        source: None,
        related_information: None,
        tags: None,
        data: None,
    }
}

fn node_range(node: Node) -> Range {
    Range {
        start: Position {
            line: node.start_position().row as u32,
            character: node.start_position().column as u32,
        },
        end: Position {
            line: node.end_position().row as u32,
            character: node.end_position().column as u32,
        },
    }
}
