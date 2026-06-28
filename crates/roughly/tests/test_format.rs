use {
    fixtures::{Fixture, FixtureKind, FixtureRunFile, run_fixture_suite},
    indoc::indoc,
    regex::{Captures, Regex},
    ropey::Rope,
    roughly::{
        format::{Config, FormatError, LineEnding, format},
        tree,
    },
    std::{fs, path::PathBuf},
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
        Ok(formatted) => formatted,
        Err(error) => format!("error: {error:?}"),
    };

    Ok(vec![vec![FixtureRunFile {
        path: PathBuf::new(),
        output,
    }]])
}

#[test]
fn documentation_examples() {
    // Formats every code example in the formatter documentation template and renders the result into
    // `docs/src/content/docs/formatter.md`. The generated docs page is the golden output: run with
    // `ROUGHLY_BLESS=1` to rewrite it, otherwise the test fails when the page drifts from the
    // formatter's current behavior.
    let markdown = fs::read_to_string("tests/format/formatter.template.md").unwrap();

    let regex = Regex::new(r#"(?m)```r\n([\s\S]*?)```"#).unwrap();
    const BEFORE: &str =
        "R CODE IN THIS FILE IS FORMATTED AND SAVED TO docs/src/content/docs/formatter.md";
    const AFTER: &str = "THIS FILE IS GENERATED AUTOMATICALLY.\
MAKE CHANGES TO tests/format/formatter.template.md INSTEAD";

    let formatted = regex
        .replace_all(&markdown, |captures: &Captures| {
            let text = captures.get(1).unwrap().as_str();
            text.split_once('\n')
                .and_then(|(first, rest)| first.strip_prefix("#").map(|comment| (comment, rest)))
                .and_then(|(comment, text)| {
                    comment
                        .split_once(":")
                        .map(|(_name, directive)| (directive.trim(), text))
                })
                .map(|(directive, text)| {
                    if directive == "skip" {
                        return text.into();
                    }

                    let code = format_str(text).unwrap();
                    match directive {
                        "compare" => {
                            format!("# Before formatting\n{text}\n# After formatting\n{code}")
                        }
                        "format" => code,
                        _ => panic!(),
                    }
                })
                .map(|content| format!("```r\n{content}```"))
                .unwrap_or(text.into())
        })
        .replace(BEFORE, AFTER);

    let path = "../../docs/src/content/docs/formatter.md";
    if matches!(std::env::var("ROUGHLY_BLESS").as_deref(), Ok("1")) {
        // Note: we cannot write the file during a nix build, where the source tree is read-only.
        if fs::metadata(path).is_ok() {
            fs::write(path, &formatted).unwrap();
        }
        return;
    }

    // When the generated page is available, assert it matches; under a sandboxed build where the docs
    // tree is absent there is nothing to compare against.
    if let Ok(existing) = fs::read_to_string(path) {
        assert_eq!(
            existing, formatted,
            "docs/src/content/docs/formatter.md is out of date; rerun with ROUGHLY_BLESS=1"
        );
    }
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
