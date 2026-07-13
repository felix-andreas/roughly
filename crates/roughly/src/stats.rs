//! `roughly analysis-stats`: a workspace performance diagnosis. Runs the full analysis pipeline
//! over a workspace through the same query engine the language server uses and prints where the
//! time goes — per-phase totals, the slowest files, and an incremental edit probe — plus where the
//! memory goes (per-phase resident-set growth), so a slow or memory-hungry workspace can be
//! diagnosed (and reported) with one command instead of guesswork.

use {
    crate::config,
    analysis::{
        document::Document,
        hir::Module,
        naming::{DocumentKind, DocumentNamingComputation, NamesGlobal},
        typecheck::ModuleCheck,
    },
    engine::{
        Durability, Engine,
        queries::{
            Config as EngineConfig, FileDiagnostics, FileId, Key, ParsedDocument, RoughlyQueries,
        },
    },
    ignore::Walk,
    std::{
        path::{Path, PathBuf},
        time::{Duration, Instant},
    },
};

// How many entries the slowest-files table shows.
const SLOWEST_FILES: usize = 10;

pub fn analysis_stats(target: Option<&Path>) -> Result<(), crate::cli::CommandError> {
    let target = target.unwrap_or(Path::new("."));
    let config = config::Config::discover(target).map_err(|error| {
        eprintln!("error: {error}");
        crate::cli::CommandError
    })?;
    let target = std::fs::canonicalize(target).map_err(|error| {
        eprintln!("error: failed to resolve {}: {error}", target.display());
        crate::cli::CommandError
    })?;
    let root = crate::cli::analysis_root_for_target(&target);

    let mut check_config = config.check;
    let typing_was_off = !check_config.typing;
    // A diagnosis with the type checker off measures nothing interesting; force it on and say so.
    check_config.typing = true;

    // Discover and order files exactly as the language server does: package files (under `R/`)
    // first, ascending by root-relative path, then scripts — the order the last-writer-wins
    // symbol index folds in.
    let r_path = root.join("R");
    let mut entries: Vec<(bool, String, PathBuf)> = Vec::new();
    for entry in Walk::new(&target) {
        let entry = entry.map_err(|error| {
            eprintln!("error: {error}");
            crate::cli::CommandError
        })?;
        let path = entry.into_path();
        if !path.extension().is_some_and(|ext| ext == "R" || ext == "r") {
            continue;
        }
        let is_package = path.starts_with(&r_path);
        let key = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        entries.push((is_package, key, path));
    }
    entries.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    if entries.is_empty() {
        eprintln!("error: no R files under {}", target.display());
        return Err(crate::cli::CommandError);
    }

    let project_stubs = analysis::stdlib::discover_project_stubs(&root);
    for (path, error_message) in &project_stubs.unreadable {
        eprintln!(
            "warning: failed to read override stub {}: {error_message}",
            path.display()
        );
    }
    let mut engine = Engine::new(RoughlyQueries::with_project_stubs(project_stubs.sources));
    engine.set_input_durable(
        Key::Config,
        EngineConfig {
            check: check_config,
            lint: config.lint,
        },
        Durability::HIGH,
    );

    // Phase 1: parse. The host parses (the engine's source input IS the parsed document, exactly
    // as the server feeds open buffers), so parse time is measured here per file.
    let rss_baseline = resident_set_bytes();
    let mut parser = analysis::tree::new_parser().map_err(|error| {
        eprintln!("error: failed to initialize the parser: {error}");
        crate::cli::CommandError
    })?;
    let mut files: Vec<FileRecord> = Vec::with_capacity(entries.len());
    let mut parse_total = Duration::ZERO;
    let mut package_count = 0usize;
    for (index, (is_package, _, path)) in entries.iter().enumerate() {
        let source = match std::fs::read_to_string(path) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("warning: failed to read {}: {error}", path.display());
                continue;
            }
        };
        let loc = source.lines().count();
        let start = Instant::now();
        let document = match Document::parse(&mut parser, &source) {
            Ok(document) => document,
            Err(error) => {
                eprintln!("warning: failed to parse {}: {error:?}", path.display());
                continue;
            }
        };
        let parse_time = start.elapsed();
        parse_total += parse_time;

        let file = index as FileId;
        engine.set_input_durable(
            Key::SourceText(file),
            ParsedDocument(document),
            Durability::HIGH,
        );
        engine.set_input_durable(Key::FileName(file), path.clone(), Durability::HIGH);
        engine.set_input_durable(
            Key::DocumentKind(file),
            if *is_package {
                DocumentKind::Package
            } else {
                DocumentKind::Script
            },
            Durability::HIGH,
        );
        package_count += usize::from(*is_package);
        files.push(FileRecord {
            file,
            path: path.clone(),
            loc,
            source,
            typecheck: Duration::ZERO,
        });
    }
    engine.set_input_durable(
        Key::ProjectFiles,
        files.iter().map(|record| record.file).collect::<Vec<_>>(),
        Durability::HIGH,
    );
    let rss_after_parse = resident_set_bytes();

    // Phases 2-5: staged fetches. Memoization makes the attribution honest — each stage reuses
    // everything the stages before it computed, so its wall time is its own marginal cost. The
    // package-wide folds and the per-symbol interface schemes have no line of their own: they run
    // on demand inside the first stage that needs them (naming diagnostics and type inference),
    // which is exactly where an editor pays for them.
    let total_loc: usize = files.iter().map(|record| record.loc).sum();
    let mut lower_total = Duration::ZERO;
    for record in &files {
        let start = Instant::now();
        let _ = engine.fetch::<Module>(Key::Lower(record.file));
        lower_total += start.elapsed();
    }
    let rss_after_lower = resident_set_bytes();
    let mut naming_total = Duration::ZERO;
    for record in &files {
        let start = Instant::now();
        let _ = engine.fetch::<DocumentNamingComputation>(Key::LocalNaming(record.file));
        naming_total += start.elapsed();
    }
    let rss_after_naming = resident_set_bytes();
    let mut typecheck_total = Duration::ZERO;
    for record in &mut files {
        let start = Instant::now();
        let _ = engine.fetch::<ModuleCheck>(Key::Typecheck(record.file));
        record.typecheck = start.elapsed();
        typecheck_total += record.typecheck;
    }
    let rss_after_typecheck = resident_set_bytes();
    let mut diagnostics_total = Duration::ZERO;
    let mut diagnostic_count = 0usize;
    for record in &files {
        let start = Instant::now();
        let rendered = engine.fetch::<FileDiagnostics>(Key::Diagnostics(record.file));
        diagnostics_total += start.elapsed();
        diagnostic_count += rendered.naming.len()
            + rendered.package_naming.len()
            + rendered.type_errors.len()
            + rendered.lowering.len()
            + rendered.lint.len();
    }
    let rss_after_diagnostics = resident_set_bytes();
    let cold_total = parse_total + lower_total + naming_total + typecheck_total + diagnostics_total;

    let symbol_count = engine
        .fetch::<NamesGlobal>(Key::PackageSymbolIndex)
        .global_bindings
        .len();

    println!("workspace: {}", root.display());
    println!(
        "  {} package files + {} scripts, {} LoC, {} exported globals",
        package_count,
        files.len() - package_count,
        total_loc,
        symbol_count,
    );
    if typing_was_off {
        println!("  note: [check] typing is off in the configuration; stats force it on");
    }
    println!();
    println!("cold analysis (one full pass, phase marginal cost, resident-set growth):");
    let phase = |name: &str, duration: Duration, memory: Option<(u64, u64)>| {
        let memory = match memory {
            Some((before, after)) => {
                format!("  {:>+9.1} MiB", (after as f64 - before as f64) / MEBIBYTE)
            }
            None => String::new(),
        };
        println!(
            "  {name:<22} {:>10.1} ms  ({:>4.1}%){memory}",
            duration.as_secs_f64() * 1e3,
            duration.as_secs_f64() / cold_total.as_secs_f64().max(f64::EPSILON) * 100.0,
        );
    };
    let delta = |before: Option<u64>, after: Option<u64>| Some((before?, after?));
    phase("parse", parse_total, delta(rss_baseline, rss_after_parse));
    phase(
        "lower",
        lower_total,
        delta(rss_after_parse, rss_after_lower),
    );
    phase(
        "local naming",
        naming_total,
        delta(rss_after_lower, rss_after_naming),
    );
    phase(
        "typecheck (+interfaces)",
        typecheck_total,
        delta(rss_after_naming, rss_after_typecheck),
    );
    phase(
        "diagnostics (+folds)",
        diagnostics_total,
        delta(rss_after_typecheck, rss_after_diagnostics),
    );
    let total_memory = match delta(rss_baseline, rss_after_diagnostics) {
        Some((before, after)) => {
            format!("  {:>+9.1} MiB", (after as f64 - before as f64) / MEBIBYTE)
        }
        None => String::new(),
    };
    println!(
        "  {:<22} {:>10.1} ms        {total_memory}   ({} diagnostics, {} engine memos)",
        "total",
        cold_total.as_secs_f64() * 1e3,
        diagnostic_count,
        engine.slot_count(),
    );
    if let Some(peak) = peak_resident_set_bytes() {
        println!(
            "  peak resident set      {:>10.1} MiB",
            peak as f64 / MEBIBYTE
        );
    }

    println!();
    println!("slowest files (typecheck):");
    let mut by_typecheck: Vec<&FileRecord> = files.iter().collect();
    by_typecheck.sort_by_key(|record| std::cmp::Reverse(record.typecheck));
    for record in by_typecheck.iter().take(SLOWEST_FILES) {
        println!(
            "  {:>10.1} ms  {:>6} LoC  {}",
            record.typecheck.as_secs_f64() * 1e3,
            record.loc,
            record
                .path
                .strip_prefix(&root)
                .unwrap_or(&record.path)
                .display(),
        );
    }

    // The incremental probe: a burst of body-only keystrokes (a top-level `invisible(1…)` literal
    // growing one digit per edit — well-formed every step, no export changes) on the slowest file,
    // fed at open-document durability exactly as keystrokes would be. The edited-file recheck is
    // what a user waits on while typing; the workspace revalidate is the cost of confirming every
    // other file's diagnostics are unaffected. The recompute table attributes the keystroke cost:
    // it counts which query bodies each keystroke re-ran (and how often), which wall-clock alone
    // cannot show — e.g. the same file re-inferred several times through the interface layer.
    const TYPING_BURST: usize = 10;
    if let Some(slowest) = by_typecheck.first().map(|record| record.file)
        && let Some(record) = files.iter().find(|record| record.file == slowest)
    {
        let totals_before = engine.group().execution_totals();
        let validations_before = engine.validation_count();
        let mut keystroke_times = Vec::with_capacity(TYPING_BURST);
        let mut probe_failed = false;
        for keystroke in 1..=TYPING_BURST {
            let edited_source =
                format!("{}\ninvisible({})\n", record.source, "1".repeat(keystroke));
            match Document::parse(&mut parser, &edited_source) {
                Ok(document) => {
                    engine.set_input_durable(
                        Key::SourceText(record.file),
                        ParsedDocument(document),
                        Durability::LOW,
                    );
                    let start = Instant::now();
                    let _ = engine.fetch::<FileDiagnostics>(Key::Diagnostics(record.file));
                    keystroke_times.push(start.elapsed());
                }
                Err(error) => {
                    eprintln!("warning: incremental probe skipped (parse failed: {error:?})");
                    probe_failed = true;
                    break;
                }
            }
        }
        if !probe_failed {
            let totals_after_burst = engine.group().execution_totals();
            let validations_after_burst = engine.validation_count();
            let start = Instant::now();
            for other in &files {
                let _ = engine.fetch::<FileDiagnostics>(Key::Diagnostics(other.file));
            }
            let sweep = start.elapsed();
            let totals_after_sweep = engine.group().execution_totals();
            let validations_after_sweep = engine.validation_count();

            keystroke_times.sort();
            let median = keystroke_times[keystroke_times.len() / 2];
            let worst = *keystroke_times.last().expect("burst is non-empty");
            println!();
            println!(
                "incremental (typing burst: {TYPING_BURST} body edits on {}, open-document durability):",
                record
                    .path
                    .strip_prefix(&root)
                    .unwrap_or(&record.path)
                    .display(),
            );
            println!(
                "  edited-file diagnostics   {:>10.1} ms median, {:.1} ms max",
                median.as_secs_f64() * 1e3,
                worst.as_secs_f64() * 1e3,
            );
            println!(
                "  workspace revalidate      {:>10.1} ms  (after the last keystroke)",
                sweep.as_secs_f64() * 1e3
            );
            println!();
            println!("  recomputed query bodies:      per keystroke   revalidate sweep");
            for ((name, before), ((_, after_burst), (_, after_sweep))) in totals_before
                .iter()
                .zip(totals_after_burst.iter().zip(totals_after_sweep.iter()))
            {
                let burst_delta = after_burst - before;
                let sweep_delta = after_sweep - after_burst;
                if burst_delta == 0 && sweep_delta == 0 {
                    continue;
                }
                println!(
                    "    {name:<28} {:>11.1}   {:>14}",
                    burst_delta as f64 / TYPING_BURST as f64,
                    sweep_delta,
                );
            }
            println!(
                "    {:<28} {:>11.1}   {:>14}",
                "validation walk (slots)",
                (validations_after_burst - validations_before) as f64 / TYPING_BURST as f64,
                validations_after_sweep - validations_after_burst,
            );
        }
    }

    Ok(())
}

struct FileRecord {
    file: FileId,
    path: PathBuf,
    loc: usize,
    source: String,
    typecheck: Duration,
}

const MEBIBYTE: f64 = 1024.0 * 1024.0;

// The current resident set, so each phase's growth attributes the retained memory (memoized
// values dominate; the resident set also keeps allocator-held freed pages, so a phase with heavy
// transient allocation reads slightly high). `None` where the kernel does not expose it — the
// memory column is then simply omitted.
fn resident_set_bytes() -> Option<u64> {
    proc_status_field("VmRSS:")
}

fn peak_resident_set_bytes() -> Option<u64> {
    proc_status_field("VmHWM:")
}

fn proc_status_field(field: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with(field))?;
    let kibibytes: u64 = line
        .strip_prefix(field)?
        .trim()
        .strip_suffix("kB")?
        .trim()
        .parse()
        .ok()?;
    Some(kibibytes * 1024)
}
