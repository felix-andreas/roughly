use {std::collections::BTreeSet, typing::check};

#[test]
fn diagnostics() {
    const DIAGNOSTICS_TESTS: &str = include_str!("fixtures/diagnostics.R.test");
    run_test_groups(&parse_test_file(DIAGNOSTICS_TESTS));
}

#[derive(Debug)]
struct TestGroup {
    name: &'static str,
    cases: Vec<TestCase>,
}

#[derive(Debug)]
struct TestCase {
    name: &'static str,
    code: &'static str,
    expected: &'static str,
}

fn parse_test_file(text: &'static str) -> Vec<TestGroup> {
    text.split("#====")
        .filter_map(|block| {
            if block.trim().is_empty() {
                return None;
            }

            let (name, cases) = block.split_once('\n').unwrap_or_else(|| {
                panic!("each test group must have a name line followed by content")
            });

            Some(TestGroup {
                name: name.trim(),
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
                            name: name.trim(),
                            code,
                            expected: expected_block.trim_end(),
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

            let rendered = check(case.code).render(case.code);
            assert_eq!(
                rendered.trim_end(),
                case.expected,
                "fixture `{snapshot_name}`"
            );
        }
    }
}
