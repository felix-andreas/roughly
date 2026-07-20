use {
    fixtures::{Fixture, FixtureKind, FixtureRunFile, run_fixture_suite},
    indoc::indoc,
    ropey::Rope,
    roughly_legacy::{
        format::{Config, FormatError, LineEnding, format},
        tree,
    },
    std::path::PathBuf,
};

#[test]
fn format_fixtures() {
    run_fixture_suite("tests/format", run_format_fixture);
}

fn run_format_fixture(fixture: &Fixture) -> Result<Vec<Vec<FixtureRunFile>>, String> {
    let FixtureKind::Simple(case) = &fixture.kind else {
        return Err("format fixtures must use the `Simple` shape".to_owned());
    };
    let output = match format_str(&case.input) {
        Ok(formatted) => {
            // Idempotence: re-formatting the formatted output must be a fixed point. This holds the
            // whole suite to `format(format(x)) == format(x)`, which matters most for the
            // continuation indentation of multi-line `#:` annotations.
            match format_str(&formatted) {
                Ok(second) if second != formatted => {
                    return Err(format!(
                        "format is not idempotent\n--- first pass ---\n{formatted}\n--- second pass ---\n{second}"
                    ));
                }
                Ok(_) => formatted,
                Err(error) => return Err(format!("re-format failed: {error:?}")),
            }
        }
        Err(error) => format!("error: {error:?}"),
    };

    Ok(vec![vec![FixtureRunFile {
        path: PathBuf::new(),
        output,
    }]])
}

#[test]
fn annotation_formatting_is_idempotent() {
    // Every annotation fixture's blessed output must be a fixed point: re-formatting the canonical
    // form leaves it byte-identical. Together with the `format_fixtures` suite (which pins
    // format(input) == expected) this proves format(format(input)) == format(input).
    let mut checked = 0;
    for group in fixtures::read_fixture_suite("tests/format") {
        if !group.name.starts_with("annotation") {
            continue;
        }
        for case in group.cases {
            let FixtureKind::Simple(simple) = case.kind else {
                panic!("annotation fixtures must be simple cases");
            };
            let expected = format!("{}\n", simple.expected);
            let reformatted = format_str(&expected).unwrap();
            assert_eq!(
                reformatted, expected,
                "annotation case `{}__{}` is not a formatting fixed point",
                group.name, case.name
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "expected annotation fixtures to be present");
}

#[test]
fn annotation_continuation_honors_indent_width() {
    // A line of a wrapped `#:` annotation indents one `indent_width` step per enclosing expanded
    // bracket, so a non-default width scales the steps (here the hugged `{list{` opens a single
    // expanded level rendered as four spaces, and the glued closers `}}` dedent fully).
    let input = "#: @type Instrument {list{\n#: id: integer\n#: }}\n";
    let formatted = format(
        tree::parse(&mut tree::new_parser(), input, None).root_node(),
        &Rope::from_str(input),
        Config {
            indent_width: 4,
            line_ending: LineEnding::Lf,
        },
    )
    .unwrap();
    assert_eq!(
        formatted,
        "#: @type Instrument {list{\n#:     id: integer\n#: }}\n"
    );
}

#[test]
fn trailing_spaces_in_comments() {
    assert_eq!(format_str("#' \n#' comment  ").unwrap(), "#'\n#' comment\n");
}

#[test]
fn empty_input_stays_empty() {
    // A 0-byte file must format to 0 bytes so `roughly fmt` reports it unchanged; the fixture
    // harness trims trailing whitespace, so the exact bytes are asserted here.
    assert_eq!(format_str("").unwrap(), "");
    // A file that already ends in a bare newline keeps it.
    assert_eq!(format_str("\n").unwrap(), "\n");
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
