mod common;

use {
    analysis::{Analysis, CheckConfig, LintConfig, ide, text::TextPosition},
    std::{path::PathBuf, time::Instant},
};

const ITEMS_PER_FILE: usize = 20;

#[test]
#[ignore = "timing benchmark, run manually"]
fn benchmark_10k() {
    run_benchmark(10_000);
}

#[test]
#[ignore = "timing benchmark, run manually"]
fn benchmark_100k() {
    run_benchmark(100_000);
}

#[test]
#[ignore = "timing benchmark, run manually"]
fn benchmark_200k() {
    run_benchmark(200_000);
}

#[test]
#[ignore = "timing benchmark, run manually"]
fn benchmark_ide_100k() {
    run_ide_benchmark(100_000);
}

#[test]
#[ignore = "timing benchmark, run manually"]
fn benchmark_ide_200k() {
    run_ide_benchmark(200_000);
}

// Times the IDE requests whose cost scales with project size, to catch regressions in interactive
// latency on large packages. The synthetic package cross-references `fn_{file}_{item}` from the next
// file, so a request placed on such a reference exercises a real cross-file global symbol.
fn run_ide_benchmark(target_loc: usize) {
    let lines_per_file = common::lines_per_file(ITEMS_PER_FILE);
    let file_count = (target_loc + lines_per_file / 2) / lines_per_file;
    let files = common::generate_package(file_count, ITEMS_PER_FILE);
    let realized_loc = common::total_lines(&files);

    let mut analysis_state = Analysis::new(
        PathBuf::from("/pkg"),
        LintConfig::default(),
        CheckConfig {
            unused: false,
            typing: true,
        },
    );
    for (path, source) in &files {
        analysis_state
            .add_document_from_source(path.clone(), source)
            .expect("document should parse");
    }
    analysis::run_full(&mut analysis_state);

    // The reference `fn_{file - 1}_0` lives on the `use_{file}_0 <- fn_{file - 1}_0(2L)` line, which
    // is the fifth line (index 4) of item 0 in that file. Its column is the width of the assignment
    // prefix that precedes it.
    let probe_file = file_count / 2;
    let probe_path = common::file_path(probe_file);
    let reference_line = 4;
    let reference_column = format!("use_{probe_file}_0 <- ").len();
    let reference_position = TextPosition {
        line_index: reference_line,
        character_index: reference_column,
    };

    macro_rules! time {
        ($label:literal, $count:expr) => {{
            let start = Instant::now();
            let result_count: usize = $count;
            println!("  {:<28} {:?} ({result_count} result(s))", $label, start.elapsed());
        }};
    }

    println!("IDE benchmark target {target_loc} LoC");
    println!("  realized LoC:         {realized_loc}");
    println!("  files:                {file_count}");
    time!(
        "goto-definition (global):",
        ide::definition(&mut analysis_state, &probe_path, reference_position)
            .map(|locations| locations.len())
            .unwrap_or(0)
    );
    time!(
        "find-references (global):",
        ide::references(&mut analysis_state, &probe_path, reference_position, true)
            .map(|locations| locations.len())
            .unwrap_or(0)
    );
    time!(
        "hover (global):",
        ide::hover(&mut analysis_state, &probe_path, reference_position).is_some() as usize
    );
    time!(
        "completion:",
        ide::completion(
            &mut analysis_state,
            &probe_path,
            TextPosition {
                line_index: reference_line,
                character_index: reference_column + 3,
            },
        )
        .map(|items| items.len())
        .unwrap_or(0)
    );
}

fn run_benchmark(target_loc: usize) {
    let lines_per_file = common::lines_per_file(ITEMS_PER_FILE);
    let file_count = (target_loc + lines_per_file / 2) / lines_per_file;
    let files = common::generate_package(file_count, ITEMS_PER_FILE);
    let realized_loc = common::total_lines(&files);
    assert!(
        realized_loc >= target_loc * 9 / 10 && realized_loc <= target_loc * 11 / 10,
        "realized LoC {realized_loc} is not within 10% of target {target_loc}"
    );

    let mut analysis_state = Analysis::new(
        PathBuf::from("/pkg"),
        LintConfig::default(),
        CheckConfig {
            unused: false,
            typing: true,
        },
    );
    for (path, source) in &files {
        analysis_state
            .add_document_from_source(path.clone(), source)
            .expect("document should parse");
    }

    let cold_start = Instant::now();
    analysis::run_full(&mut analysis_state);
    let cold_elapsed = cold_start.elapsed();

    let edited_file = file_count / 2;
    analysis_state
        .add_document_from_source(
            common::file_path(edited_file),
            &common::generate_file(edited_file, ITEMS_PER_FILE, true),
        )
        .expect("document should parse");
    let recheck_start = Instant::now();
    let recomputed = analysis::typecheck(&mut analysis_state);
    let recheck_elapsed = recheck_start.elapsed();

    println!("benchmark target {target_loc} LoC");
    println!("  realized LoC:         {realized_loc}");
    println!("  files:                {file_count}");
    println!("  cold full check:      {cold_elapsed:?}");
    println!("  single-file recheck:  {recheck_elapsed:?}");
    println!("  documents recomputed: {}", recomputed.len());
    assert_eq!(
        recomputed.len(),
        1,
        "body-only edit should recheck one document"
    );
}
