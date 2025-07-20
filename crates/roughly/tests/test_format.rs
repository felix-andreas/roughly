use {
    indoc::indoc,
    ropey::Rope,
    roughly::{
        format::{Config, FormatError, LineEnding, format},
        tree,
    },
};

fn format_str(text: &str) -> Result<String, FormatError> {
    format(
        tree::parse(&mut tree::new_parser(), text, None).root_node(),
        &Rope::from_str(text),
        Config {
            indent_width: 2,
            line_ending: LineEnding::Auto,
        },
    )
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
}

fn parse_test_file(text: &'static str) -> Vec<TestGroup> {
    text.split("#====")
        .filter_map(|block| {
            if block.trim().is_empty() {
                return None;
            }
            let (name, cases) = block.split_once('\n').unwrap();
            Some(TestGroup {
                name: name.trim(), // trim potental \r
                cases: cases
                    .split("#----")
                    .filter_map(|case| {
                        if case.trim().is_empty() {
                            return None;
                        }
                        let (name, code) = case.split_once("\n").unwrap();
                        Some(TestCase {
                            name: name.trim(), // trim potental \r
                            code,
                        })
                    })
                    .collect(),
            })
        })
        .collect()
}

fn run_test_groups(groups: &[TestGroup]) {
    let maybe_filter = std::env::var("FORMAT_FILTER").ok();
    for group in groups {
        for case in &group.cases {
            let snapshot = format!("{}__{}", group.name, case.name);
            if maybe_filter
                .as_ref()
                .is_some_and(|filter| !snapshot.contains(filter))
            {
                continue;
            }

            let code = format_str(case.code).unwrap();
            insta::assert_snapshot!(snapshot, code);
        }
    }
}

#[test]
fn base() {
    const BASE_TESTS: &str = include_str!("format/base.R.test");
    run_test_groups(&parse_test_file(BASE_TESTS));
}

#[test]
fn special() {
    const BASE_TESTS: &str = include_str!("format/special.R.test");
    run_test_groups(&parse_test_file(BASE_TESTS));
}

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// EDGE CASES
// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Line formatting is tested internally - this function verifies the line ending behavior
#[test]
fn line_formatting() {
    assert_eq!(
        "foo\nbar\nbaz\n",
        format_str("foo \n bar \n baz \n").unwrap()
    );
    assert_eq!(
        "foo\nbar\nbaz\n",
        format_str("foo\nbar\r\nbaz\r\n").unwrap()
    );
    assert_eq!(
        "foo\r\nbar\r\nbaz\r\n",
        format_str("foo\r\nbar\r\nbaz\r\n").unwrap()
    );
    assert_eq!(
        "foo\r\nbar\r\nbaz\r\n",
        format_str("foo\r\nbar\nbaz\n").unwrap()
    );
}

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// ERROR CASES
#[test]
fn error() {
    let result = format_str(indoc! {r#"
        function
    "#});

    let Err(FormatError::SyntaxError { kind, line, col }) = result else {
        panic!()
    };
    assert_eq!(kind, "ERROR");
    assert_eq!(line, 0);
    assert_eq!(col, 0);
}

#[test]
fn missing() {
    let result = format_str(indoc! {r#"
        x <- 1
        function() { # missing function body
            x <- 2
            x <- 3
        x <- 3
    "#});

    assert!(matches!(
        result,
        Err(FormatError::Missing {
            kind: "}",
            line: 5,
            col: 0
        })
    ));

    let result = format_str(indoc! {r#"
        foo(
    "#});
    assert!(matches!(
        result,
        Err(FormatError::Missing {
            kind: ")",
            line: 0,
            col: 4
        })
    ));
}

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test

// Migrated to base.R.test and special.R.test
