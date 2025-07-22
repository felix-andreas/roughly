use {
    indoc::indoc,
    regex::{Captures, Regex},
    ropey::Rope,
    roughly::{
        format::{Config, FormatError, LineEnding, format},
        tree,
    },
    std::fs,
};

#[test]
fn base() {
    const BASE_TESTS: &str = include_str!("format/base.R.test");
    run_test_groups(&parse_test_file(BASE_TESTS));
}

#[test]
fn special() {
    const SPECIAL_TESTS: &str = include_str!("format/special.R.test");
    run_test_groups(&parse_test_file(SPECIAL_TESTS));
}

#[test]
fn misc() {
    const MISC_TESTS: &str = include_str!("format/misc.R.test");
    run_test_groups(&parse_test_file(MISC_TESTS));
}

#[test]
fn docs() {
    // This tests all code examples in the formatter documentation.
    // It updates the formatted output as needed to keep the
    // documentation consistent with the formatter's behavior.
    let markdown = fs::read_to_string("tests/format/formatter.template.md").unwrap();

    let regex = Regex::new(r#"(?m)```r\n([\s\S]*?)```"#).unwrap();

    let result = regex.replace_all(&markdown, |captures: &Captures| {
        let text = captures.get(1).unwrap().as_str();
        text.split_once('\n')
            .and_then(|(first, rest)| first.strip_prefix("#").map(|comment| (comment, rest)))
            .and_then(|(comment, text)| {
                comment
                    .split_once(":")
                    .map(|(name, directive)| ((name.trim(), directive.trim()), text))
            })
            .map(|((name, directive), text)| {
                let snapshot = format!("documentation_examples__{name}");
                let code = format_str(text).unwrap();
                insta::assert_snapshot!(snapshot, code);

                match dbg!(directive) {
                    "compare" => format!("# Before formatting\n{text}\n# After formatting {code}"),
                    "format" => code,
                    _ => panic!(),
                }
            })
            .unwrap_or(text.into())
    });

    dbg!("before");
    fs::write("../../docs/content/formatter2.md", result.to_string()).unwrap();
    dbg!("after");
}

#[test]
fn trailing_spaces_in_comments() {
    assert_eq!(format_str("#' \n#' comment  ").unwrap(), "#'\n#' comment\n");
}

#[test]
fn line_formatting() {
    assert_eq!(format_str("x \n y \n y \n").unwrap(), "x\ny\ny\n");
    assert_eq!(format_str("x\ny\r\ny\r\n").unwrap(), "x\ny\ny\n");
    assert_eq!(format_str("x\r\ny\r\ny\r\n").unwrap(), "x\r\ny\r\ny\r\n");
    assert_eq!(format_str("x\r\ny\ny\n").unwrap(), "x\r\ny\r\ny\r\n");
}

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
        call(
    "#});
    assert!(matches!(
        result,
        Err(FormatError::Missing {
            kind: ")",
            line: 0,
            col: 5
        })
    ));
}

//
// UTILS
//

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
