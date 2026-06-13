use {
    analysis::{Analysis, CheckConfig, DocumentChange, LintConfig, TextPosition, TextRange},
    std::path::PathBuf,
};

fn typing_analysis(base: &str) -> Analysis {
    Analysis::new(
        PathBuf::from(base),
        LintConfig::default(),
        CheckConfig {
            unused: false,
            typing: true,
        },
    )
}

fn error_messages(analysis_state: &Analysis, path: &str) -> Vec<String> {
    let document_id = analysis_state
        .document_id_for_path(std::path::Path::new(path))
        .expect("document should exist");
    analysis_state
        .document_diagnostics(document_id)
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect()
}

fn replace_document(analysis_state: &mut Analysis, path: &str, source: &str) {
    analysis_state
        .add_document_from_source(PathBuf::from(path), source)
        .expect("document should parse");
}

#[test]
fn absolute_base_path_reports_type_errors() {
    let mut analysis_state = typing_analysis("/pkg");
    replace_document(
        &mut analysis_state,
        "/pkg/R/main.R",
        "#: fn(count: integer) -> integer\ndouble_count <- function(count) count + count\nbad <- double_count(\"two\")\n",
    );
    analysis::run_full(&mut analysis_state);

    let messages = error_messages(&analysis_state, "/pkg/R/main.R");
    assert_eq!(messages, vec!["expected `integer`, found `character`"]);
}

#[test]
fn every_error_in_a_document_is_reported() {
    let mut analysis_state = typing_analysis("/pkg");
    replace_document(
        &mut analysis_state,
        "/pkg/R/main.R",
        "#: integer\nfirst <- \"one\"\n#: integer\nsecond <- \"two\"\n",
    );
    analysis::run_full(&mut analysis_state);

    let messages = error_messages(&analysis_state, "/pkg/R/main.R");
    assert_eq!(messages.len(), 2, "{messages:?}");
}

#[test]
fn errors_in_several_documents_are_reported_in_one_run() {
    let mut analysis_state = typing_analysis("/pkg");
    replace_document(&mut analysis_state, "/pkg/R/a.R", "#: integer\na <- \"one\"\n");
    replace_document(&mut analysis_state, "/pkg/R/b.R", "#: integer\nb <- \"two\"\n");
    analysis::run_full(&mut analysis_state);

    assert_eq!(error_messages(&analysis_state, "/pkg/R/a.R").len(), 1);
    assert_eq!(error_messages(&analysis_state, "/pkg/R/b.R").len(), 1);
}

#[test]
fn scripts_are_typechecked_against_package_globals() {
    let mut analysis_state = typing_analysis("/pkg");
    replace_document(
        &mut analysis_state,
        "/pkg/R/a.R",
        "#: fn(count: integer) -> integer\ndouble_count <- function(count) count + count\n",
    );
    replace_document(
        &mut analysis_state,
        "/pkg/scripts/use.R",
        "good <- double_count(2L)\nbad <- double_count(\"two\")\n",
    );
    analysis::run_full(&mut analysis_state);

    assert_eq!(error_messages(&analysis_state, "/pkg/R/a.R").len(), 0);
    assert_eq!(
        error_messages(&analysis_state, "/pkg/scripts/use.R"),
        vec!["expected `integer`, found `character`"]
    );
}

#[test]
fn body_edit_without_interface_change_rechecks_only_edited_document() {
    let mut analysis_state = typing_analysis("/pkg");
    replace_document(
        &mut analysis_state,
        "/pkg/R/a.R",
        "#: fn(count: integer) -> integer\ndouble_count <- function(count) count + count\n",
    );
    replace_document(
        &mut analysis_state,
        "/pkg/R/b.R",
        "result <- double_count(2L)\n",
    );
    let first_run = analysis::typecheck(&mut analysis_state);
    assert_eq!(first_run.len(), 2, "initial run checks both documents");

    // Body change that keeps the exported interface identical.
    replace_document(
        &mut analysis_state,
        "/pkg/R/a.R",
        "#: fn(count: integer) -> integer\ndouble_count <- function(count) { count + count }\n",
    );
    let second_run = analysis::typecheck(&mut analysis_state);
    let a_id = analysis_state
        .document_id_for_path(std::path::Path::new("/pkg/R/a.R"))
        .expect("document should exist");
    assert_eq!(second_run, vec![a_id], "only the edited document rechecks");
}

#[test]
fn interface_change_rechecks_dependents_and_reports_dependent_errors() {
    let mut analysis_state = typing_analysis("/pkg");
    replace_document(
        &mut analysis_state,
        "/pkg/R/a.R",
        "#: fn(count: integer) -> integer\ndouble_count <- function(count) count + count\n",
    );
    replace_document(
        &mut analysis_state,
        "/pkg/R/b.R",
        "result <- double_count(2L)\n",
    );
    analysis::run_full(&mut analysis_state);
    assert_eq!(error_messages(&analysis_state, "/pkg/R/b.R").len(), 0);

    // The exported interface changes; the dependent document must be rechecked and now fail.
    replace_document(
        &mut analysis_state,
        "/pkg/R/a.R",
        "#: fn(count: character) -> integer\ndouble_count <- function(count) 1L\n",
    );
    let second_run = analysis::typecheck(&mut analysis_state);
    let b_id = analysis_state
        .document_id_for_path(std::path::Path::new("/pkg/R/b.R"))
        .expect("document should exist");
    assert!(
        second_run.contains(&b_id),
        "dependent document should recheck after an interface change"
    );
    assert_eq!(
        error_messages(&analysis_state, "/pkg/R/b.R"),
        vec!["expected `character`, found `integer`"]
    );
}

#[test]
fn unedited_documents_keep_diagnostics_after_unrelated_edit() {
    let mut analysis_state = typing_analysis("/pkg");
    replace_document(&mut analysis_state, "/pkg/R/a.R", "#: integer\na <- \"bad\"\n");
    replace_document(&mut analysis_state, "/pkg/R/b.R", "b <- 1L\n");
    analysis::run_full(&mut analysis_state);
    assert_eq!(error_messages(&analysis_state, "/pkg/R/a.R").len(), 1);

    analysis_state
        .edit_document(
            std::path::Path::new("/pkg/R/b.R"),
            &[DocumentChange {
                range: TextRange {
                    start: TextPosition {
                        line_index: 0,
                        character_index: 5,
                    },
                    end: TextPosition {
                        line_index: 0,
                        character_index: 7,
                    },
                },
                text: "2L".to_owned(),
            }],
        )
        .expect("edit should apply");
    analysis::run_full(&mut analysis_state);
    assert_eq!(
        error_messages(&analysis_state, "/pkg/R/a.R").len(),
        1,
        "untouched document keeps its diagnostics"
    );
}

// Run manually with: cargo test -p analysis --release --test test_incremental -- --ignored --nocapture
#[test]
#[ignore = "timing benchmark, run manually"]
fn benchmark_single_file_recheck_in_large_package() {
    let mut analysis_state = typing_analysis("/pkg");
    for file_index in 0..500 {
        let mut source = String::new();
        for item_index in 0..30 {
            source.push_str("#: fn(count: integer) -> integer\n");
            source.push_str(&format!(
                "fn_{file_index}_{item_index} <- function(count) count + {item_index}L\n"
            ));
            if file_index > 0 {
                source.push_str(&format!(
                    "value_{file_index}_{item_index} <- fn_{}_{item_index}(2L)\n",
                    file_index - 1
                ));
            }
        }
        replace_document(
            &mut analysis_state,
            &format!("/pkg/R/file_{file_index:04}.R"),
            &source,
        );
    }

    let full_start = std::time::Instant::now();
    analysis::run_full(&mut analysis_state);
    let full_elapsed = full_start.elapsed();

    // Body-only edit in one file.
    replace_document(
        &mut analysis_state,
        "/pkg/R/file_0250.R",
        &{
            let mut source = String::new();
            for item_index in 0..30 {
                source.push_str("#: fn(count: integer) -> integer\n");
                source.push_str(&format!(
                    "fn_250_{item_index} <- function(count) {{ count + {item_index}L }}\n"
                ));
                source.push_str(&format!("value_250_{item_index} <- fn_249_{item_index}(2L)\n"));
            }
            source
        },
    );
    let incremental_start = std::time::Instant::now();
    let recomputed = analysis::typecheck(&mut analysis_state);
    let incremental_elapsed = incremental_start.elapsed();

    println!("full check: {full_elapsed:?}");
    println!(
        "single-file recheck: {incremental_elapsed:?} ({} documents recomputed)",
        recomputed.len()
    );
    assert_eq!(recomputed.len(), 1, "body edit should recheck one document");
}
