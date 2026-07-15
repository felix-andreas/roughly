//! `roughly analysis-stats`: a workspace performance diagnosis. Runs the full analysis pipeline
//! over a workspace through the same query engine the language server uses and prints where the
//! time goes — per-phase totals, the slowest files, and an incremental edit probe — plus where the
//! memory goes (per-phase resident-set growth), so a slow or memory-hungry workspace can be
//! diagnosed (and reported) with one command instead of guesswork.

use {
    crate::config,
    analysis::{
        diagnostic::Diagnostic,
        document::Document,
        hir::Module,
        naming::{DocumentKind, DocumentNamingComputation, NamesGlobal},
    },
    engine::{
        Durability, Engine,
        queries::{
            Config as EngineConfig, FileDiagnostics, FileId, FileInference, Key, RoughlyQueries,
            SourceText, source_input,
        },
    },
    ignore::Walk,
    std::{
        collections::BTreeMap,
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

    // Phase 1: load. The corpus is fed rope-only, exactly as the server loads a workspace from
    // disk; parsing happens on demand inside the first tree-reading query of each file (`lower`),
    // so this phase is file reading and rope construction.
    let rss_baseline = resident_set_bytes();
    let mut parser = analysis::tree::new_parser().map_err(|error| {
        eprintln!("error: failed to initialize the parser: {error}");
        crate::cli::CommandError
    })?;
    let mut files: Vec<FileRecord> = Vec::with_capacity(entries.len());
    let mut load_total = Duration::ZERO;
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
        let input = source_input(&source);
        let load_time = start.elapsed();
        load_total += load_time;

        let file = index as FileId;
        engine.set_input_durable(Key::SourceText(file), input, Durability::HIGH);
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
    let rss_after_load = resident_set_bytes();

    // Phases 2-5: staged fetches. Memoization makes the attribution honest — each stage reuses
    // everything the stages before it computed, so its wall time is its own marginal cost. The
    // package-wide folds and the per-symbol interface schemes have no line of their own: they run
    // on demand inside the first stage that needs them (naming diagnostics and type inference),
    // which is exactly where an editor pays for them.
    let total_loc: usize = files.iter().map(|record| record.loc).sum();
    let mut lower_total = Duration::ZERO;
    let mut lint_total = Duration::ZERO;
    for record in &files {
        let start = Instant::now();
        let _ = engine.fetch::<Module>(Key::Lower(record.file));
        lower_total += start.elapsed();
        // Lint runs right after this file's lower, while its parse is still in the bounded cache —
        // the same adjacency the server's per-file prime has — so the lint line shows lint's own
        // cost rather than a staging re-parse of the whole corpus.
        let start = Instant::now();
        let _ = engine.fetch::<Vec<Diagnostic>>(Key::Lint(record.file));
        lint_total += start.elapsed();
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
        let _ = engine.fetch::<FileInference>(Key::Typecheck(record.file));
        record.typecheck = start.elapsed();
        typecheck_total += record.typecheck;
    }
    let rss_after_typecheck = resident_set_bytes();
    let mut package_naming_total = Duration::ZERO;
    for record in &files {
        let start = Instant::now();
        let _ = engine.fetch::<Vec<Diagnostic>>(Key::PackageNamingDiagnostics(record.file));
        package_naming_total += start.elapsed();
    }
    let rss_after_package_naming = resident_set_bytes();
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
    let cold_total = load_total
        + lower_total
        + naming_total
        + typecheck_total
        + lint_total
        + package_naming_total
        + diagnostics_total;

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
    phase(
        "load (read + ropes)",
        load_total,
        delta(rss_baseline, rss_after_load),
    );
    // `lower` includes each file's one on-demand parse; lint runs adjacently against the same
    // cached parse, so its line below is lint's own cost (the memory delta covers both).
    phase(
        "lower (+parse)",
        lower_total,
        delta(rss_after_load, rss_after_lower),
    );
    phase("lint", lint_total, None);
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
        "package naming (+folds)",
        package_naming_total,
        delta(rss_after_typecheck, rss_after_package_naming),
    );
    phase(
        "diagnostics (render)",
        diagnostics_total,
        delta(rss_after_package_naming, rss_after_diagnostics),
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
        // Declare the probe file open (as an editor's did_open would): list it in `OpenFiles` and
        // downgrade its source to open-document durability (same text). Then settle the one-time
        // open-event revalidation walk, so the burst measures steady-state keystrokes: the durable
        // halves of the all-files folds exclude the open file and green out in O(1) per keystroke.
        engine.set_input_durable(Key::OpenFiles, vec![record.file], Durability::HIGH);
        match Document::parse(&mut parser, &record.source) {
            Ok(document) => engine.set_input_durable(
                Key::SourceText(record.file),
                SourceText::from_document(&document),
                Durability::LOW,
            ),
            Err(error) => {
                eprintln!("warning: incremental probe open failed to parse: {error:?}");
            }
        }
        let _ = engine.fetch::<FileDiagnostics>(Key::Diagnostics(record.file));
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
                        SourceText::from_document(&document),
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
            let mut validated_kinds: BTreeMap<&'static str, u64> = BTreeMap::new();
            for key in engine.slots_verified_this_revision() {
                *validated_kinds.entry(key_kind(&key)).or_default() += 1;
            }
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
            println!();
            println!("  slots validated by the last keystroke (unique, by query):");
            let mut kinds: Vec<(&'static str, u64)> = validated_kinds.into_iter().collect();
            kinds.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
            for (kind, count) in kinds {
                println!("    {kind:<28} {count:>11}");
            }
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

// The key's query family, for the per-keystroke walk attribution table.
fn key_kind(key: &Key) -> &'static str {
    match key {
        Key::SourceText(_) => "source text",
        Key::DocumentKind(_) => "document kind",
        Key::ProjectFiles => "project files",
        Key::Config => "config",
        Key::FileName(_) => "file name",
        Key::OpenFiles => "open files",
        Key::Lower(_) => "lower",
        Key::LocalNaming(_) => "local naming",
        Key::ExportedNames(_) => "exported names",
        Key::TopLevelSite(..) => "top-level site",
        Key::PackageSymbolIndex => "package symbol index",
        Key::DurableSymbolIndex => "durable symbol index",
        Key::CompletionExports(_) => "completion exports",
        Key::PackageCompletionIndex => "package completion index",
        Key::DurableCompletionIndex => "durable completion index",
        Key::DeclaredGlobals(_) => "declared globals",
        Key::PackageDeclaredGlobals => "package declared globals",
        Key::DurableDeclaredGlobals => "durable declared globals",
        Key::TypeDefinitionsModule(_) => "type definitions module",
        Key::PackageTypeDefinitions => "package type definitions",
        Key::DurableTypeDefinitionModules => "durable type definition modules",
        Key::FallbackRange => "fallback range",
        Key::DefiningItem(_) => "defining item",
        Key::InterfaceDeps(_) => "interface deps",
        Key::SymbolScc(_) => "symbol scc",
        Key::GlobalScheme(_) => "global scheme",
        Key::ExportedSchemes(_) => "exported schemes",
        Key::InterfaceScc(_) => "interface scc",
        Key::FileTypeDefinitions(_) => "file type definitions",
        Key::PackageTypeIndex => "package type index",
        Key::DurableTypeIndexSites => "durable type index sites",
        Key::TypeNameStatus(_) => "type name status",
        Key::PackageCandidateOrder => "package candidate order",
        Key::DurableCandidateOrder => "durable candidate order",
        Key::DefinerOrder(_) => "definer order",
        Key::TypeCandidateOrder => "type candidate order",
        Key::DurableTypeCandidateOrder => "durable type candidate order",
        Key::TypeDefinerOrder(_) => "type definer order",
        Key::TypeDefinitionSites(..) => "type definition sites",
        Key::PackageNamingDiagnostics(_) => "package naming diagnostics",
        Key::LoweringDiagnostics(_) => "lowering diagnostics",
        Key::Lint(_) => "lint",
        Key::Typecheck(_) => "typecheck",
        Key::Diagnostics(_) => "diagnostics",
    }
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
