use {
    crate::{
        config, diagnostics,
        format::{self, LineEnding},
        index,
        lsp_types::{self, DiagnosticSeverity},
        server, tree, utils,
    },
    console::style,
    ignore::Walk,
    ropey::Rope,
    std::{
        path::{Path, PathBuf},
        time::Duration,
    },
};

//
// LOG
//

#[derive(Debug, Clone, Copy)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

pub fn log(level: LogLevel, message: &str) {
    eprintln!(
        "{}{}",
        match level {
            LogLevel::Info => style(""),
            LogLevel::Warn => style("warning: ").yellow().bold(),
            LogLevel::Error => style("error: ").red().bold(),
        },
        style(message).bold(),
    );
}

pub fn info(message: &str) {
    log(LogLevel::Info, message);
}

pub fn warn(message: &str) {
    log(LogLevel::Warn, message);
}

pub fn error(message: &str) {
    log(LogLevel::Error, message);
}

//
// CHECK
//

#[derive(Debug)]
pub struct CheckError;

pub fn check(
    maybe_files: Option<&[PathBuf]>,
    experimental_features: ExperimentalFeatures,
) -> Result<(), CheckError> {
    let mut parser = tree::new_parser();

    let root: Vec<PathBuf> = vec![".".into()];
    let files = maybe_files.unwrap_or(&root);

    let paths_with_config = files
        .iter()
        .map(|file| {
            let config = match config::Config::from_path(file) {
                Ok(config) => config,
                Err(err) => {
                    error(&err.to_string());
                    return Err(CheckError);
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
                    Err(err) => {
                        error(&err.to_string());
                        Some(Err(CheckError))
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;

            Ok((paths, config))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut n_files = 0;
    let mut n_errors = 0;
    for (paths, config) in paths_with_config {
        let config = diagnostics::Config::from_config(config, experimental_features.unused);
        for path in paths {
            n_files += 1;
            let old = match std::fs::read_to_string(&path) {
                Ok(old) => old,
                Err(err) => {
                    n_errors += 1;
                    error(&format!("failed to read: {}", path.display()));
                    eprintln!("{err}");
                    continue;
                }
            };
            let tree = tree::parse(&mut parser, &old, None);
            let rope = Rope::from_str(&old);

            for diagnostic in diagnostics::analyze_full(tree.root_node(), &rope, config) {
                n_errors += 1;
                log(
                    match diagnostic.severity {
                        Some(DiagnosticSeverity::INFORMATION) => LogLevel::Info,
                        Some(DiagnosticSeverity::WARNING) => LogLevel::Warn,
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

                let width_message =
                    u32::max(1, range.end.character.abs_diff(range.start.character));
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
        warn("No R files found under the given path(s)");
        return Err(CheckError);
    }

    if n_errors == 0 {
        Ok(())
    } else {
        Err(CheckError)
    }
}

//
// FMT
//

#[derive(Debug)]
pub struct FmtError;

pub fn fmt(maybe_files: Option<&[PathBuf]>, check: bool, diff: bool) -> Result<(), FmtError> {
    let mut parser = tree::new_parser();

    let root: Vec<PathBuf> = vec![".".into()];
    let files = maybe_files.unwrap_or(&root);

    let paths_with_config = files
        .iter()
        .map(|file| {
            let config = match config::Config::from_path(file) {
                Ok(config) => config,
                Err(err) => {
                    error(&err.to_string());
                    return Err(FmtError);
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
                    Err(err) => {
                        error(&err.to_string());
                        Some(Err(FmtError))
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;

            Ok((paths, config))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut n_files = 0;
    let mut n_unformatted = 0;
    let mut n_errors = 0;
    for (paths, config) in paths_with_config {
        let config = format::Config {
            indent: &" ".repeat(config.spaces),
            line_ending: LineEnding::Auto,
        };
        for path in paths {
            n_files += 1;
            let old = match std::fs::read_to_string(&path) {
                Ok(old) => old,
                Err(err) => {
                    n_errors += 1;
                    error(&format!("failed to format: {}", path.display()));
                    eprintln!("{err}");
                    continue;
                }
            };
            let tree = tree::parse(&mut parser, &old, None);
            let rope = Rope::from_str(&old);
            let new = match format::format(tree.root_node(), &rope, config) {
                Ok(new) => new,
                Err(err) => {
                    n_errors += 1;
                    error(&format!("failed to format: {}", path.display()));
                    eprintln!("{err}");
                    continue;
                }
            };
            if old != new {
                n_unformatted += 1;
                if diff {
                    eprintln!("Diff in {}:", path.display());
                    utils::print_diff(&old, &new);
                } else if check {
                    eprintln!("Would reformat: {}", style(path.display()).bold());
                } else if std::fs::write(&path, &new).is_err() {
                    error(&format!("failed to write to file: {}", path.display()));
                }
            }
        }
    }

    if n_files == 0 {
        warn("No R files found under the given path(s)");
        return Err(FmtError);
    }

    let (action_format, action_skip) = if check || diff {
        ("would be reformatted", "already formatted")
    } else {
        ("reformatted", "left unchanged")
    };

    let n_unchanged = n_files - n_unformatted;
    info(&format!(
        "{} file{} {}, {} file{} {}",
        n_unformatted,
        if n_unformatted == 1 { "" } else { "s" },
        action_format,
        n_unchanged,
        if n_unchanged == 1 { "" } else { "s" },
        action_skip
    ));

    if n_errors > 0 || (check && n_unformatted > 0) {
        return Err(FmtError);
    }

    Ok(())
}

//
// SERVER
//

pub fn server(experimental_features: ExperimentalFeatures) {
    server::run(experimental_features);
}

//
// DEBUG
//

#[derive(Debug)]
pub struct DebugError;

pub fn index(paths: Option<&[PathBuf]>, nested: bool, print_items: bool) -> Result<(), DebugError> {
    let mut parser = tree::new_parser();

    let root: Vec<PathBuf> = vec![".".into()];
    let paths = paths.unwrap_or(&root);

    let global_start = std::time::Instant::now();

    let mut total_files = 0;
    let mut total_symbols = 0;
    let mut total_bytes = 0;
    let mut total_time = Duration::new(0, 0);

    for path in paths {
        for path in Walk::new(path)
            .filter_map(|entry| match entry {
                Ok(entry) => {
                    let path = entry.into_path();
                    path.extension()
                        .is_some_and(|ext| ext == "R" || ext == "r")
                        .then_some(Ok(path))
                }
                Err(err) => {
                    error(&err.to_string());
                    Some(Err(DebugError))
                }
            })
            .collect::<Result<Vec<_>, _>>()?
        {
            let rope = utils::read_to_rope(&path).map_err(|err| {
                error(&format!("failed to index: {}", path.display()));
                eprintln!("{err}");
                DebugError
            })?;

            // Only time the indexing operation, not the I/O
            let start = std::time::Instant::now();
            let tree = tree::parse_rope(&mut parser, &rope, None);
            let symbols = index::index(tree.root_node(), &rope, nested, false);
            let elapsed = start.elapsed();

            let bytes = rope.len_bytes();
            total_bytes += bytes;
            total_files += 1;
            total_symbols += symbols.len();
            total_time += elapsed;

            eprintln!(
                "{} ({}, {} ms, {}/s)",
                style(path.display().to_string()).bold().blue(),
                utils::human_bytes(bytes as f64),
                elapsed.as_millis(),
                utils::human_bytes(bytes as f64 / elapsed.as_secs_f64()),
            );

            if print_items {
                for symbol in &symbols {
                    eprintln!(
                        "    {:04}:{:03} {} ({})",
                        symbol.range.start.line,
                        symbol.range.start.character,
                        style(&symbol.name).bold(),
                        style(match symbol.kind {
                            lsp_types::SymbolKind::FUNCTION => "function",
                            lsp_types::SymbolKind::CLASS => "class",
                            lsp_types::SymbolKind::INTERFACE => "generic",
                            lsp_types::SymbolKind::METHOD => "method",
                            lsp_types::SymbolKind::VARIABLE => "variable",
                            _ => "other",
                        })
                        .italic()
                    );
                }
                eprintln!();
            }
        }
    }

    if total_files == 0 {
        warn("No R files found under the given path(s)");
        return Err(DebugError);
    }

    let global_elapsed = global_start.elapsed();

    info(&format!(
        "Indexed {} symbols from {} files in {} ms ({}/s) - including I/O: {} ms ({}/s)",
        total_symbols,
        total_files,
        total_time.as_millis(),
        utils::human_bytes(total_bytes as f64 / total_time.as_secs_f64()),
        global_elapsed.as_millis(),
        utils::human_bytes(total_bytes as f64 / global_elapsed.as_secs_f64())
    ));

    Ok(())
}

pub fn ast(path: &Path) -> Result<(), DebugError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            error(&err.to_string());
            return Err(DebugError);
        }
    };

    let tree = tree::parse(&mut tree::new_parser(), &text, None);
    eprintln!("{}", tree::format(tree.root_node()));
    Ok(())
}

//
// EXPERIMENTAL FEATURES
//

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExperimentalFeatures {
    pub range_formatting: bool,
    pub unused: bool,
}

impl ExperimentalFeatures {
    pub fn parse(flags: &[impl AsRef<str>]) -> Self {
        let mut unused = false;
        let mut range_formatting = false;

        for flag in flags {
            match flag.as_ref() {
                "all" => {
                    unused = true;
                    range_formatting = true;
                }
                "range_formatting" => range_formatting = true,
                "unused" => unused = true,
                unknown => {
                    warn(&format!("unknown experimental feature: {unknown}"));
                }
            }
        }

        Self {
            unused,
            range_formatting,
        }
    }
}
