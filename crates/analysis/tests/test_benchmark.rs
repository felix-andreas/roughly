mod common;

use {
    analysis::{Analysis, CheckConfig, LintConfig},
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
