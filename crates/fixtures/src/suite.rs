use {
    crate::{Fixture, FixtureGroup, parse_file},
    std::{collections::BTreeSet, fs, path::Path},
};

pub fn run_fixture_suite<RunFixture>(directory_path: &str, kind: &str, mut run_fixture: RunFixture)
where
    RunFixture: FnMut(&Fixture) -> Result<String, String>,
{
    let groups = read_fixture_suite(directory_path, kind);
    let maybe_filter = std::env::var("FIXTURE_FILTER").ok();
    let mut snapshot_names = BTreeSet::new();
    let mut failures = Vec::new();
    let mut executed_fixture_count = 0;

    for group in &groups {
        for fixture in &group.cases {
            let snapshot_name = format!("{}__{}", group.name, fixture.name);

            assert!(
                snapshot_names.insert(snapshot_name.clone()),
                "duplicate typing {kind} test snapshot name `{snapshot_name}`"
            );

            let expected = match expected_output(fixture) {
                Ok(expected) => expected,
                Err(error) => {
                    failures.push(format!(
                "fixture `{snapshot_name}` is not supported by the current typing `{kind}` runner: {error}"
            ));
                    continue;
                }
            };

            if maybe_filter
                .as_ref()
                .is_some_and(|filter| !snapshot_name.contains(filter))
            {
                continue;
            }

            executed_fixture_count += 1;

            let actual_output = match run_fixture(fixture) {
                Ok(actual_output) => actual_output,
                Err(error) => {
                    failures.push(format!(
                        "fixture `{snapshot_name}` is not supported by the current typing `{kind}` runner: {error}"
                    ));
                    continue;
                }
            };
            let actual_output = actual_output.trim_end();
            if actual_output != expected {
                failures.push(format!(
                    "\u{1b}[1mfixture `{snapshot_name}` failed\u{1b}[0m\n\u{1b}[1minput:\u{1b}[0m\n{}\n\u{1b}[1mexpected:\u{1b}[0m\n{}\n\u{1b}[1mactual:\u{1b}[0m\n{}",
                    format!("{fixture:?}").trim_end(),
                    expected,
                    actual_output
                ));
            }
        }
    }

    if !failures.is_empty() {
        panic!(
            "{} {kind} test(s) failed:\n\n{}",
            failures.len(),
            failures.join("\n\n")
        );
    }

    eprintln!("{} {kind} fixture(s) passed", executed_fixture_count);
}

fn expected_output(fixture: &Fixture) -> Result<String, String> {
    match &fixture.kind {
        crate::FixtureKind::Simple(case) => Ok(case.expected.clone()),
        crate::FixtureKind::MultiFile(case) => {
            Ok(render_multi_file_output(case.expectations.iter().map(
                |expectation| (expectation.path.as_path(), expectation.expected.as_str()),
            )))
        }
        crate::FixtureKind::Generational(_) => {
            Err("generation-based multi-file cases are not supported".to_owned())
        }
    }
}

fn render_multi_file_output<'a>(entries: impl IntoIterator<Item = (&'a Path, &'a str)>) -> String {
    entries
        .into_iter()
        .map(|(path, expected)| format!("== {} ==\n{}", path.display(), expected.trim_end()))
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub fn read_fixture_suite(directory_path: &str, kind: &str) -> Vec<FixtureGroup> {
    let fixture_paths = collect_fixture_paths(directory_path, kind);
    let mut groups = Vec::new();

    for fixture_path in fixture_paths {
        let fixture_text = fs::read_to_string(&fixture_path).unwrap_or_else(|error| {
            panic!(
                "failed to read {kind} fixture file `{}`: {error}",
                fixture_path.display()
            )
        });
        let fixture_file = parse_file(&fixture_text).unwrap_or_else(|error| {
            panic!(
                "failed to parse {kind} fixture file `{}`: {error}",
                fixture_path.display()
            )
        });
        groups.extend(fixture_file.groups);
    }

    groups
}

fn collect_fixture_paths(directory_path: &str, kind: &str) -> Vec<std::path::PathBuf> {
    let mut fixture_paths = Vec::new();
    collect_fixture_paths_recursively(Path::new(directory_path), &mut fixture_paths, kind);
    fixture_paths.sort();
    fixture_paths
}

fn collect_fixture_paths_recursively(
    directory_path: &Path,
    fixture_paths: &mut Vec<std::path::PathBuf>,
    kind: &str,
) {
    let entries = fs::read_dir(directory_path).unwrap_or_else(|error| {
        panic!(
            "failed to read {kind} fixture directory `{}`: {error}",
            directory_path.display()
        )
    });

    let mut entry_paths = entries
        .map(|entry| {
            entry.unwrap_or_else(|error| {
                panic!(
                    "failed to read entry in {kind} fixture directory `{}`: {error}",
                    directory_path.display()
                )
            })
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    entry_paths.sort();

    for entry_path in entry_paths {
        if entry_path.is_dir() {
            collect_fixture_paths_recursively(&entry_path, fixture_paths, kind);
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
