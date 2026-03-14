use {
    crate::{
        config::{self, ExperimentalFeatures},
        diagnostics, format, index,
        lsp_types::DiagnosticSeverity,
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
            let config = match config::Config::from_path(file, experimental_features) {
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

            for diagnostic in diagnostics::analyze_full(tree.root_node(), &rope, config.lint) {
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

pub fn fmt(
    maybe_files: Option<&[PathBuf]>,
    check: bool,
    diff: bool,
    verbose: bool,
    experimental_features: ExperimentalFeatures,
) -> Result<(), FmtError> {
    let mut parser = tree::new_parser();

    let root: Vec<PathBuf> = vec![".".into()];
    let files = maybe_files.unwrap_or(&root);

    let paths_with_config = files
        .iter()
        .map(|file| {
            let config = config::Config::from_path(file, experimental_features).map_err(|err| {
                error(&err.to_string());
                FmtError
            })?;

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

    let start_global = std::time::Instant::now();
    let mut elapsed_total = Duration::new(0, 0);
    let mut bytes_total = 0;

    let mut n_files = 0;
    let mut n_unformatted = 0;
    let mut n_errors = 0;
    for (paths, config) in paths_with_config {
        for path in paths {
            n_files += 1;

            let initial = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(err) => {
                    n_errors += 1;
                    error(&format!("failed to format: {}", path.display()));
                    eprintln!("{err}");
                    continue;
                }
            };

            let start = std::time::Instant::now();
            let tree = tree::parse(&mut parser, &initial, None);
            let rope = Rope::from_str(&initial);
            let new = match format::format(tree.root_node(), &rope, config.format) {
                Ok(new) => new,
                Err(err) => {
                    n_errors += 1;
                    error(&format!("failed to format: {}", path.display()));
                    eprintln!("{err}");
                    continue;
                }
            };
            elapsed_total += start.elapsed();
            bytes_total += rope.len_bytes();

            if initial != new {
                n_unformatted += 1;
                if diff {
                    eprintln!("Diff in {}:", path.display());
                    utils::print_diff(&initial, &new);
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

    if verbose {
        let global_elapsed = start_global.elapsed();
        info(&format!(
            "Formatted {} bytes in {} ms ({}/s) - including I/O: {} ms ({}/s)",
            utils::human_bytes(bytes_total as f64),
            elapsed_total.as_millis(),
            utils::human_bytes(bytes_total as f64 / elapsed_total.as_secs_f64()),
            global_elapsed.as_millis(),
            utils::human_bytes(bytes_total as f64 / global_elapsed.as_secs_f64())
        ));
    }

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

    let start_global = std::time::Instant::now();

    let mut n_files = 0;
    let mut n_symbols = 0;
    let mut n_bytes = 0;
    let mut elapsed_total = Duration::new(0, 0);

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
            n_bytes += bytes;
            n_files += 1;
            n_symbols += symbols.len();
            elapsed_total += elapsed;

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
                        style(match symbol.info {
                            index::ItemInfo::Unknown => "unknown",
                            index::ItemInfo::Integer => "integer",
                            index::ItemInfo::Float => "float",
                            index::ItemInfo::Complex => "complex",
                            index::ItemInfo::Bool => "bool",
                            index::ItemInfo::String => "string",
                            index::ItemInfo::Null => "null",
                            index::ItemInfo::Function => "function",
                            index::ItemInfo::S4Class => "S4Class",
                            index::ItemInfo::S4Generic => "S4Generic",
                            index::ItemInfo::S4Method { .. } => "S4Method",
                            index::ItemInfo::R6Class => "R6Class",
                            index::ItemInfo::R6Method => "R6Method",
                            index::ItemInfo::R6Field => "R6Field",
                        })
                        .italic()
                    );
                }
                eprintln!();
            }
        }
    }

    if n_files == 0 {
        warn("No R files found under the given path(s)");
        return Err(DebugError);
    }

    let elapsed_global = start_global.elapsed();

    info(&format!(
        "Indexed {} symbols from {} files in {} ms ({}/s) - including I/O: {} ms ({}/s)",
        n_symbols,
        n_files,
        elapsed_total.as_millis(),
        utils::human_bytes(n_bytes as f64 / elapsed_total.as_secs_f64()),
        elapsed_global.as_millis(),
        utils::human_bytes(n_bytes as f64 / elapsed_global.as_secs_f64())
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
    eprintln!("{}", tree::display_ast(tree.root_node()));
    Ok(())
}

//
// EXPERIMENTAL FEATURES
//

pub fn parse_experimental_flags(flags: &[impl AsRef<str>]) -> ExperimentalFeatures {
    // note: each flag may contain multiple features separated by spaces, e.g., "feature1 feature2"
    let mut features = ExperimentalFeatures::default();

    for flag in flags.iter().flat_map(|flag| flag.as_ref().split(' ')) {
        match flag {
            "all" => {
                features.goto_references = true;
                features.range_formatting = true;
                features.rename = true;
                features.unused = true;
                features.typing = true;
            }
            "goto_references" => features.goto_references = true,
            "range_formatting" => features.range_formatting = true,
            "rename" => features.rename = true,
            "unused" => features.unused = true,
            "typing" => features.typing = true,
            "goto_definition" => {
                warn(&format!(
                    "The '{flag}' flag has been stabilized. You can remove it."
                ));
            }
            unknown => {
                warn(&format!("unknown experimental feature: '{unknown}'"));
            }
        }
    }

    features
}
