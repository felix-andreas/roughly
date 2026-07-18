//! Parallel-read stress tests: salsa storage-handle clones let several
//! threads force the same queries concurrently. The dangerous shape is the
//! package-interface fixpoint — concurrent threads entering the same
//! `global_scheme` cycle from different heads — which historically hung
//! parallel salsa fixpoints upstream. These tests gate any multi-core use of
//! the database: no deadlock (bounded wall time), no panic, and answers
//! byte-equal to the single-threaded ones.

use semantics::diagnostics::{file_diagnostics, strict_diagnostics};
use semantics::{DocumentKind, ProjectFiles, RootDatabase, SourceFile};

/// A project whose files form mutually recursive cross-file definition
/// cycles (each file's functions call the neighbours'), so every thread's
/// query enters the interface fixpoint.
fn cyclic_project(db: &RootDatabase, files: usize, functions_per_file: usize) -> Vec<SourceFile> {
    let mut sources = Vec::new();
    for file_index in 0..files {
        let mut source = String::new();
        for function_index in 0..functions_per_file {
            let next_file = (file_index + 1) % files;
            source.push_str(&format!(
                "f_{file_index}_{function_index} <- function(x) f_{next_file}_{function_index}(x) + g_{file_index}(x)\n"
            ));
        }
        // A self-referential growing binding rides the fixpoint round cap.
        source.push_str(&format!(
            "g_{file_index} <- function(x) if (x > 0) g_{}(x - 1) else x\n",
            (file_index + files - 1) % files
        ));
        sources.push(source);
    }
    let handles: Vec<SourceFile> = sources
        .into_iter()
        .map(|source| SourceFile::new(db, source, DocumentKind::Package))
        .collect();
    ProjectFiles::new(db, handles.clone());
    handles
}

fn all_diagnostics(db: &RootDatabase, files: &[SourceFile]) -> Vec<String> {
    let mut rendered = Vec::new();
    for &file in files {
        for diagnostic in file_diagnostics(db, file) {
            rendered.push(format!(
                "{}..{} {}",
                u32::from(diagnostic.range.start()),
                u32::from(diagnostic.range.end()),
                diagnostic.message
            ));
        }
        for diagnostic in strict_diagnostics(db, file) {
            rendered.push(format!(
                "strict {}..{} {}",
                u32::from(diagnostic.range.start()),
                u32::from(diagnostic.range.end()),
                diagnostic.message
            ));
        }
    }
    rendered
}

/// Eight threads race the same cyclic interface fixpoint; every thread's
/// answer must equal the single-threaded baseline, within a hard wall-time
/// bound (a hang here is a real parallel-fixpoint bug, never load).
#[test]
fn concurrent_cycle_queries_agree_with_the_baseline() {
    let db = RootDatabase::default();
    semantics::stubs::install_shipped_stubs(&db);
    let files = cyclic_project(&db, 6, 4);

    let baseline_db = RootDatabase::default();
    semantics::stubs::install_shipped_stubs(&baseline_db);
    let baseline_files = cyclic_project(&baseline_db, 6, 4);
    let baseline = all_diagnostics(&baseline_db, &baseline_files);

    let started = std::time::Instant::now();
    std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for worker in 0..8 {
            let db = db.clone();
            let files = files.clone();
            workers.push(scope.spawn(move || {
                // Stagger entry points so threads enter the cycle from
                // different heads.
                let mut ordered = files.clone();
                ordered.rotate_left(worker % files.len());
                all_diagnostics(&db, &ordered)
            }));
        }
        for (worker, handle) in workers.into_iter().enumerate() {
            let rendered = handle.join().expect("worker thread must not panic");
            let mut ordered = files.clone();
            ordered.rotate_left(worker % files.len());
            let expected = {
                let mut expected = Vec::new();
                let by_file: std::collections::HashMap<SourceFile, Vec<String>> = files
                    .iter()
                    .map(|&file| (file, all_diagnostics(&db, std::slice::from_ref(&file))))
                    .collect();
                for file in &ordered {
                    expected.extend(by_file[file].iter().cloned());
                }
                expected
            };
            assert_eq!(rendered, expected, "worker {worker} diverged");
        }
    });
    assert!(
        started.elapsed() < std::time::Duration::from_secs(60),
        "parallel fixpoint took implausibly long — treat as a hang"
    );

    let mut single = all_diagnostics(&db, &files);
    let mut baseline_sorted = baseline;
    single.sort();
    baseline_sorted.sort();
    assert_eq!(
        single, baseline_sorted,
        "parallel execution corrupted the memoized answers"
    );
}

/// Concurrent cold priming of disjoint files must scale without corrupting
/// state: the parallel answers equal a fresh sequential database's.
#[test]
fn parallel_cold_prime_matches_sequential() {
    let mut sources = Vec::new();
    for index in 0..24 {
        sources.push(format!(
            "value_{index} <- {index}L\nuse_{index} <- function() value_{index} + shared_helper()\n"
        ));
    }
    sources.push("shared_helper <- function() 1L\n".to_owned());

    let db = RootDatabase::default();
    semantics::stubs::install_shipped_stubs(&db);
    let files: Vec<SourceFile> = sources
        .iter()
        .map(|source| SourceFile::new(&db, source.clone(), DocumentKind::Package))
        .collect();
    ProjectFiles::new(&db, files.clone());

    std::thread::scope(|scope| {
        for chunk in files.chunks(4) {
            let db = db.clone();
            let chunk = chunk.to_vec();
            scope.spawn(move || {
                for &file in &chunk {
                    let _ = file_diagnostics(&db, file);
                }
            });
        }
    });

    let sequential_db = RootDatabase::default();
    semantics::stubs::install_shipped_stubs(&sequential_db);
    let sequential_files: Vec<SourceFile> = sources
        .iter()
        .map(|source| SourceFile::new(&sequential_db, source.clone(), DocumentKind::Package))
        .collect();
    ProjectFiles::new(&sequential_db, sequential_files.clone());

    for (&file, &sequential_file) in files.iter().zip(&sequential_files) {
        assert_eq!(
            all_diagnostics(&db, std::slice::from_ref(&file)),
            all_diagnostics(&sequential_db, std::slice::from_ref(&sequential_file)),
        );
    }
}
