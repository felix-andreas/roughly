use {
    fixtures::{
        FixtureKind, FixtureRunFile, expected_output_paths_for_generation, parse_file,
        run_fixture_suite, run_fixture_suite_with_bless,
    },
    indoc::indoc,
    std::{
        fs,
        panic::{AssertUnwindSafe, catch_unwind},
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
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

#[test]
fn blesses_simple_fixture_expectation() {
    let fixture_directory = write_fixture_suite(indoc! {"
        #==== basics
        #---- scalar
        value <- 1
        #++++
        WRONG
    "});

    run_fixture_suite_with_bless(
        path_to_string(&fixture_directory).as_str(),
        |_fixture| {
            Ok(vec![vec![FixtureRunFile {
                path: PathBuf::new(),
                output: "value: integer".to_owned(),
            }]])
        },
        true,
    );

    assert_eq!(
        read_suite_file(&fixture_directory),
        indoc! {"
            #==== basics
            #---- scalar
            value <- 1
            #++++
            value: integer
        "}
    );

    // Re-running without bless against the same actual output must now pass.
    run_fixture_suite(path_to_string(&fixture_directory).as_str(), |_fixture| {
        Ok(vec![vec![FixtureRunFile {
            path: PathBuf::new(),
            output: "value: integer".to_owned(),
        }]])
    });
}

#[test]
fn blesses_multi_file_and_generation_expectations() {
    let fixture_directory = write_fixture_suite(indoc! {"
        #==== workspace
        #---- generational
        #---- a.R
        alpha <- 1
        #++++
        WRONG a v1
        #---- b.R
        beta <- alpha
        #++++
        WRONG b v1
        #.... v2
        #---- edit a.R 1:11-1:11 -> \" + 2\"
        #++++
        WRONG a v2
    "});

    let run_fixture = |_fixture: &fixtures::Fixture| {
        Ok(vec![
            vec![
                FixtureRunFile {
                    path: PathBuf::from("a.R"),
                    output: "a v1".to_owned(),
                },
                FixtureRunFile {
                    path: PathBuf::from("b.R"),
                    output: "b v1".to_owned(),
                },
            ],
            vec![
                FixtureRunFile {
                    path: PathBuf::from("a.R"),
                    output: "a v2".to_owned(),
                },
                FixtureRunFile {
                    path: PathBuf::from("b.R"),
                    output: "b v1".to_owned(),
                },
            ],
        ])
    };

    run_fixture_suite_with_bless(path_to_string(&fixture_directory).as_str(), run_fixture, true);

    assert_eq!(
        read_suite_file(&fixture_directory),
        indoc! {"
            #==== workspace
            #---- generational
            #---- a.R
            alpha <- 1
            #++++
            a v1
            #---- b.R
            beta <- alpha
            #++++
            b v1
            #.... v2
            #---- edit a.R 1:11-1:11 -> \" + 2\"
            #++++
            a v2
        "}
    );

    // The carried-forward `b.R` expectation must still hold in the second generation.
    run_fixture_suite(path_to_string(&fixture_directory).as_str(), run_fixture);
}

#[test]
fn leaves_any_expectations_untouched_when_blessing() {
    let fixture_directory = write_fixture_suite(indoc! {"
        #==== workspace
        #---- mixed
        #---- a.R
        alpha <- 1
        #++++
        WRONG a
        #---- b.R
        beta <- alpha
        #++++ any
    "});

    run_fixture_suite_with_bless(
        path_to_string(&fixture_directory).as_str(),
        |_fixture| {
            Ok(vec![vec![
                FixtureRunFile {
                    path: PathBuf::from("a.R"),
                    output: "a out".to_owned(),
                },
                FixtureRunFile {
                    path: PathBuf::from("b.R"),
                    output: "b out".to_owned(),
                },
            ]])
        },
        true,
    );

    assert_eq!(
        read_suite_file(&fixture_directory),
        indoc! {"
            #==== workspace
            #---- mixed
            #---- a.R
            alpha <- 1
            #++++
            a out
            #---- b.R
            beta <- alpha
            #++++ any
        "}
    );
}

#[test]
fn blessing_a_correct_suite_changes_no_bytes() {
    let original = indoc! {"
        #==== basics
        #---- scalar
        value <- 1
        #++++
        value: integer
    "};
    let fixture_directory = write_fixture_suite(original);

    run_fixture_suite_with_bless(
        path_to_string(&fixture_directory).as_str(),
        |_fixture| {
            Ok(vec![vec![FixtureRunFile {
                path: PathBuf::new(),
                output: "value: integer".to_owned(),
            }]])
        },
        true,
    );

    assert_eq!(read_suite_file(&fixture_directory), original);
}

fn read_suite_file(fixture_directory: &Path) -> String {
    fs::read_to_string(fixture_directory.join("suite.test")).expect("suite file should be readable")
}

fn write_fixture_suite(contents: &str) -> PathBuf {
    // Tests run in parallel, so a timestamp is not unique enough to keep their suite
    // directories apart; a process-wide counter is.
    static FIXTURE_DIRECTORY_COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique_suffix = FIXTURE_DIRECTORY_COUNTER.fetch_add(1, Ordering::Relaxed);
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
