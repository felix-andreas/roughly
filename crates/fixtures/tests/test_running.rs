use {
    fixtures::{FixtureOutput, FixtureRunFile, run_fixture_suite},
    indoc::indoc,
    std::{
        fs,
        panic::{AssertUnwindSafe, catch_unwind},
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    },
};

#[test]
fn runs_simple_fixtures_with_structured_output() {
    let fixture_directory = write_fixture_suite(indoc! {"
        #==== basics
        #---- scalar
        value <- 1
        #++++
        value: integer
    "});

    run_fixture_suite(
        path_to_string(&fixture_directory).as_str(),
        "running",
        |fixture| {
            Ok(vec![FixtureOutput {
                name: fixture.name.clone(),
                files: vec![FixtureRunFile {
                    path: PathBuf::new(),
                    output: "value: integer".to_owned(),
                }],
            }])
        },
    );
}

#[test]
fn compares_multi_file_outputs_by_path() {
    let fixture_directory = write_fixture_suite(indoc! {"
        #==== workspace
        #---- multi_file
        #---- a.R
        alpha <- 1
        #++++
        a snapshot
        #---- b.R
        beta <- alpha
        #++++
        b snapshot
    "});

    run_fixture_suite(
        path_to_string(&fixture_directory).as_str(),
        "running",
        |fixture| {
            Ok(vec![FixtureOutput {
                name: fixture.name.clone(),
                files: vec![
                    FixtureRunFile {
                        path: PathBuf::from("b.R"),
                        output: "b snapshot".to_owned(),
                    },
                    FixtureRunFile {
                        path: PathBuf::from("a.R"),
                        output: "a snapshot".to_owned(),
                    },
                ],
            }])
        },
    );
}

#[test]
fn carries_expectations_forward_across_generations() {
    let fixture_directory = write_fixture_suite(indoc! {"
        #==== workspace
        #---- generational
        #.... v1
        #---- a.R
        alpha <- 1
        #++++
        a v1
        #---- b.R
        beta <- alpha
        #++++ any
        #.... v2
        #---- edit a.R 1:11-1:11 -> \" + 2\"
        #++++
        a v2
        #---- move b.R -> c.R
        #---- delete old.R
    "});

    run_fixture_suite(
        path_to_string(&fixture_directory).as_str(),
        "running",
        |_fixture| {
            Ok(vec![
                FixtureOutput {
                    name: "v1".to_owned(),
                    files: vec![
                        FixtureRunFile {
                            path: PathBuf::from("a.R"),
                            output: "a v1".to_owned(),
                        },
                        FixtureRunFile {
                            path: PathBuf::from("b.R"),
                            output: "anything".to_owned(),
                        },
                    ],
                },
                FixtureOutput {
                    name: "v2".to_owned(),
                    files: vec![
                        FixtureRunFile {
                            path: PathBuf::from("a.R"),
                            output: "a v2".to_owned(),
                        },
                        FixtureRunFile {
                            path: PathBuf::from("c.R"),
                            output: "still anything".to_owned(),
                        },
                    ],
                },
            ])
        },
    );
}

#[test]
fn rejects_extra_actual_outputs() {
    let fixture_directory = write_fixture_suite(indoc! {"
        #==== basics
        #---- scalar
        value <- 1
        #++++
        value: integer
    "});

    let panic = catch_unwind(AssertUnwindSafe(|| {
        run_fixture_suite(
            path_to_string(&fixture_directory).as_str(),
            "running",
            |fixture| {
                Ok(vec![FixtureOutput {
                    name: fixture.name.clone(),
                    files: vec![
                        FixtureRunFile {
                            path: PathBuf::new(),
                            output: "value: integer".to_owned(),
                        },
                        FixtureRunFile {
                            path: PathBuf::from("extra.R"),
                            output: "extra".to_owned(),
                        },
                    ],
                }])
            },
        );
    }))
    .expect_err("fixture suite should fail");

    let panic_message = panic_message(panic);
    assert!(panic_message.contains("unexpected output"));
    assert!(panic_message.contains("extra.R"));
}

#[test]
fn rejects_missing_outputs_even_for_any_expectations() {
    let fixture_directory = write_fixture_suite(indoc! {"
        #==== workspace
        #---- generational
        #.... v1
        #---- a.R
        alpha <- 1
        #++++ any
    "});

    let panic = catch_unwind(AssertUnwindSafe(|| {
        run_fixture_suite(
            path_to_string(&fixture_directory).as_str(),
            "running",
            |_fixture| {
                Ok(vec![FixtureOutput {
                    name: "v1".to_owned(),
                    files: Vec::new(),
                }])
            },
        );
    }))
    .expect_err("fixture suite should fail");

    let panic_message = panic_message(panic);
    assert!(panic_message.contains("missing output"));
    assert!(panic_message.contains("a.R"));
}

#[test]
fn rejects_exact_output_mismatches() {
    let fixture_directory = write_fixture_suite(indoc! {"
        #==== basics
        #---- scalar
        value <- 1
        #++++
        value: integer
    "});

    let panic = catch_unwind(AssertUnwindSafe(|| {
        run_fixture_suite(
            path_to_string(&fixture_directory).as_str(),
            "running",
            |fixture| {
                Ok(vec![FixtureOutput {
                    name: fixture.name.clone(),
                    files: vec![FixtureRunFile {
                        path: PathBuf::new(),
                        output: "value: character".to_owned(),
                    }],
                }])
            },
        );
    }))
    .expect_err("fixture suite should fail");

    let panic_message = panic_message(panic);
    assert!(panic_message.contains("expected:"));
    assert!(panic_message.contains("value: integer"));
    assert!(panic_message.contains("value: character"));
}

#[test]
fn rejects_duplicate_actual_paths_within_one_snapshot() {
    let fixture_directory = write_fixture_suite(indoc! {"
        #==== workspace
        #---- multi_file
        #---- a.R
        alpha <- 1
        #++++
        a snapshot
    "});

    let panic = catch_unwind(AssertUnwindSafe(|| {
        run_fixture_suite(
            path_to_string(&fixture_directory).as_str(),
            "running",
            |fixture| {
                Ok(vec![FixtureOutput {
                    name: fixture.name.clone(),
                    files: vec![
                        FixtureRunFile {
                            path: PathBuf::from("a.R"),
                            output: "a snapshot".to_owned(),
                        },
                        FixtureRunFile {
                            path: PathBuf::from("a.R"),
                            output: "other".to_owned(),
                        },
                    ],
                }])
            },
        );
    }))
    .expect_err("fixture suite should fail");

    let panic_message = panic_message(panic);
    assert!(panic_message.contains("duplicate output"));
    assert!(panic_message.contains("a.R"));
}

#[test]
fn rejects_snapshot_name_mismatches() {
    let fixture_directory = write_fixture_suite(indoc! {"
        #==== workspace
        #---- generational
        #.... v1
        #---- a.R
        alpha <- 1
        #++++
        a v1
    "});

    let panic = catch_unwind(AssertUnwindSafe(|| {
        run_fixture_suite(
            path_to_string(&fixture_directory).as_str(),
            "running",
            |_fixture| {
                Ok(vec![FixtureOutput {
                    name: "wrong".to_owned(),
                    files: vec![FixtureRunFile {
                        path: PathBuf::from("a.R"),
                        output: "a v1".to_owned(),
                    }],
                }])
            },
        );
    }))
    .expect_err("fixture suite should fail");

    let panic_message = panic_message(panic);
    assert!(panic_message.contains("expected snapshot"));
    assert!(panic_message.contains("v1"));
    assert!(panic_message.contains("wrong"));
}

#[test]
fn rejects_snapshot_count_mismatches() {
    let fixture_directory = write_fixture_suite(indoc! {"
        #==== workspace
        #---- generational
        #.... v1
        #---- a.R
        alpha <- 1
        #++++
        a v1
        #.... v2
        #---- edit a.R 1:11-1:11 -> \" + 2\"
        #++++
        a v2
    "});

    let panic = catch_unwind(AssertUnwindSafe(|| {
        run_fixture_suite(
            path_to_string(&fixture_directory).as_str(),
            "running",
            |_fixture| {
                Ok(vec![FixtureOutput {
                    name: "v1".to_owned(),
                    files: vec![FixtureRunFile {
                        path: PathBuf::from("a.R"),
                        output: "a v1".to_owned(),
                    }],
                }])
            },
        );
    }))
    .expect_err("fixture suite should fail");

    let panic_message = panic_message(panic);
    assert!(panic_message.contains("expected snapshots:"));
    assert!(panic_message.contains("actual snapshots:"));
}

#[test]
fn rejects_missing_first_generation_expectations() {
    let fixture_directory = write_fixture_suite(indoc! {"
        #==== workspace
        #---- generational
        #.... v1
        #---- a.R
        alpha <- 1
        #.... v2
        #---- edit a.R 1:11-1:11 -> \" + 2\"
        #++++
        a v2
    "});

    let panic = catch_unwind(AssertUnwindSafe(|| {
        run_fixture_suite(
            path_to_string(&fixture_directory).as_str(),
            "running",
            |_fixture| {
                Ok(vec![
                    FixtureOutput {
                        name: "v1".to_owned(),
                        files: Vec::new(),
                    },
                    FixtureOutput {
                        name: "v2".to_owned(),
                        files: vec![FixtureRunFile {
                            path: PathBuf::from("a.R"),
                            output: "a v2".to_owned(),
                        }],
                    },
                ])
            },
        );
    }))
    .expect_err("fixture suite should fail");

    assert!(panic_message(panic).contains("explicit first expectation"));
}

#[test]
fn rejects_runner_errors_as_unsupported_fixtures() {
    let fixture_directory = write_fixture_suite(indoc! {"
        #==== basics
        #---- scalar
        value <- 1
        #++++
        value: integer
    "});

    let panic = catch_unwind(AssertUnwindSafe(|| {
        run_fixture_suite(
            path_to_string(&fixture_directory).as_str(),
            "running",
            |_fixture| Err("runner failed".to_owned()),
        );
    }))
    .expect_err("fixture suite should fail");

    let panic_message = panic_message(panic);
    assert!(panic_message.contains("not supported"));
    assert!(panic_message.contains("runner failed"));
}

#[test]
fn rejects_duplicate_fixture_names() {
    let fixture_directory = write_fixture_suite(indoc! {"
        #==== basics
        #---- scalar
        value <- 1
        #++++
        one

        #---- scalar
        value <- 2
        #++++
        two
    "});

    let panic = catch_unwind(AssertUnwindSafe(|| {
        run_fixture_suite(
            path_to_string(&fixture_directory).as_str(),
            "running",
            |fixture| {
                Ok(vec![FixtureOutput {
                    name: fixture.name.clone(),
                    files: vec![FixtureRunFile {
                        path: PathBuf::new(),
                        output: "one".to_owned(),
                    }],
                }])
            },
        );
    }))
    .expect_err("fixture suite should fail");

    assert!(panic_message(panic).contains("duplicate typing running test snapshot name"));
}

#[test]
fn rejects_main_paths_in_generational_output_operations() {
    let fixture_directory = write_fixture_suite(indoc! {"
        #==== workspace
        #---- generational
        #.... v1
        #----
        alpha <- 1
        #++++
        a v1
    "});

    let panic = catch_unwind(AssertUnwindSafe(|| {
        run_fixture_suite(
            path_to_string(&fixture_directory).as_str(),
            "running",
            |_fixture| {
                Ok(vec![FixtureOutput {
                    name: "v1".to_owned(),
                    files: vec![FixtureRunFile {
                        path: PathBuf::from("a.R"),
                        output: "a v1".to_owned(),
                    }],
                }])
            },
        );
    }))
    .expect_err("fixture suite should fail");

    assert!(panic_message(panic).contains("implicit main path"));
}

fn write_fixture_suite(contents: &str) -> PathBuf {
    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after unix epoch")
        .as_nanos();
    let directory_path = std::env::temp_dir().join(format!(
        "roughly-fixtures-{}-{}",
        std::process::id(),
        unique_suffix
    ));
    fs::create_dir_all(&directory_path).expect("fixture directory should be created");
    fs::write(directory_path.join("suite.test"), contents).expect("fixture file should be written");
    directory_path
}

fn path_to_string(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

fn panic_message(panic: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = panic.downcast_ref::<String>() {
        return message.clone();
    }

    if let Some(message) = panic.downcast_ref::<&str>() {
        return (*message).to_owned();
    }

    "unknown panic payload".to_owned()
}
