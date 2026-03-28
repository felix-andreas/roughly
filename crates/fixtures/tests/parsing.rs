use {
    fixtures::{
        FixtureExpectation, FixtureKind, FixtureOperation, FixturePath, ParseFixtureError,
        parse_file,
    },
    indoc::indoc,
    workspace::{DocumentKind, TextPosition, TextRange, Workspace},
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
fn parses_multi_file_fixture_cases() {
    let fixture_file = parse_file(indoc! {"
        #==== workspace
        #---- multi_file
        #---- a.R
        alpha <- 1
        #---- b.R
        beta <- alpha
        #++++ a.R
        a snapshot
        #++++ b.R
        b snapshot
    "})
    .expect("fixture should parse");

    match &fixture_file.groups[0].cases[0].kind {
        FixtureKind::MultiFile(case) => {
            assert_eq!(case.input_files.len(), 2);
            assert_eq!(case.input_files[0].path, std::path::PathBuf::from("a.R"));
            assert_eq!(case.expectations.len(), 2);
            assert_eq!(case.expectations[1].path, std::path::PathBuf::from("b.R"));
        }
        _ => panic!("expected multi-file fixture case"),
    }
}

#[test]
fn parses_generation_based_multi_file_fixture_cases() {
    let fixture_file = parse_file(indoc! {r#"
        #==== workspace
        #---- multi_file
        #.... v1
        #---- a.R
        alpha <- 1
        #---- b.R
        beta <- alpha
        #++++ a.R
        a v1
        #++++ b.R
        b v1
        #.... v2
        #---- edit a.R 1:11-1:11 -> " + 2"
        #---- move b.R -> subdir/b.R
        #---- delete old.R
        #++++ a.R
        a v2
        #++++ none subdir/b.R
    "#})
    .expect("fixture should parse");

    match &fixture_file.groups[0].cases[0].kind {
        FixtureKind::Generational(case) => {
            assert_eq!(case.generations.len(), 2);
            assert_eq!(case.generations[0].name, "v1");
            assert_eq!(case.generations[0].operations.len(), 2);
            assert_eq!(case.generations[0].expectations.len(), 2);

            match &case.generations[1].operations[0] {
                FixtureOperation::EditDocument {
                    path,
                    range,
                    replacement_text,
                } => {
                    assert_eq!(path, &FixturePath::Path("a.R".into()));
                    assert_eq!(range.start.line_number, 1);
                    assert_eq!(range.start.column_number, 11);
                    assert_eq!(replacement_text, " + 2");
                }
                _ => panic!("expected edit operation"),
            }

            assert!(matches!(
                case.generations[1].expectations[1],
                FixtureExpectation::Clear { .. }
            ));
        }
        _ => panic!("expected generation-based multi-file fixture case"),
    }
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
fn parsed_workspace_operations_apply_to_workspace() {
    let fixture_file = parse_file(indoc! {r#"
        #==== workspace
        #---- evolution
        #.... v1
        #---- a.R
        alpha <- 1
        #---- b.R
        beta <- alpha
        #.... v2
        #---- edit a.R 1:11-1:11 -> " + 2"
        #---- move b.R -> nested/b.R
        #---- delete nested/b.R
    "#})
    .expect("fixture should parse");

    let mut workspace = Workspace::new().expect("workspace should initialize");
    let FixtureKind::Generational(case) = &fixture_file.groups[0].cases[0].kind else {
        panic!("expected generation-based multi-file fixture case");
    };

    for generation in &case.generations {
        for operation in &generation.operations {
            match operation {
                FixtureOperation::CreateDocument { path, contents } => workspace
                    .replace_document(workspace_path(path), contents, DocumentKind::Standalone)
                    .expect("replace should succeed"),
                FixtureOperation::EditDocument {
                    path,
                    range,
                    replacement_text,
                } => workspace
                    .edit_document_range(
                        workspace_path(path),
                        TextRange {
                            start: TextPosition {
                                line_index: range.start.line_number - 1,
                                character_index: range.start.column_number - 1,
                            },
                            end: TextPosition {
                                line_index: range.end.line_number - 1,
                                character_index: range.end.column_number - 1,
                            },
                        },
                        replacement_text,
                    )
                    .expect("edit should succeed"),
                FixtureOperation::MoveDocument {
                    source_path,
                    destination_path,
                } => workspace
                    .move_document(
                        workspace_path(source_path),
                        workspace_path(destination_path),
                    )
                    .expect("move should succeed"),
                FixtureOperation::DeleteDocument { path } => workspace
                    .delete_document(workspace_path(path))
                    .expect("delete should succeed"),
            }
        }
    }

    let document = workspace
        .document("a.R")
        .expect("document should exist after edits");
    assert_eq!(document.document().rope().to_string(), "alpha <- 1 + 2\n");
    assert!(workspace.document("nested/b.R").is_none());
}

fn workspace_path(path: &FixturePath) -> std::path::PathBuf {
    match path {
        FixturePath::Main => "main.R".into(),
        FixturePath::Path(path) => path.clone(),
    }
}
