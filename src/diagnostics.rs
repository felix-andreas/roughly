use {
    crate::{
        cli,
        config::{self, Case},
        tree,
    },
    console::style,
    ignore::Walk,
    inflections::case,
    ropey::Rope,
    std::path::PathBuf,
    tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range},
    tree_sitter::{Node, TreeCursor},
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

pub fn run(maybe_files: Option<&[PathBuf]>) -> Result<(), ()> {
    let root: Vec<PathBuf> = vec![".".into()];
    let files = maybe_files.unwrap_or(&root);

    let config = match config::Config::from_path(&files.first().unwrap()) {
        Ok(config) => config,
        Err(err) => {
            cli::error(&err.to_string());
            return Err(());
        }
    };

    let mut n_files = 0;
    let mut n_errors = 0;
    for path in files
        .iter()
        .flat_map(Walk::new)
        .filter_map(|e| e.ok())
        .map(|entry| entry.into_path())
        .filter(|path| {
            path.extension()
                .map(|ext| ext == "R" || ext == "r")
                .unwrap_or(false)
        })
    {
        n_files += 1;
        let old = match std::fs::read_to_string(&path) {
            Ok(old) => old,
            Err(err) => {
                n_errors += 1;
                cli::error(&format!("failed to format: {}", path.display()));
                eprintln!("{err}");
                continue;
            }
        };
        let tree = tree::parse(&old, None);
        let rope = Rope::from_str(&old);

        for diagnostic in diagnostics(tree.root_node(), &rope, Config { case: config.case }) {
            n_errors += 1;
            cli::error(&format!("{}", diagnostic.message));
            let range = diagnostic.range;
            eprintln!(
                "{} {}:{}:{}",
                style("  -->").bold().blue(),
                path.display(),
                range.start.line,
                range.start.character
            );

            let start_byte = rope.line_to_char(if range.start.line == 0 {
                0
            } else {
                range.start.line - 1
            } as usize);
            let end_byte = rope
                .try_char_to_byte(
                    rope.line_to_char(range.end.line as usize) + range.end.character as usize,
                )
                .unwrap();

            let lines = rope.slice(start_byte..end_byte);
            let width = range.end.line.to_string().len() + 1;
            let width_error = u32::max(
                1,
                if range.end.character > range.start.character {
                    range.end.character - range.start.character
                } else {
                    range.start.character - range.end.character
                },
            );
            for (i, line) in lines.lines().enumerate() {
                eprint!("{} {}", style(format!("{i:<width$}|")).blue().bold(), line);
            }
            eprintln!();
            eprintln!(
                "{}{}  {}",
                " ".repeat(width),
                " ".repeat(usize::min(
                    range.start.character as usize,
                    range.end.character as usize
                )),
                style("^".repeat(width_error as usize)).red().bold()
            );
            eprintln!(
                "{}{}  {}",
                " ".repeat(width),
                " ".repeat(usize::min(
                    range.start.character as usize,
                    range.end.character as usize
                )),
                style(&diagnostic.message).red().bold()
            );

            eprintln!("\n")
        }
    }

    if n_files == 0 {
        cli::warning("No R files found under the given path(s)");
        return Err(());
    }

    if n_errors == 0 { Ok(()) } else { Err(()) }
}

pub fn diagnostics(node: Node, rope: &Rope, config: Config) -> Vec<Diagnostic> {
    let mut diagnostics = diagnostics_syntax(node, &rope);
    diagnostics.extend(diagnostics_semantics(node, &rope, config));
    diagnostics
}

pub fn diagnostics_syntax(node: Node, rope: &Rope) -> Vec<Diagnostic> {
    fn traverse(cursor: &mut TreeCursor, diagnostics: &mut Vec<Diagnostic>, rope: &Rope) -> bool {
        let node = cursor.node();
        if !(node.is_error() || node.has_error()) {
            return false;
        }

        // DEBUG
        // eprintln!(
        //     "{}_{:?}_{}",
        //     node.kind(),
        //     node.has_error(),
        //     rope.byte_slice(node.byte_range()),
        // );

        match node.kind() {
            "arguments"
            | "braced_expression"
            | "for_statement"
            | "if_statement"
            | "parameters"
            | "parenthesized_expression"
            | "while_statement" => {
                if let Some(open) = node.child_by_field_name("open") {
                    if let Some(close) = node.child_by_field_name("close") {
                        if close.is_missing() {
                            diagnostics.push(error(
                                open,
                                format!("missing closing delimiter {}", close.kind()),
                            ));
                        }
                    }
                }
            }
            "binary_operator" => {
                if let Some(operator) = node.child_by_field_name("operator") {
                    if let Some(rhs) = node.child_by_field_name("rhs") {
                        if rhs.is_missing() {
                            diagnostics.push(error(
                                operator,
                                format!("missing rhs for operator {}", operator.kind()),
                            ));
                        }
                    }
                }
            }
            "function_definition" => {
                if let Some(body) = node.child_by_field_name("body") {
                    if body.is_missing() {
                        diagnostics.push(error(node, format!("missing function body")));
                    }
                }
            }
            _ => {}
        }

        let mut handled_error = false;
        // let child_kinds = vec![];
        if cursor.goto_first_child() {
            if node.is_error() {
                let child = cursor.node();
                match child.kind() {
                    "(" | "{" | "[" | "[[" => diagnostics.push(error(
                        child,
                        format!("missing closing delimiter {}", child.kind()),
                    )),
                    _ => {}
                }
            }

            loop {
                handled_error |= traverse(cursor, diagnostics, rope);

                if !cursor.goto_next_sibling() {
                    break;
                }
            }

            cursor.goto_parent();
        }

        if !handled_error && node.is_error() {
            handled_error = true;
            let raw = rope.byte_slice(node.byte_range()).to_string();
            match raw.as_str() {
                ")" | "}" | "]" | "]]" => diagnostics.push(error(
                    node,
                    format!("Syntax Error: unexpected closing delimiter {}", raw),
                )),
                _ => {
                    diagnostics.push(error(node, format!("Syntax Error: unexpected {:?}", raw)));
                }
            }
        }

        handled_error
    }

    let mut diagnostics = Vec::new();
    let mut cursor = node.walk();
    traverse(&mut cursor, &mut diagnostics, rope);
    diagnostics
}

pub fn diagnostics_semantics(node: Node, rope: &Rope, config: Config) -> Vec<Diagnostic> {
    fn diag(node: Node, message: String) -> Diagnostic {
        Diagnostic {
            message: message,
            severity: Some(DiagnosticSeverity::WARNING),
            range: node_range(node),
            code: None,
            code_description: None,
            source: None,
            related_information: None,
            tags: None,
            data: None,
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct State {
        check_traing_commas: bool,
        check_case: bool,
    }

    impl State {
        fn check_traing_commas(&mut self, check: bool) {
            self.check_traing_commas = check;
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
                if state.check_traing_commas {
                    state.check_traing_commas(false);
                    if cursor.goto_last_child() {
                        while cursor.goto_previous_sibling() && cursor.node().kind() == "comment" {}
                        if cursor.node().kind() == "comma" {
                            diagnostics
                                .push(diag(cursor.node(), "trailing comma in arguments".into()));
                        }
                        cursor.goto_parent();
                    }
                }
            }
            "binary_operator" => {
                if let Some(lhs) = node.child_by_field_name("lhs") {
                    if lhs.kind() == "identifier" {
                        let name = rope.byte_slice(lhs.byte_range()).to_string();
                        if state.check_case {
                            let correct_case = match config.case {
                                Case::Camel => case::to_camel_case(&name),
                                Case::Snake => case::to_snake_case(&name),
                            };
                            if name != correct_case {
                                diagnostics.push(diag(
                                    node,
                                    format!(
                                        "Variable '{}' should have {} name, e.g. {}",
                                        name,
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
                }
                if let Some(operator) = node.child_by_field_name("operator") {
                    if operator.kind() == "=" {
                        diagnostics.push(diag(node, format!("Use <-, not =, for assignment")));
                    }
                }
            }
            "call" => state.check_traing_commas(true),
            "function_definition" => state.check_case(true),
            _ => {}
        }

        if cursor.goto_first_child() {
            loop {
                traverse(cursor, diagnostics, rope, config, state);

                if !cursor.goto_next_sibling() {
                    break;
                }
            }

            cursor.goto_parent();
        }
    }

    let mut diagnostics = Vec::new();
    let mut cursor = node.walk();
    traverse(
        &mut cursor,
        &mut diagnostics,
        rope,
        config,
        State::default(),
    );
    diagnostics
}

fn error(node: Node, message: String) -> Diagnostic {
    diag(node, message, DiagnosticSeverity::ERROR)
}

fn diag(node: Node, message: String, severity: DiagnosticSeverity) -> Diagnostic {
    Diagnostic {
        message: message,
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
