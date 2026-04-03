use {
    fixtures::{
        FixtureKind, FixtureRunFile, expected_output_paths_for_generation, parse_file,
        run_fixture_suite,
    },
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

    run_fixture_suite(path_to_string(&fixture_directory).as_str(), |_fixture| {
        Ok(vec![vec![FixtureRunFile {
            path: PathBuf::new(),
            output: "value: integer".to_owned(),
        }]])
    });
}

#[test]
fn computes_expected_output_paths_for_multi_file_generations() {
    let fixture_file = parse_file(indoc! {"
        #==== workspace
        #---- generational
        #---- R/a.R
        alpha <- 1
        #++++
        a v1
        #---- R/b.R
        beta <- alpha
        #++++ any
        #.... v2
        #---- move R/b.R -> R/c.R
        #++++ hover.request
        hover v2
    "})
    .expect("fixture should parse");

    let FixtureKind::MultiFile(fixture) = &fixture_file.groups[0].cases[0].kind else {
        panic!("expected multi-file fixture case");
    };

    let initial_paths =
        expected_output_paths_for_generation(fixture, 0).expect("initial paths should compute");
    assert_eq!(initial_paths.name, "generational");
    assert_eq!(
        initial_paths.paths,
        vec![PathBuf::from("R/a.R"), PathBuf::from("R/b.R")]
    );

    let second_paths =
        expected_output_paths_for_generation(fixture, 1).expect("second paths should compute");
    assert_eq!(second_paths.name, "v2");
    assert_eq!(
        second_paths.paths,
        vec![
            PathBuf::from("R/a.R"),
            PathBuf::from("R/c.R"),
            PathBuf::from("hover.request"),
        ]
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

    run_fixture_suite(path_to_string(&fixture_directory).as_str(), |_fixture| {
        Ok(vec![vec![
            FixtureRunFile {
                path: PathBuf::from("b.R"),
                output: "b snapshot".to_owned(),
            },
            FixtureRunFile {
                path: PathBuf::from("a.R"),
                output: "a snapshot".to_owned(),
            },
        ]])
    });
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
        #---- lookup.hover
        a hover v1
        #++++
        hover v1
        #.... v2
        #---- edit a.R 1:11-1:11 -> \" + 2\"
        #++++
        a v2
        #---- move b.R -> c.R
        #++++ lookup.hover
        hover v2
        #---- delete old.R
    "});

    run_fixture_suite(path_to_string(&fixture_directory).as_str(), |_fixture| {
        Ok(vec![
            vec![
                FixtureRunFile {
                    path: PathBuf::from("a.R"),
                    output: "a v1".to_owned(),
                },
                FixtureRunFile {
                    path: PathBuf::from("b.R"),
                    output: "anything".to_owned(),
                },
                FixtureRunFile {
                    path: PathBuf::from("lookup.hover"),
                    output: "hover v1".to_owned(),
                },
            ],
            vec![
                FixtureRunFile {
                    path: PathBuf::from("a.R"),
                    output: "a v2".to_owned(),
                },
                FixtureRunFile {
                    path: PathBuf::from("c.R"),
                    output: "still anything".to_owned(),
                },
                FixtureRunFile {
                    path: PathBuf::from("lookup.hover"),
                    output: "hover v2".to_owned(),
                },
            ],
        ])
    });
}

#[test]
fn does_not_carry_ide_action_expectations_forward_across_generations() {
    let fixture_directory = write_fixture_suite(indoc! {"
        #==== workspace
        #---- ide
        #---- a.R
        alpha <- 1
        #++++ any
        #!!!! hover lookup.hover
        a.R:1:1
        #++++
        hover v1
        #.... v2
        #---- edit a.R 1:11-1:11 -> \" + 2\"
    "});

    run_fixture_suite(path_to_string(&fixture_directory).as_str(), |_fixture| {
        Ok(vec![
            vec![
                FixtureRunFile {
                    path: PathBuf::from("a.R"),
                    output: "anything".to_owned(),
                },
                FixtureRunFile {
                    path: PathBuf::from("lookup.hover"),
                    output: "hover v1".to_owned(),
                },
            ],
            vec![FixtureRunFile {
                path: PathBuf::from("a.R"),
                output: "still anything".to_owned(),
            }],
        ])
    });
}

#[test]
fn allows_delete_operations_to_assert_other_paths() {
    let fixture_directory = write_fixture_suite(indoc! {"
        #==== workspace
        #---- generational
        #.... v1
        #---- a.R
        alpha <- 1
        #++++ any
        #---- lookup.hover
        R/a.R:1:1
        #++++
        hover v1
        #.... v2
        #---- delete a.R
        #++++ lookup.hover
        no hover
    "});

    run_fixture_suite(path_to_string(&fixture_directory).as_str(), |_fixture| {
        Ok(vec![
            vec![
                FixtureRunFile {
                    path: PathBuf::from("a.R"),
                    output: "anything".to_owned(),
                },
                FixtureRunFile {
                    path: PathBuf::from("lookup.hover"),
                    output: "hover v1".to_owned(),
                },
            ],
            vec![FixtureRunFile {
                path: PathBuf::from("lookup.hover"),
                output: "no hover".to_owned(),
            }],
        ])
    });
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
        run_fixture_suite(path_to_string(&fixture_directory).as_str(), |_fixture| {
            Ok(vec![vec![
                FixtureRunFile {
                    path: PathBuf::new(),
                    output: "value: integer".to_owned(),
                },
                FixtureRunFile {
                    path: PathBuf::from("extra.R"),
                    output: "extra".to_owned(),
                },
            ]])
        });
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
        run_fixture_suite(path_to_string(&fixture_directory).as_str(), |_fixture| {
            Ok(vec![Vec::new()])
        });
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
        run_fixture_suite(path_to_string(&fixture_directory).as_str(), |_fixture| {
            Ok(vec![vec![FixtureRunFile {
                path: PathBuf::new(),
                output: "value: character".to_owned(),
            }]])
        });
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
        run_fixture_suite(path_to_string(&fixture_directory).as_str(), |_fixture| {
            Ok(vec![vec![
                FixtureRunFile {
                    path: PathBuf::from("a.R"),
                    output: "a snapshot".to_owned(),
                },
                FixtureRunFile {
                    path: PathBuf::from("a.R"),
                    output: "other".to_owned(),
                },
            ]])
        });
    }))
    .expect_err("fixture suite should fail");

    let panic_message = panic_message(panic);
    assert!(panic_message.contains("duplicate output"));
    assert!(panic_message.contains("a.R"));
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
        run_fixture_suite(path_to_string(&fixture_directory).as_str(), |_fixture| {
            Ok(vec![vec![FixtureRunFile {
                path: PathBuf::from("a.R"),
                output: "a v1".to_owned(),
            }]])
        });
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
        run_fixture_suite(path_to_string(&fixture_directory).as_str(), |_fixture| {
            Ok(vec![
                Vec::new(),
                vec![FixtureRunFile {
                    path: PathBuf::from("a.R"),
                    output: "a v2".to_owned(),
                }],
            ])
        });
    }))
    .expect_err("fixture suite should fail");

    assert!(
        panic_message(panic)
            .contains("initial multi-file generation entries must have an output expectation")
    );
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
        run_fixture_suite(path_to_string(&fixture_directory).as_str(), |_fixture| {
            Err("runner failed".to_owned())
        });
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
        run_fixture_suite(path_to_string(&fixture_directory).as_str(), |_fixture| {
            Ok(vec![vec![FixtureRunFile {
                path: PathBuf::new(),
                output: "one".to_owned(),
            }]])
        });
    }))
    .expect_err("fixture suite should fail");

    assert!(panic_message(panic).contains("duplicate fixture snapshot name"));
}

#[test]
fn rejects_empty_paths_in_generational_initial_generation() {
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
        run_fixture_suite(path_to_string(&fixture_directory).as_str(), |_fixture| {
            Ok(vec![vec![FixtureRunFile {
                path: PathBuf::from("a.R"),
                output: "a v1".to_owned(),
            }]])
        });
    }))
    .expect_err("fixture suite should fail");

    assert!(panic_message(panic).contains("document path must not be empty"));
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
