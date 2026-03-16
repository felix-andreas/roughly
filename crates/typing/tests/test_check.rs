use {
    std::{collections::BTreeSet, fs, path::Path},
    typing::check,
};

#[test]
fn diagnostics() {
    run_fixture_suite("tests/fixtures/diagnostics");
}

#[derive(Debug)]
struct TestGroup {
    name: String,
    cases: Vec<TestCase>,
}

#[derive(Debug)]
struct TestCase {
    name: String,
    code: String,
    expected: String,
}

fn run_fixture_suite(directory_path: &str) {
    let fixture_paths = collect_fixture_paths(directory_path);
    let mut groups = Vec::new();

    for fixture_path in fixture_paths {
        let fixture_text = fs::read_to_string(&fixture_path).unwrap_or_else(|error| {
            panic!(
                "failed to read diagnostics fixture file `{}`: {error}",
                fixture_path.display()
            )
        });
        groups.extend(parse_test_file(&fixture_text));
    }

    run_test_groups(&groups);
}

fn collect_fixture_paths(directory_path: &str) -> Vec<std::path::PathBuf> {
    let mut fixture_paths = Vec::new();
    collect_fixture_paths_recursively(Path::new(directory_path), &mut fixture_paths);
    fixture_paths.sort();
    fixture_paths
}

fn collect_fixture_paths_recursively(
    directory_path: &Path,
    fixture_paths: &mut Vec<std::path::PathBuf>,
) {
    let entries = fs::read_dir(directory_path).unwrap_or_else(|error| {
        panic!(
            "failed to read diagnostics fixture directory `{}`: {error}",
            directory_path.display()
        )
    });

    let mut entry_paths = entries
        .map(|entry| {
            entry.unwrap_or_else(|error| {
                panic!(
                    "failed to read entry in diagnostics fixture directory `{}`: {error}",
                    directory_path.display()
                )
            })
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    entry_paths.sort();

    for entry_path in entry_paths {
        if entry_path.is_dir() {
            collect_fixture_paths_recursively(&entry_path, fixture_paths);
            continue;
        }

        if entry_path
            .extension()
            .is_some_and(|extension| extension == "test")
        {
            fixture_paths.push(entry_path);
        }
    }
}

fn parse_test_file(text: &str) -> Vec<TestGroup> {
    text.split("#====")
        .filter_map(|block| {
            if block.trim().is_empty() {
                return None;
            }

            let (name, cases) = block.split_once('\n').unwrap_or_else(|| {
                panic!("each test group must have a name line followed by content")
            });

            Some(TestGroup {
                name: name.trim().to_owned(),
                cases: cases
                    .split("#----")
                    .filter_map(|case| {
                        if case.trim().is_empty() {
                            return None;
                        }

                        let (name, body) = case.split_once('\n').unwrap_or_else(|| {
                            panic!("each test case must have a name line followed by content")
                        });

                        let (code, expected_block) =
                            body.split_once("#++++\n").unwrap_or_else(|| {
                                panic!(
                                    "each test case must include a `#++++` separator before the expected diagnostics"
                                )
                            });

                        Some(TestCase {
                            name: name.trim().to_owned(),
                            code: code.to_owned(),
                            expected: expected_block.trim_end().to_owned(),
                        })
                    })
                    .collect(),
            })
        })
        .collect()
}

fn run_test_groups(groups: &[TestGroup]) {
    let maybe_filter = std::env::var("TYPING_FILTER").ok();
    let mut snapshot_names = BTreeSet::new();

    for group in groups {
        for case in &group.cases {
            let snapshot_name = format!("{}__{}", group.name, case.name);

            assert!(
                snapshot_names.insert(snapshot_name.clone()),
                "duplicate typing test snapshot name `{snapshot_name}`"
            );

            if maybe_filter
                .as_ref()
                .is_some_and(|filter| !snapshot_name.contains(filter))
            {
                continue;
            }

            let rendered = check(&case.code).render(&case.code);
            assert_eq!(
                rendered.trim_end(),
                case.expected,
                "fixture `{snapshot_name}`"
            );
        }
    }
}
