use {
    fixtures::{
        FixtureExpectedOutput, FixtureGenerationEntry, FixtureGenerationExpectation, FixtureKind,
        FixtureOperation, ParseFixtureError, parse_file,
    },
    indoc::indoc,
    std::path::PathBuf,
};

#[test]
fn parses_simple_fixture_files() {
    let fixture_file = parse_file(indoc! {"
        #==== basics
        #---- scalar
        value <- 1
        #++++
        value: integer
    "})
    .expect("fixture should parse");

    assert_eq!(fixture_file.groups.len(), 1);
    assert_eq!(fixture_file.groups[0].name, "basics");
    assert_eq!(fixture_file.groups[0].cases.len(), 1);

    match &fixture_file.groups[0].cases[0].kind {
        FixtureKind::Simple(case) => {
            assert_eq!(case.input, "value <- 1\n");
            assert_eq!(case.expected, "value: integer");
        }
        _ => panic!("expected simple fixture case"),
    }
}

#[test]
fn rejects_groups_without_bodies() {
    let error = parse_file("#==== basics").expect_err("fixture should fail");

    assert_eq!(
        error,
        ParseFixtureError::InvalidFixture {
            message: "each test group must have a name line followed by content".to_owned()
        }
    );
}

#[test]
fn parses_multiple_groups_and_cases() {
    let fixture_file = parse_file(indoc! {"
        #==== basics
        #---- one
        value <- 1
        #++++
        one

        #---- two
        value <- 2
        #++++
        two

        #==== more
        #---- three
        value <- 3
        #++++
        three
    "})
    .expect("fixture should parse");

    assert_eq!(fixture_file.groups.len(), 2);
    assert_eq!(fixture_file.groups[0].cases.len(), 2);
    assert_eq!(fixture_file.groups[1].cases.len(), 1);
    assert_eq!(fixture_file.groups[0].cases[1].name, "two");
    assert_eq!(fixture_file.groups[1].cases[0].name, "three");
}

#[test]
fn parses_multi_file_fixture_cases_with_immediate_expectations() {
    let fixture_file = parse_file(indoc! {"
        #==== workspace
        #---- multi_file
        #---- a.R
        alpha <- 1
        #++++
        a snapshot
        #---- b.R
        beta <- alpha
        #++++ any
    "})
    .expect("fixture should parse");

    match &fixture_file.groups[0].cases[0].kind {
        FixtureKind::MultiFile(case) => {
            assert_eq!(case.initial_generation.name, "multi_file");
            assert_eq!(case.initial_generation.entries.len(), 2);
            assert_eq!(
                case.initial_generation.entries[0],
                FixtureGenerationEntry {
                    operation: FixtureOperation::CreateDocument {
                        path: PathBuf::from("a.R"),
                        contents: "alpha <- 1\n".to_owned(),
                    },
                    expectation: Some(FixtureGenerationExpectation {
                        path: PathBuf::from("a.R"),
                        output: FixtureExpectedOutput::Exact("a snapshot".to_owned()),
                    }),
                }
            );
            assert_eq!(
                case.initial_generation.entries[1],
                FixtureGenerationEntry {
                    operation: FixtureOperation::CreateDocument {
                        path: PathBuf::from("b.R"),
                        contents: "beta <- alpha\n".to_owned(),
                    },
                    expectation: Some(FixtureGenerationExpectation {
                        path: PathBuf::from("b.R"),
                        output: FixtureExpectedOutput::Any,
                    }),
                }
            );
            assert!(case.generations.is_empty());
        }
        _ => panic!("expected multi-file fixture case"),
    }
}

#[test]
fn parses_generation_based_fixture_cases_with_ordered_entries() {
    let fixture_file = parse_file(indoc! {r#"
        #==== workspace
        #---- multi_file
        #.... v1
        #---- a.R
        alpha <- 1
        #++++
        a v1
        #---- b.R
        beta <- alpha
        #++++ any
        #.... v2
        #---- edit a.R 1:11-1:11 -> " + 2"
        #++++
        a v2
        #---- move b.R -> subdir/b.R
        #---- delete old.R
    "#})
    .expect("fixture should parse");

    match &fixture_file.groups[0].cases[0].kind {
        FixtureKind::MultiFile(case) => {
            assert_eq!(case.initial_generation.name, "v1");
            assert_eq!(case.initial_generation.entries.len(), 2);
            assert_eq!(
                case.initial_generation.entries[0].expectation,
                Some(FixtureGenerationExpectation {
                    path: PathBuf::from("a.R"),
                    output: FixtureExpectedOutput::Exact("a v1".to_owned()),
                })
            );
            assert_eq!(
                case.initial_generation.entries[1].expectation,
                Some(FixtureGenerationExpectation {
                    path: PathBuf::from("b.R"),
                    output: FixtureExpectedOutput::Any,
                })
            );
            assert_eq!(case.generations.len(), 1);
            assert_eq!(case.generations[0].name, "v2");

            match &case.generations[0].entries[0].operation {
                FixtureOperation::EditDocument {
                    path,
                    range,
                    replacement_text,
                } => {
                    assert_eq!(path, &PathBuf::from("a.R"));
                    assert_eq!(range.start.line_number, 1);
                    assert_eq!(range.start.column_number, 11);
                    assert_eq!(replacement_text, " + 2");
                }
                _ => panic!("expected edit operation"),
            }

            match &case.generations[0].entries[1].operation {
                FixtureOperation::MoveDocument {
                    source_path,
                    destination_path,
                } => {
                    assert_eq!(source_path, &PathBuf::from("b.R"));
                    assert_eq!(destination_path, &PathBuf::from("subdir/b.R"));
                }
                _ => panic!("expected move operation"),
            }

            assert!(matches!(
                case.generations[0].entries[2].operation,
                FixtureOperation::DeleteDocument { .. }
            ));
            assert!(case.generations[0].entries[2].expectation.is_none());
        }
        _ => panic!("expected generation-based multi-file fixture case"),
    }
}

#[test]
fn parses_initial_ide_actions() {
    let fixture_file = parse_file(indoc! {"
        #==== workspace
        #---- ide
        #---- R/main.R
        value <- function(parameter) parameter
        #++++ any
        #!!!! hover lookup.hover
        R/main.R:1:30
        #++++
        hover output
    "})
    .expect("fixture should parse");

    let FixtureKind::MultiFile(case) = &fixture_file.groups[0].cases[0].kind else {
        panic!("expected multi-file fixture case");
    };

    assert_eq!(case.initial_generation.entries.len(), 2);
    assert_eq!(
        case.initial_generation.entries[1],
        FixtureGenerationEntry {
            operation: FixtureOperation::Action {
                action: "hover".to_owned(),
                path: PathBuf::from("lookup.hover"),
                contents: "R/main.R:1:30\n".to_owned(),
            },
            expectation: Some(FixtureGenerationExpectation {
                path: PathBuf::from("lookup.hover"),
                output: FixtureExpectedOutput::Exact("hover output".to_owned()),
            }),
        }
    );
}

#[test]
fn rejects_simple_expectations_with_paths() {
    let error = parse_file(indoc! {"
        #==== basics
        #---- invalid
        value <- 1
        #++++ value.R
        nope
    "})
    .expect_err("fixture should fail");

    assert_eq!(
        error,
        ParseFixtureError::InvalidFixture {
            message: "simple fixture cases must use a bare `#++++` separator with no path"
                .to_owned()
        }
    );
}

#[test]
fn rejects_simple_cases_without_expectations() {
    let error = parse_file(indoc! {"
        #==== basics
        #---- invalid
        value <- 1
    "})
    .expect_err("fixture should fail");

    assert_eq!(
        error,
        ParseFixtureError::InvalidFixture {
            message: "simple fixture cases must include a `#++++` separator".to_owned()
        }
    );
}

#[test]
fn rejects_multi_file_inputs_without_immediate_expectations() {
    let error = parse_file(indoc! {"
        #==== workspace
        #---- invalid
        #---- a.R
        alpha <- 1
        #---- b.R
        beta <- alpha
        #++++
        b snapshot
    "})
    .expect_err("fixture should fail");

    assert_eq!(
        error,
        ParseFixtureError::InvalidFixture {
            message: "initial multi-file generation entries must have an output expectation"
                .to_owned()
        }
    );
}

#[test]
fn rejects_multi_file_operations() {
    let error = parse_file(indoc! {"
        #==== workspace
        #---- invalid
        #---- edit a.R 1:1-1:1 -> \"x\"
        #++++
        anything
    "})
    .expect_err("fixture should fail");

    assert_eq!(
        error,
        ParseFixtureError::InvalidFixture {
            message: "initial multi-file generation may only create whole documents or actions"
                .to_owned()
        }
    );
}

#[test]
fn rejects_multi_file_empty_paths() {
    let error = parse_file(indoc! {"
        #==== workspace
        #---- invalid
        #----
        alpha <- 1
        #++++
        anything
    "})
    .expect_err("fixture should fail");

    assert_eq!(
        error,
        ParseFixtureError::InvalidFixture {
            message: "document path must not be empty".to_owned()
        }
    );
}

#[test]
fn parses_generation_expectations_with_explicit_target_paths() {
    let fixture_file = parse_file(indoc! {"
        #==== workspace
        #---- generational
        #.... v1
        #---- a.R
        alpha <- 1
        #++++
        a v1
        #---- lookup.hover
        R/a.R:1:1
        #++++
        hover v1
        #.... v2
        #---- move a.R -> b.R
        #++++ lookup.hover
        hover v2
        #---- delete b.R
        #++++ lookup.hover
        no hover
    "})
    .expect("fixture should parse");

    let FixtureKind::MultiFile(case) = &fixture_file.groups[0].cases[0].kind else {
        panic!("expected multi-file fixture case");
    };

    assert_eq!(case.generations.len(), 1);
    assert_eq!(
        case.generations[0].entries[0].expectation,
        Some(FixtureGenerationExpectation {
            path: PathBuf::from("lookup.hover"),
            output: FixtureExpectedOutput::Exact("hover v2".to_owned()),
        })
    );
    assert_eq!(
        case.generations[0].entries[1].expectation,
        Some(FixtureGenerationExpectation {
            path: PathBuf::from("lookup.hover"),
            output: FixtureExpectedOutput::Exact("no hover".to_owned()),
        })
    );
}

#[test]
fn allows_targeted_initial_multi_file_expectation_headers() {
    parse_file(indoc! {"
        #==== workspace
        #---- allowed
        #---- a.R
        alpha <- 1
        #++++ a.R
        a snapshot
    "})
    .expect("fixture should parse");
}

#[test]
fn rejects_untargeted_generation_expectations_for_delete_operations() {
    let error = parse_file(indoc! {"
        #==== workspace
        #---- invalid
        #.... v1
        #---- a.R
        alpha <- 1
        #++++
        a v1
        #.... v2
        #---- delete a.R
        #++++
        gone
    "})
    .expect_err("fixture should fail");

    assert_eq!(
        error,
        ParseFixtureError::InvalidFixture {
            message:
                "output expectations without a target path require an operation with an output path"
                    .to_owned()
        }
    );
}

#[test]
fn rejects_generations_without_name_lines() {
    let error = parse_file(indoc! {"
        #==== workspace
        #---- invalid
        #....
    "})
    .expect_err("fixture should fail");

    assert_eq!(
        error,
        ParseFixtureError::InvalidFixture {
            message: "multi-file fixture cases must include at least one input file".to_owned()
        }
    );
}

#[test]
fn rejects_initial_generational_edits() {
    let error = parse_file(indoc! {r#"
        #==== workspace
        #---- invalid
        #.... v1
        #---- edit a.R 1:1-1:1 -> "x"
    "#})
    .expect_err("fixture should fail");

    assert_eq!(
        error,
        ParseFixtureError::InvalidFixture {
            message: "initial multi-file generation entries must have an output expectation"
                .to_owned()
        }
    );
}

#[test]
fn parses_unknown_ide_actions_without_runner_validation() {
    let fixture_file = parse_file(indoc! {"
        #==== workspace
        #---- ide
        #---- a.R
        alpha <- 1
        #++++ any
        #!!!! references refs.request
        alpha
        #++++
        anything
    "})
    .expect("fixture should parse");

    let FixtureKind::MultiFile(case) = &fixture_file.groups[0].cases[0].kind else {
        panic!("expected multi-file fixture case");
    };

    assert_eq!(
        case.initial_generation.entries[1],
        FixtureGenerationEntry {
            operation: FixtureOperation::Action {
                action: "references".to_owned(),
                path: PathBuf::from("refs.request"),
                contents: "alpha\n".to_owned(),
            },
            expectation: Some(FixtureGenerationExpectation {
                path: PathBuf::from("refs.request"),
                output: FixtureExpectedOutput::Exact("anything".to_owned()),
            }),
        }
    );
}

#[test]
fn rejects_any_expectations_with_bodies() {
    let error = parse_file(indoc! {"
        #==== workspace
        #---- invalid
        #---- a.R
        alpha <- 1
        #++++ any
        body
    "})
    .expect_err("fixture should fail");

    assert_eq!(
        error,
        ParseFixtureError::InvalidFixture {
            message: "`#++++ any` must not have an expectation body".to_owned()
        }
    );
}

#[test]
fn rejects_move_operations_without_arrow() {
    let error = parse_file(indoc! {"
        #==== workspace
        #---- invalid
        #.... v1
        #---- move a.R b.R
    "})
    .expect_err("fixture should fail");

    assert_eq!(
        error,
        ParseFixtureError::InvalidFixture {
            message: "move operations must use `#---- move source -> destination`".to_owned()
        }
    );
}

#[test]
fn rejects_edit_operations_without_arrow() {
    let error = parse_file(indoc! {"
        #==== workspace
        #---- invalid
        #.... v1
        #---- edit a.R 1:1-1:1
    "})
    .expect_err("fixture should fail");

    assert_eq!(
        error,
        ParseFixtureError::InvalidFixture {
            message: "edit operations must use `#---- edit path start:end-start:end -> \"text\"`"
                .to_owned()
        }
    );
}

#[test]
fn rejects_edit_operations_with_invalid_json_string() {
    let error = parse_file(indoc! {r#"
        #==== workspace
        #---- invalid
        #.... v1
        #---- edit a.R 1:1-1:1 -> nope
    "#})
    .expect_err("fixture should fail");

    assert!(matches!(
        error,
        ParseFixtureError::InvalidFixture { message }
            if message.starts_with("edit replacement text must be a valid JSON string literal:")
    ));
}

#[test]
fn rejects_edit_operations_without_ranges() {
    let error = parse_file(indoc! {r#"
        #==== workspace
        #---- invalid
        #.... v1
        #---- edit a.R -> "x"
    "#})
    .expect_err("fixture should fail");

    assert_eq!(
        error,
        ParseFixtureError::InvalidFixture {
            message: "edit operations must include a path and a range before `->`".to_owned()
        }
    );
}

#[test]
fn rejects_ranges_without_separators() {
    let error = parse_file(indoc! {r#"
        #==== workspace
        #---- invalid
        #.... v1
        #---- edit a.R 1:1 -> "x"
    "#})
    .expect_err("fixture should fail");

    assert_eq!(
        error,
        ParseFixtureError::InvalidFixture {
            message: "ranges must use `start_line:start_column-end_line:end_column`".to_owned()
        }
    );
}

#[test]
fn rejects_positions_without_columns() {
    let error = parse_file(indoc! {r#"
        #==== workspace
        #---- invalid
        #.... v1
        #---- edit a.R 1-1:1 -> "x"
    "#})
    .expect_err("fixture should fail");

    assert_eq!(
        error,
        ParseFixtureError::InvalidFixture {
            message: "positions must use `line:column`".to_owned()
        }
    );
}

#[test]
fn rejects_zero_based_positions() {
    let error = parse_file(indoc! {r#"
        #==== workspace
        #---- invalid
        #.... v1
        #---- edit a.R 0:1-1:1 -> "x"
    "#})
    .expect_err("fixture should fail");

    assert_eq!(
        error,
        ParseFixtureError::InvalidFixture {
            message: "line number values are 1-based".to_owned()
        }
    );
}

#[test]
fn rejects_non_numeric_positions() {
    let error = parse_file(indoc! {r#"
        #==== workspace
        #---- invalid
        #.... v1
        #---- edit a.R x:1-1:1 -> "x"
    "#})
    .expect_err("fixture should fail");

    assert!(matches!(
        error,
        ParseFixtureError::InvalidFixture { message }
            if message.starts_with("invalid line number `x`:")
    ));
}
