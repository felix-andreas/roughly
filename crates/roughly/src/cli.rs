//! The CLI commands. Exit codes are a documented contract: 0 clean, 1
//! findings (diagnostics, or files a `fmt --check`/`--diff` run would
//! change), 2 usage/configuration/IO errors.

use crate::config::{self, ExperimentalFeatures};
use crate::diagnostics::{apply_suppressions, document_diagnostics};
use crate::namespace;
use crate::position::LineIndex;
use console::style;
use ignore::Walk;
use semantics::diagnostics::{Diagnostic, Severity};
use semantics::{DocumentKind, ProjectFiles, RootDatabase, SourceFile};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

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

/// Result of a CLI command, mapped by `main` onto the documented exit codes:
/// `Clean` exits 0 and `Findings` exits 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Clean,
    Findings,
}

/// A usage, configuration, or I/O failure, already reported on stderr;
/// `main` exits 2.
#[derive(Debug)]
pub struct CommandError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    /// Rendered diagnostics with source snippets, on stderr
    Human,
    /// One JSON object per diagnostic (JSON Lines), on stdout
    Json,
}

/// The severity floor for `check` output: diagnostics below it are neither
/// rendered nor counted toward the exit code, so `--min-severity error`
/// makes warnings-only trees exit clean.
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
            let paths = collect_r_files(&target)?;
            Ok((target, paths, config))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut n_files = 0;
    let mut n_diagnostics = 0;
    let mut n_failures = 0;
    for (target, paths, config) in targets_with_config {
        let root = analysis_root_for_target(&target);

        // Project override stubs fold on top of the shipped corpus (later
        // sources win); an unreadable override is an I/O failure because it
        // silently changes what every file below checks against.
        let mut stub_sources = semantics::stubs::shipped_stub_sources();
        let mut project_stub_files: Vec<(PathBuf, String)> = Vec::new();
        match discover_project_stubs(&root) {
            Ok(project_stubs) => {
                for stub in project_stubs {
                    stub_sources.push((stub.stem, stub.text.clone()));
                    project_stub_files.push((stub.path, stub.text));
                }
            }
            Err((path, err)) => {
                n_failures += 1;
                error(&format!("failed to read override stub: {}", path.display()));
                eprintln!("{err}");
            }
        }

        let mut db = RootDatabase::default();
        semantics::stubs::StubSources::new(&db, stub_sources);

        // A broken override stub silently changes what every file below
        // checks against, so what the loader drops is reported as findings
        // before the per-file diagnostics: one whole-line error per dropped
        // declaration.
        for (stub_path, stub_text) in &project_stub_files {
            let stub_index = LineIndex::new(stub_text);
            for problem in semantics::stubs::stub_source_problems(&db, stub_text) {
                n_diagnostics += 1;
                let start = stub_index.line_start(problem.line as u32);
                let end = start + stub_index.line_length(problem.line as u32, stub_text);
                let diagnostic = Diagnostic {
                    range: syntax::TextRange::new(start.into(), end.into()),
                    severity: Severity::Error,
                    code: "stub",
                    message: problem.message,
                    related: Vec::new(),
                };
                match output {
                    OutputFormat::Human => {
                        render_human_diagnostic(stub_path, stub_text, &stub_index, &diagnostic, &[])
                    }
                    OutputFormat::Json => {
                        render_json_diagnostic(stub_path, &stub_index, &diagnostic, &[])
                    }
                }
            }
        }

        let namespace_path = root.join("NAMESPACE");
        let namespace_source = std::fs::read_to_string(&namespace_path).ok();
        let namespace_imports = namespace_source
            .as_deref()
            .map(namespace::parse_namespace_imports)
            .unwrap_or_default();
        let description_source = std::fs::read_to_string(root.join("DESCRIPTION")).ok();
        let dependencies = description_source
            .as_deref()
            .map(semantics::metadata::parse_description_dependencies)
            .unwrap_or_default();
        let collate = description_source
            .as_deref()
            .map(semantics::metadata::parse_description_collate)
            .unwrap_or_default();
        let collate_rank: HashMap<&str, usize> = collate
            .iter()
            .enumerate()
            .map(|(rank, name)| (name.as_str(), rank))
            .collect();
        let metadata = semantics::metadata::PackageMetadata::new(
            &db,
            semantics::metadata::normalized_imports(&namespace_imports),
            dependencies,
            BTreeSet::new(),
        );

        let r_path = root.join("R");
        let mut used_tokens = BTreeSet::new();
        let mut checked: Vec<(PathBuf, String)> = Vec::with_capacity(paths.len());
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
            namespace::collect_used_tokens(&source, &mut used_tokens);
            checked.push((path, source));
        }

        // Feed the project in the server's order — package files first, in
        // DESCRIPTION `Collate` order when declared (unlisted files after the
        // listed ones), then ascending by root-relative path — so the
        // last-writer-wins symbol index selects the same winners.
        // Diagnostics still print in discovery order below.
        let mut ordered: Vec<usize> = (0..checked.len()).collect();
        ordered.sort_by_key(|index| {
            let path = &checked[*index].0;
            let rank = path
                .file_name()
                .and_then(|name| collate_rank.get(name.to_string_lossy().as_ref()))
                .copied()
                .unwrap_or(usize::MAX);
            (
                !path.starts_with(&r_path),
                rank,
                path.strip_prefix(&root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .replace('\\', "/"),
            )
        });
        let mut files: Vec<Option<SourceFile>> = vec![None; checked.len()];
        let mut project = Vec::with_capacity(checked.len());
        for index in ordered {
            let (path, source) = &checked[index];
            let kind = if path.starts_with(&r_path) {
                DocumentKind::Package
            } else {
                DocumentKind::Script
            };
            let file = SourceFile::new(&db, source.clone(), kind);
            files[index] = Some(file);
            project.push(file);
        }
        ProjectFiles::new(&db, project.clone());
        let attached = semantics::metadata::attached_union(&db, project);
        if !attached.is_empty() {
            use salsa::Setter;
            metadata.set_attached(&mut db).to(attached);
        }

        let path_by_file: std::collections::HashMap<SourceFile, &PathBuf> = checked
            .iter()
            .enumerate()
            .filter_map(|(index, (path, _))| files[index].map(|file| (file, path)))
            .collect();
        // The cold pass fans out across cores: salsa storage-handle clones
        // share memos, so threads compute disjoint files concurrently (the
        // parallel stress suite gates this). Rendering stays sequential in
        // discovery order below.
        let workers = std::thread::available_parallelism()
            .map(std::num::NonZero::get)
            .unwrap_or(1)
            .min(checked.len().max(1));
        // Warm per-item naming (and the stub corpus) across cores first: the
        // project-wide interface walk demands naming for every item, and
        // computed inside that one salsa query it serializes the whole
        // front half of the cold pass on a single thread. With the memos
        // warm the walk is a cheap graph assembly. Files are dealt to
        // workers largest-first so one big file cannot skew a chunk.
        std::thread::scope(|scope| {
            {
                let db = db.clone();
                scope.spawn(move || {
                    let _ = semantics::stubs::stubs(&db);
                });
            }
            let mut by_size: Vec<usize> = (0..checked.len()).collect();
            by_size.sort_by_key(|&index| std::cmp::Reverse(checked[index].1.len()));
            for worker in 0..workers {
                let db = db.clone();
                let files = &files;
                let deal: Vec<usize> = by_size
                    .iter()
                    .copied()
                    .skip(worker)
                    .step_by(workers)
                    .collect();
                scope.spawn(move || {
                    for index in deal {
                        let Some(file) = files[index] else {
                            continue;
                        };
                        for item in semantics::item_tree(&db, file) {
                            let _ = semantics::item_naming(&db, item);
                        }
                    }
                });
            }
        });
        type FileFindings = Vec<(Diagnostic, Vec<RelatedNote>)>;
        let mut per_file: Vec<FileFindings> = (0..checked.len()).map(|_| Vec::new()).collect();
        std::thread::scope(|scope| {
            let mut pending: Vec<(usize, &mut FileFindings)> =
                per_file.iter_mut().enumerate().collect();
            let chunk_size = pending.len().div_ceil(workers.max(1)).max(1);
            let mut chunks = Vec::new();
            while !pending.is_empty() {
                let take = chunk_size.min(pending.len());
                chunks.push(pending.split_off(pending.len() - take));
            }
            for chunk in chunks {
                let db = db.clone();
                let checked = &checked;
                let files = &files;
                let path_by_file = &path_by_file;
                let config = &config;
                scope.spawn(move || {
                    for (index, slot) in chunk {
                        let (_, source) = &checked[index];
                        let file = files[index].expect("every checked file was fed to the project");
                        let rendered = document_diagnostics(&db, file, config);
                        let rendered = apply_suppressions(rendered, source);
                        for diagnostic in rendered {
                            // Related ranges live in other documents; they
                            // render from their own document's text.
                            let related: Vec<RelatedNote> = diagnostic
                                .related
                                .iter()
                                .filter_map(|related| {
                                    let related_path = path_by_file.get(&related.file)?;
                                    let related_index = LineIndex::new(related.file.text(&db));
                                    let start = related_index.line_column(related.range.start());
                                    Some(RelatedNote {
                                        path: (*related_path).clone(),
                                        line: start.line,
                                        column: start.column,
                                        message: related.message,
                                    })
                                })
                                .collect();
                            slot.push((diagnostic, related));
                        }
                    }
                });
            }
        });
        for (index, (path, source)) in checked.iter().enumerate() {
            let line_index = LineIndex::new(source);
            for (diagnostic, related) in &per_file[index] {
                if min_severity == MinSeverity::Error && diagnostic.severity != Severity::Error {
                    continue;
                }
                n_diagnostics += 1;
                match output {
                    OutputFormat::Human => {
                        render_human_diagnostic(path, source, &line_index, diagnostic, related)
                    }
                    OutputFormat::Json => {
                        render_json_diagnostic(path, &line_index, diagnostic, related)
                    }
                }
            }
        }

        // Package-level NAMESPACE diagnostics: the import-typo check (a
        // warning per `importFrom` naming something a known namespace does
        // not export) and the opt-in `unused-import` lint (an imported name
        // appearing in no checked source's token set). Emitted last so the
        // lint sees the whole package.
        if let Some(namespace_source) = &namespace_source {
            let knows =
                |package: &str| semantics::stubs::namespace_known(&db, package).unwrap_or(false);
            let exports =
                |package: &str, name: &str| semantics::stubs::namespace_exports(&db, package, name);
            let mut problems =
                namespace::namespace_import_problems(&namespace_imports, &knows, &exports);
            problems.extend(namespace::unused_import_diagnostics(
                &namespace_imports,
                &used_tokens,
                config.lint.unused_import,
            ));
            let index = LineIndex::new(namespace_source);
            for diagnostic in problems {
                if min_severity == MinSeverity::Error && diagnostic.severity != Severity::Error {
                    continue;
                }
                n_diagnostics += 1;
                match output {
                    OutputFormat::Human => render_human_diagnostic(
                        &namespace_path,
                        namespace_source,
                        &index,
                        &diagnostic,
                        &[],
                    ),
                    OutputFormat::Json => {
                        render_json_diagnostic(&namespace_path, &index, &diagnostic, &[])
                    }
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

fn collect_r_files(target: &Path) -> Result<Vec<PathBuf>, CommandError> {
    Walk::new(target)
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
        .collect()
}

/// One project override stub under `<root>/stubs/`: its path, file stem
/// (the namespace label), and text.
pub(crate) struct ProjectStub {
    pub(crate) path: PathBuf,
    pub(crate) stem: String,
    pub(crate) text: String,
}

/// The project override stubs under `<root>/stubs/*.Rtypes`, in path order.
pub(crate) fn discover_project_stubs(
    root: &Path,
) -> Result<Vec<ProjectStub>, (PathBuf, std::io::Error)> {
    let stubs_dir = root.join("stubs");
    let Ok(entries) = std::fs::read_dir(&stubs_dir) else {
        return Ok(Vec::new());
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "Rtypes")
        })
        .collect();
    paths.sort();
    let mut sources = Vec::new();
    for path in paths {
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let stem = path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_default();
                sources.push(ProjectStub { path, stem, text });
            }
            Err(err) => return Err((path, err)),
        }
    }
    Ok(sources)
}

/// One companion location of a diagnostic, resolved for rendering.
struct RelatedNote {
    path: PathBuf,
    line: u32,
    column: u32,
    message: &'static str,
}

/// Renders one diagnostic rustc-style on stderr: the message, a
/// `--> path:line:column` header, the source line(s), a caret underline, and
/// one `note:` line per related location. Rendered lines and columns are
/// 1-based byte positions.
fn render_human_diagnostic(
    path: &Path,
    source: &str,
    index: &LineIndex,
    diagnostic: &Diagnostic,
    related: &[RelatedNote],
) {
    log(
        match diagnostic.severity {
            Severity::Warning => LogLevel::Warn,
            Severity::Error => LogLevel::Error,
        },
        &diagnostic.message,
    );

    let start = index.line_column(diagnostic.range.start());
    let end = index.line_column(diagnostic.range.end());
    let gutter_width = (end.line as usize + 1).to_string().len();
    eprintln!(
        "{}{} {}:{}:{}",
        " ".repeat(gutter_width),
        style("-->").bold().blue(),
        path.display(),
        start.line + 1,
        start.column + 1
    );

    for line in start.line..=end.line {
        let line_start = index.line_start(line) as usize;
        let line_text = &source[line_start..line_start + index.line_length(line, source) as usize];
        eprintln!(
            "{} {}",
            style(format!("{:<width$}|", line + 1, width = gutter_width + 1))
                .blue()
                .bold(),
            line_text
        );
    }

    // The underline sits below the last rendered line, so it starts at the
    // range's start column only when the range is confined to a single line.
    let caret_column = if start.line == end.line {
        start.column as usize
    } else {
        0
    };
    let caret_width = usize::max(1, (end.column as usize).saturating_sub(caret_column));
    eprintln!(
        "{}{}  {}",
        " ".repeat(gutter_width + 1),
        " ".repeat(caret_column),
        {
            let carets = style("^".repeat(caret_width)).bold();
            match diagnostic.severity {
                Severity::Warning => carets.yellow(),
                Severity::Error => carets.red(),
            }
        }
    );
    for note in related {
        eprintln!(
            "{}{} {} {} {} {}:{}:{}",
            " ".repeat(gutter_width),
            style("=").bold().blue(),
            style("note:").bold(),
            note.message,
            style("-->").bold().blue(),
            note.path.display(),
            note.line + 1,
            note.column + 1,
        );
    }
    eprintln!();
}

/// Renders one diagnostic as a JSON Lines record on stdout for CI use.
/// Positions are 1-based like the human renderer; the field names are a
/// documented contract.
fn render_json_diagnostic(
    path: &Path,
    index: &LineIndex,
    diagnostic: &Diagnostic,
    related: &[RelatedNote],
) {
    let severity = match diagnostic.severity {
        Severity::Warning => "warning",
        Severity::Error => "error",
    };
    let start = index.line_column(diagnostic.range.start());
    let end = index.line_column(diagnostic.range.end());
    let related: Vec<serde_json::Value> = related
        .iter()
        .map(|note| {
            serde_json::json!({
                "path": note.path.display().to_string(),
                "line": note.line + 1,
                "column": note.column + 1,
                "message": note.message,
            })
        })
        .collect();
    let record = serde_json::json!({
        "path": path.display().to_string(),
        "line": start.line + 1,
        "column": start.column + 1,
        "endLine": end.line + 1,
        "endColumn": end.column + 1,
        "severity": severity,
        "code": diagnostic.code,
        "message": diagnostic.message,
        "related": related,
    });
    println!("{record}");
}

pub(crate) fn analysis_root_for_target(target: &Path) -> PathBuf {
    if target.is_dir() {
        return target.to_path_buf();
    }
    // A file directly under an `R/` directory is a package source file; its
    // package root is the directory containing `R/`.
    match target.parent() {
        Some(parent) if parent.file_name().is_some_and(|name| name == "R") => parent
            .parent()
            .map(|root| root.to_path_buf())
            .unwrap_or_else(|| parent.to_path_buf()),
        Some(parent) => parent.to_path_buf(),
        None => PathBuf::from("."),
    }
}

pub fn fmt(
    maybe_files: Option<&[PathBuf]>,
    check: bool,
    diff: bool,
    verbose: bool,
) -> Result<Outcome, CommandError> {
    let root: Vec<PathBuf> = vec![".".into()];
    let files = maybe_files.unwrap_or(&root);

    let paths_with_config = files
        .iter()
        .map(|file| {
            let config = config::Config::discover(file).map_err(|err| {
                error(&err.to_string());
                CommandError
            })?;
            let paths = collect_r_files(file)?;
            Ok((paths, config))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let start_global = std::time::Instant::now();
    let mut elapsed_total = std::time::Duration::ZERO;
    let mut bytes_total = 0usize;

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
            let new = match format::format(&initial, config.format) {
                Ok(new) => new,
                Err(err) => {
                    n_errors += 1;
                    error(&format!("failed to format: {}", path.display()));
                    eprintln!("{err}");
                    continue;
                }
            };
            elapsed_total += start.elapsed();
            bytes_total += initial.len();

            if initial != new {
                n_unformatted += 1;
                if diff {
                    eprintln!("Diff in {}:", path.display());
                    print_diff(&initial, &new);
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
            human_bytes(bytes_total as f64),
            elapsed_total.as_millis(),
            human_bytes(bytes_total as f64 / elapsed_total.as_secs_f64()),
            global_elapsed.as_millis(),
            human_bytes(bytes_total as f64 / global_elapsed.as_secs_f64())
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

pub fn server(experimental_features: ExperimentalFeatures) {
    crate::server::run(experimental_features);
}

pub fn ast(path: &Path) -> Result<(), CommandError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            error(&err.to_string());
            return Err(CommandError);
        }
    };
    let parse = syntax::parse(&text);
    eprintln!("{:#?}", parse.syntax_node());
    for error in parse.errors() {
        eprintln!(
            "error {:?}..{:?}: {}",
            u32::from(error.range.start()),
            u32::from(error.range.end()),
            error.message
        );
    }
    Ok(())
}

pub fn parse_experimental_flags(flags: &[impl AsRef<str>]) -> ExperimentalFeatures {
    let mut features = ExperimentalFeatures::default();
    for flag in flags.iter().flat_map(|flag| flag.as_ref().split(' ')) {
        match flag {
            "all" | "range_formatting" => features.range_formatting = true,
            "" => {}
            unknown => {
                warn(&format!("unknown experimental feature: '{unknown}'"));
            }
        }
    }
    features
}

fn print_diff(old: &str, new: &str) {
    use console::Style;
    use similar::{ChangeTag, TextDiff};

    struct Line(Option<usize>);
    impl std::fmt::Display for Line {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            match self.0 {
                None => write!(f, "    "),
                Some(index) => write!(f, "{:<4}", index + 1),
            }
        }
    }

    let diff = TextDiff::from_lines(old, new);
    for (index, group) in diff.grouped_ops(3).iter().enumerate() {
        if index > 0 {
            eprintln!("{:-^1$}", "-", 80);
        }
        for op in group {
            for change in diff.iter_inline_changes(op) {
                let (sign, sign_style) = match change.tag() {
                    ChangeTag::Delete => ("-", Style::new().red()),
                    ChangeTag::Insert => ("+", Style::new().green()),
                    ChangeTag::Equal => (" ", Style::new().dim()),
                };
                eprint!(
                    "{}{} |{}",
                    Line(change.old_index()),
                    Line(change.new_index()),
                    sign_style.apply_to(sign).bold(),
                );
                for (emphasized, value) in change.iter_strings_lossy() {
                    if emphasized {
                        eprint!("{}", sign_style.apply_to(value).underlined().on_black());
                    } else {
                        eprint!("{}", sign_style.apply_to(value));
                    }
                }
                if change.missing_newline() {
                    eprintln!();
                }
            }
        }
    }
}

fn human_bytes(bytes: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}
