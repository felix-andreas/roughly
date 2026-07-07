use {
    crate::{
        config::{self, ExperimentalFeatures},
        diagnostics, format, index,
        lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString},
        position::PositionEncoding,
        server, tree, utils,
    },
    analysis::{self, Analysis, diagnostic::RelatedLocation},
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

/// Result of a CLI command, mapped by `main` onto the documented exit codes: `Clean` exits 0 and
/// `Findings` — diagnostics reported, or files a `fmt --check`/`--diff` run would change — exits 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Clean,
    Findings,
}

/// A usage, configuration, or I/O failure, already reported on stderr; `main` exits 2.
#[derive(Debug)]
pub struct CommandError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    /// Rendered diagnostics with source snippets, on stderr
    Human,
    /// One JSON object per diagnostic (JSON Lines), on stdout
    Json,
}

// The severity floor for `check` output: diagnostics below it are neither rendered nor counted
// toward the exit code, so `--min-severity error` makes warnings-only trees exit clean.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum MinSeverity {
    Warning,
    Error,
}

pub fn check(
    maybe_files: Option<&[PathBuf]>,
    output: OutputFormat,
    min_severity: MinSeverity,
) -> Result<Outcome, CommandError> {
    let root: Vec<PathBuf> = vec![".".into()];
    let files = maybe_files.unwrap_or(&root);

    let targets_with_config = files
        .iter()
        .map(|file| {
            let config = match config::Config::discover(file) {
                Ok(config) => config,
                Err(err) => {
                    error(&err.to_string());
                    return Err(CommandError);
                }
            };

            let target = std::fs::canonicalize(file).map_err(|err| {
                error(&format!("failed to resolve: {}", file.display()));
                eprintln!("{err}");
                CommandError
            })?;

            let paths = Walk::new(&target)
                .filter_map(|entry| match entry {
                    Ok(entry) => {
                        let path = entry.into_path();
                        path.extension()
                            .is_some_and(|ext| ext == "R" || ext == "r")
                            .then_some(Ok(path))
                    }
                    Err(err) => {
                        error(&err.to_string());
                        Some(Err(CommandError))
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;

            Ok((target, paths, config))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut n_files = 0;
    let mut n_diagnostics = 0;
    let mut n_failures = 0;
    for (target, paths, config) in targets_with_config {
        // One analysis per target keeps package-global naming and the project-global type
        // namespace intact across all files under that target.
        let root = analysis_root_for_target(&target);

        // A broken override stub silently changes what every file below checks against, so what the
        // loader drops is reported as findings before the per-file diagnostics: an unreadable
        // override file is an I/O failure (exit 2), and each dropped declaration is an error
        // diagnostic on its stub line. The same discovered sources feed the analysis, so the report
        // and the loaded corpus can never disagree.
        let project_stubs = analysis::stdlib::discover_project_stubs(&root);
        for (path, error_message) in &project_stubs.unreadable {
            n_failures += 1;
            error(&format!("failed to read override stub: {}", path.display()));
            eprintln!("{error_message}");
        }
        for (stub_source, problems) in
            project_stubs
                .sources
                .iter()
                .zip(analysis::stdlib::stub_override_problems(
                    &project_stubs.sources,
                ))
        {
            let rope = Rope::from_str(&stub_source.source);
            for problem in problems {
                n_diagnostics += 1;
                let diagnostic =
                    diagnostics::convert_stub_problem(&problem, &rope, PositionEncoding::Utf8);
                match output {
                    OutputFormat::Human => {
                        render_human_diagnostic(&stub_source.path, &rope, &diagnostic, &[])
                    }
                    OutputFormat::Json => {
                        render_json_diagnostic(&stub_source.path, &diagnostic, &[])
                    }
                }
            }
        }

        // NAMESPACE diagnostics are emitted after the per-file loop (below), because the
        // `unused-import` lint needs the token set of every checked source. The export table (for
        // the typo check) and the parsed imports are set up here from the same discovered sources
        // the analysis loads, so the two views can never disagree; `config.lint` is read before it
        // moves into the analysis.
        let namespace_path = root.join("NAMESPACE");
        let namespace_exports = analysis::stdlib::stub_names_by_namespace(&project_stubs.sources);
        let namespace_source = std::fs::read_to_string(&namespace_path).ok();
        let namespace_imports = namespace_source
            .as_deref()
            .map(analysis::namespace::parse_namespace_imports)
            .unwrap_or_default();
        let unused_import_level = config.lint.unused_import;

        let override_sources = project_stubs.sources.clone();
        let mut analysis_state =
            Analysis::new_with_stub_library(root, config.lint, config.check, move |interner| {
                analysis::stdlib::StubLibrary::load_with_overrides(interner, &override_sources)
            });

        let mut used_tokens = std::collections::BTreeSet::new();
        let mut checked_paths = Vec::with_capacity(paths.len());
        for path in paths {
            n_files += 1;
            let source = match std::fs::read_to_string(&path) {
                Ok(source) => source,
                Err(err) => {
                    n_failures += 1;
                    error(&format!("failed to read: {}", path.display()));
                    eprintln!("{err}");
                    continue;
                }
            };
            analysis::namespace::collect_used_tokens(&source, &mut used_tokens);
            if analysis_state
                .add_document_from_source(path.clone(), &source)
                .is_err()
            {
                n_failures += 1;
                error(&format!(
                    "failed to sync analysis document from source {}",
                    path.display()
                ));
                continue;
            }
            checked_paths.push(path);
        }

        analysis::run_full(&mut analysis_state);

        for path in checked_paths {
            let Some(document_id) = analysis_state.document_id_for_path(&path) else {
                n_failures += 1;
                error(&format!(
                    "analysis document not found after sync {}",
                    path.display()
                ));
                continue;
            };
            let document = analysis_state
                .document(&path)
                .unwrap_or_else(|| panic!("analysis document missing for {}", path.display()));
            let rope = document.rope();
            // Related locations point into *other* documents, so they render from their own
            // byte-based ranges rather than through this document's rope conversion.
            let raw_diagnostics = analysis_state.document_diagnostics(document_id);
            let related: Vec<Vec<RelatedLocation>> = raw_diagnostics
                .iter()
                .map(|diagnostic| diagnostic.related.clone())
                .collect();
            let diagnostics =
                diagnostics::convert_diagnostics(raw_diagnostics, rope, PositionEncoding::Utf8);

            for (diagnostic, related) in diagnostics.iter().zip(&related) {
                if min_severity == MinSeverity::Error
                    && diagnostic.severity != Some(DiagnosticSeverity::ERROR)
                {
                    continue;
                }
                n_diagnostics += 1;
                match output {
                    OutputFormat::Human => {
                        render_human_diagnostic(&path, rope, diagnostic, related)
                    }
                    OutputFormat::Json => render_json_diagnostic(&path, diagnostic, related),
                }
            }
        }

        // Package-level NAMESPACE diagnostics: the import-typo check (a warning per `importFrom`
        // naming something a known namespace does not export) and the opt-in `unused-import` lint
        // (an imported name that appears in no checked source's token set). Emitted last so the
        // lint sees the whole package.
        if let Some(namespace_source) = &namespace_source {
            let rope = Rope::from_str(namespace_source);
            let mut problems = analysis::namespace::namespace_import_problems(
                &namespace_imports,
                &namespace_exports,
            );
            problems.extend(analysis::namespace::unused_import_diagnostics(
                &namespace_imports,
                &used_tokens,
                unused_import_level,
            ));
            for diagnostic in
                diagnostics::convert_diagnostics(problems, &rope, PositionEncoding::Utf8)
            {
                if min_severity == MinSeverity::Error
                    && diagnostic.severity != Some(DiagnosticSeverity::ERROR)
                {
                    continue;
                }
                n_diagnostics += 1;
                match output {
                    OutputFormat::Human => {
                        render_human_diagnostic(&namespace_path, &rope, &diagnostic, &[])
                    }
                    OutputFormat::Json => render_json_diagnostic(&namespace_path, &diagnostic, &[]),
                }
            }
        }
    }

    if n_files == 0 {
        error("no R files found under the given path(s)");
        return Err(CommandError);
    }
    if n_failures > 0 {
        return Err(CommandError);
    }
    Ok(if n_diagnostics == 0 {
        Outcome::Clean
    } else {
        Outcome::Findings
    })
}

// Renders one diagnostic rustc-style on stderr: the message, a `--> path:line:column` header, the
// source line(s), a caret underline, and one `note:` line per related location. Rendered lines and
// columns are 1-based; the diagnostic's range stays 0-based internally.
fn render_human_diagnostic(
    path: &Path,
    rope: &Rope,
    diagnostic: &Diagnostic,
    related: &[RelatedLocation],
) {
    log(
        match diagnostic.severity {
            Some(DiagnosticSeverity::WARNING) => LogLevel::Warn,
            Some(DiagnosticSeverity::ERROR) => LogLevel::Error,
            _ => LogLevel::Info,
        },
        &diagnostic.message,
    );

    let range = diagnostic.range;
    // Diagnostic ranges come from analysis of this same rope, so they are in bounds; the clamp
    // only guards the render against a malformed range so it cannot panic.
    let last_line_index = rope.len_lines().saturating_sub(1);
    let start_line = usize::min(range.start.line as usize, last_line_index);
    let end_line = usize::min(range.end.line as usize, last_line_index);

    let gutter_width = (end_line + 1).to_string().len();
    eprintln!(
        "{}{} {}:{}:{}",
        " ".repeat(gutter_width),
        style("-->").bold().blue(),
        path.display(),
        range.start.line + 1,
        range.start.character + 1
    );

    for line_index in start_line..=end_line {
        let line = rope.line(line_index).to_string();
        eprintln!(
            "{} {}",
            style(format!(
                "{:<width$}|",
                line_index + 1,
                width = gutter_width + 1
            ))
            .blue()
            .bold(),
            line.trim_end_matches(['\n', '\r'])
        );
    }

    // The underline sits below the last rendered line, so it starts at the range's start column
    // only when the range is confined to a single line.
    let caret_column = if start_line == end_line {
        range.start.character as usize
    } else {
        0
    };
    let caret_width = usize::max(
        1,
        (range.end.character as usize).saturating_sub(caret_column),
    );
    eprintln!(
        "{}{}  {}",
        " ".repeat(gutter_width + 1),
        " ".repeat(caret_column),
        {
            let carets = style("^".repeat(caret_width)).bold();
            match diagnostic.severity {
                Some(DiagnosticSeverity::WARNING) => carets.yellow(),
                Some(DiagnosticSeverity::ERROR) => carets.red(),
                _ => carets,
            }
        }
    );
    // Related ranges live in *other* documents, whose text is not at hand here; their byte-based
    // positions render directly (line and byte column, both 1-based), matching the target document's
    // `-->` header for any plain-ASCII line.
    for entry in related {
        eprintln!(
            "{}{} {} {} {} {}:{}:{}",
            " ".repeat(gutter_width),
            style("=").bold().blue(),
            style("note:").bold(),
            entry.message,
            style("-->").bold().blue(),
            entry.path.display(),
            entry.range.start_point.row + 1,
            entry.range.start_point.column + 1,
        );
    }
    eprintln!();
}

// Renders one diagnostic as a JSON Lines record on stdout for CI use. Positions are 1-based like
// the human renderer; the field names are a documented contract (see the getting-started page).
fn render_json_diagnostic(path: &Path, diagnostic: &Diagnostic, related: &[RelatedLocation]) {
    let severity = match diagnostic.severity {
        Some(DiagnosticSeverity::WARNING) => "warning",
        Some(DiagnosticSeverity::INFORMATION) => "information",
        Some(DiagnosticSeverity::HINT) => "hint",
        _ => "error",
    };
    let code = diagnostic.code.as_ref().map(|code| match code {
        NumberOrString::Number(number) => number.to_string(),
        NumberOrString::String(string) => string.clone(),
    });
    let related = related
        .iter()
        .map(|entry| {
            serde_json::json!({
                "path": entry.path.display().to_string(),
                "line": entry.range.start_point.row + 1,
                "column": entry.range.start_point.column + 1,
                "endLine": entry.range.end_point.row + 1,
                "endColumn": entry.range.end_point.column + 1,
                "message": entry.message,
            })
        })
        .collect::<Vec<_>>();
    let record = serde_json::json!({
        "path": path.display().to_string(),
        "line": diagnostic.range.start.line + 1,
        "column": diagnostic.range.start.character + 1,
        "endLine": diagnostic.range.end.line + 1,
        "endColumn": diagnostic.range.end.character + 1,
        "severity": severity,
        "code": code,
        "message": diagnostic.message,
        "related": related,
    });
    println!("{record}");
}

fn analysis_root_for_target(target: &Path) -> PathBuf {
    if target.is_dir() {
        return target.to_path_buf();
    }

    // A file directly under an `R/` directory is a package source file; its package root is
    // the directory containing `R/`.
    match target.parent() {
        Some(parent) if parent.file_name().is_some_and(|name| name == "R") => parent
            .parent()
            .map(|root| root.to_path_buf())
            .unwrap_or_else(|| parent.to_path_buf()),
        Some(parent) => parent.to_path_buf(),
        None => PathBuf::from("."),
    }
}

//
// FMT
//

pub fn fmt(
    maybe_files: Option<&[PathBuf]>,
    check: bool,
    diff: bool,
    verbose: bool,
) -> Result<Outcome, CommandError> {
    let mut parser = tree::new_parser();

    let root: Vec<PathBuf> = vec![".".into()];
    let files = maybe_files.unwrap_or(&root);

    let paths_with_config = files
        .iter()
        .map(|file| {
            let config = config::Config::discover(file).map_err(|err| {
                error(&err.to_string());
                CommandError
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
                        Some(Err(CommandError))
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
                } else if let Err(err) = std::fs::write(&path, &new) {
                    n_errors += 1;
                    error(&format!("failed to write to file: {}", path.display()));
                    eprintln!("{err}");
                }
            }
        }
    }

    if n_files == 0 {
        error("no R files found under the given path(s)");
        return Err(CommandError);
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

    if n_errors > 0 {
        return Err(CommandError);
    }
    Ok(if (check || diff) && n_unformatted > 0 {
        Outcome::Findings
    } else {
        Outcome::Clean
    })
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

pub fn index(
    paths: Option<&[PathBuf]>,
    nested: bool,
    print_items: bool,
) -> Result<(), CommandError> {
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
                    Some(Err(CommandError))
                }
            })
            .collect::<Result<Vec<_>, _>>()?
        {
            let rope = utils::read_to_rope(&path).map_err(|err| {
                error(&format!("failed to index: {}", path.display()));
                eprintln!("{err}");
                CommandError
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
                        symbol.range.start.line_index,
                        symbol.range.start.character_index,
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
        return Err(CommandError);
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

pub fn ast(path: &Path) -> Result<(), CommandError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            error(&err.to_string());
            return Err(CommandError);
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
                features.range_formatting = true;
            }
            "debug" => {
                warn(
                    "The 'debug' feature is no longer experimental. Enable it with `debug = true` in roughly.toml.",
                );
            }
            "range_formatting" => features.range_formatting = true,
            "typing" => {
                warn(
                    "Type checking is no longer experimental: it powers IDE features by default. To also surface type-error diagnostics, set `[check]\ntyping = true` in roughly.toml.",
                );
            }
            "unused" => {
                warn(
                    "The 'unused' check is no longer experimental. Enable it with `[check]\nunused = true` in roughly.toml.",
                );
            }
            "goto_definition" | "goto_references" | "hovering" | "rename" => {
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
