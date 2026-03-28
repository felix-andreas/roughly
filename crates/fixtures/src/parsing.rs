use {serde_json::from_str, std::path::PathBuf, thiserror::Error};

#[derive(Debug)]
pub struct FixtureFile {
    pub groups: Vec<FixtureGroup>,
}

#[derive(Debug)]
pub struct FixtureGroup {
    pub name: String,
    pub cases: Vec<Fixture>,
}

#[derive(Debug)]
pub struct Fixture {
    pub name: String,
    pub kind: FixtureKind,
}

#[derive(Debug)]
pub enum FixtureKind {
    Simple(Simple),
    MultiFile(MultiFile),
    Generational(Generational),
}

#[derive(Debug)]
pub struct Simple {
    pub input: String,
    pub expected: String,
}

#[derive(Debug)]
pub struct MultiFile {
    pub input_files: Vec<FixtureInputFile>,
    pub expectations: Vec<FixtureOutputExpectation>,
}

#[derive(Debug)]
pub struct Generational {
    pub generations: Vec<FixtureGeneration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureInputFile {
    pub path: PathBuf,
    pub contents: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureOutputExpectation {
    pub path: PathBuf,
    pub expected: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureGeneration {
    pub name: String,
    pub operations: Vec<FixtureOperation>,
    pub expectations: Vec<FixtureExpectation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixturePath {
    Main,
    Path(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixtureOperation {
    CreateDocument {
        path: FixturePath,
        contents: String,
    },
    EditDocument {
        path: FixturePath,
        range: FixtureRange,
        replacement_text: String,
    },
    MoveDocument {
        source_path: FixturePath,
        destination_path: FixturePath,
    },
    DeleteDocument {
        path: FixturePath,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixtureExpectation {
    Set { path: FixturePath, expected: String },
    Clear { path: FixturePath },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixturePosition {
    pub line_number: usize,
    pub column_number: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixtureRange {
    pub start: FixturePosition,
    pub end: FixturePosition,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseFixtureError {
    #[error("{message}")]
    InvalidFixture { message: String },
}

pub fn parse_file(text: &str) -> Result<FixtureFile, ParseFixtureError> {
    let groups = text
        .split("#====")
        .filter_map(|block| {
            if block.trim().is_empty() {
                return None;
            }

            Some(parse_group(block))
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(FixtureFile { groups })
}

fn parse_group(block: &str) -> Result<FixtureGroup, ParseFixtureError> {
    let (name, cases) = block.split_once('\n').ok_or_else(|| {
        invalid_fixture_error("each test group must have a name line followed by content")
    })?;

    Ok(FixtureGroup {
        name: name.trim().to_owned(),
        cases: parse_group_cases(cases)?,
    })
}

fn parse_group_cases(text: &str) -> Result<Vec<Fixture>, ParseFixtureError> {
    collect_case_blocks(text)?
        .into_iter()
        .map(|(name, body)| {
            let kind = if starts_with_directive_line(&body, "#....") {
                FixtureKind::Generational(parse_generational_multi_file_case(&body)?)
            } else if starts_with_directive_line(&body, "#----") {
                FixtureKind::MultiFile(parse_multi_file_case(&body)?)
            } else {
                FixtureKind::Simple(parse_simple_case(&body)?)
            };

            Ok(Fixture { name, kind })
        })
        .collect()
}

fn collect_case_blocks(text: &str) -> Result<Vec<(String, String)>, ParseFixtureError> {
    let lines = text.split_inclusive('\n').collect::<Vec<_>>();
    let mut line_index = 0;
    let mut cases = Vec::new();

    while line_index < lines.len() {
        skip_blank_lines(&lines, &mut line_index);

        if line_index >= lines.len() {
            break;
        }

        let case_name = parse_named_directive(&lines, &mut line_index, "#----", "test case")?;
        let body_start = line_index;
        let mut case_body_kind = CaseBodyKind::Undecided;
        let mut inside_expectations = false;

        while line_index < lines.len() {
            let line = lines[line_index];

            if starts_with_directive(line, "#....") {
                case_body_kind = CaseBodyKind::GenerationalMultiFile;
                inside_expectations = false;
                line_index += 1;
                continue;
            }

            if starts_with_directive(line, "#++++") {
                inside_expectations = true;
                line_index += 1;
                continue;
            }

            if starts_with_directive(line, "#----") {
                match case_body_kind {
                    CaseBodyKind::Undecided if inside_expectations => break,
                    CaseBodyKind::Undecided => {
                        case_body_kind = CaseBodyKind::MultiFile;
                        line_index += 1;
                        continue;
                    }
                    CaseBodyKind::MultiFile | CaseBodyKind::GenerationalMultiFile
                        if !inside_expectations =>
                    {
                        line_index += 1;
                        continue;
                    }
                    CaseBodyKind::MultiFile | CaseBodyKind::GenerationalMultiFile => break,
                }
            }

            line_index += 1;
        }

        cases.push((case_name, lines[body_start..line_index].concat()));
    }

    Ok(cases)
}

fn parse_simple_case(text: &str) -> Result<Simple, ParseFixtureError> {
    if text
        .lines()
        .any(|line| trim_line(line).starts_with("#++++ "))
    {
        return Err(invalid_fixture_error(
            "simple fixture cases must use a bare `#++++` separator with no path",
        ));
    }

    let (input, expected_block) = text.split_once("#++++\n").ok_or_else(|| {
        invalid_fixture_error("simple fixture cases must include a `#++++` separator")
    })?;

    Ok(Simple {
        input: input.to_owned(),
        expected: expected_block.trim_end().to_owned(),
    })
}

fn parse_multi_file_case(text: &str) -> Result<MultiFile, ParseFixtureError> {
    let lines = text.split_inclusive('\n').collect::<Vec<_>>();
    let mut line_index = 0;
    let input_files = parse_multi_file_inputs(&lines, &mut line_index)?;
    let expectations = parse_multi_file_expectations(&lines, &mut line_index)?;

    if input_files.is_empty() {
        return Err(invalid_fixture_error(
            "multi-file fixture cases must include at least one input file",
        ));
    }

    if expectations.is_empty() {
        return Err(invalid_fixture_error(
            "multi-file fixture cases must include at least one per-file expectation",
        ));
    }

    Ok(MultiFile {
        input_files,
        expectations,
    })
}

fn parse_multi_file_inputs(
    lines: &[&str],
    line_index: &mut usize,
) -> Result<Vec<FixtureInputFile>, ParseFixtureError> {
    let mut input_files = Vec::new();

    loop {
        skip_blank_lines(lines, line_index);

        if *line_index >= lines.len() || starts_with_directive(lines[*line_index], "#++++") {
            break;
        }

        let path = parse_named_directive(lines, line_index, "#----", "input file")?;
        if path.starts_with("edit ") || path.starts_with("move ") || path.starts_with("delete ") {
            return Err(invalid_fixture_error(
                "multi-file fixture cases only allow whole-file inputs",
            ));
        }

        input_files.push(FixtureInputFile {
            path: parse_required_path(&path, "input file path")?,
            contents: collect_body_until_directive(lines, line_index),
        });
    }

    Ok(input_files)
}

fn parse_multi_file_expectations(
    lines: &[&str],
    line_index: &mut usize,
) -> Result<Vec<FixtureOutputExpectation>, ParseFixtureError> {
    let mut expectations = Vec::new();

    loop {
        skip_blank_lines(lines, line_index);

        if *line_index >= lines.len() {
            break;
        }

        let path = parse_named_directive(lines, line_index, "#++++", "output expectation")?;
        if path.starts_with("none ") {
            return Err(invalid_fixture_error(
                "multi-file fixture cases do not allow explicit expectation clearing",
            ));
        }

        expectations.push(FixtureOutputExpectation {
            path: parse_required_path(&path, "expectation path")?,
            expected: collect_body_until_directive(lines, line_index)
                .trim_end()
                .to_owned(),
        });
    }

    Ok(expectations)
}

fn parse_generational_multi_file_case(text: &str) -> Result<Generational, ParseFixtureError> {
    let generations = text
        .split("#....")
        .filter_map(|block| {
            if block.trim().is_empty() {
                return None;
            }

            Some(parse_generation_block(block))
        })
        .collect::<Result<Vec<_>, _>>()?;

    if generations.is_empty() {
        return Err(invalid_fixture_error(
            "generation-based multi-file cases must include at least one `#....` generation block",
        ));
    }

    Ok(Generational { generations })
}

fn parse_generation_block(block: &str) -> Result<FixtureGeneration, ParseFixtureError> {
    let (name, body) = block.split_once('\n').ok_or_else(|| {
        invalid_fixture_error("each fixture generation must have a name line followed by content")
    })?;
    let lines = body.split_inclusive('\n').collect::<Vec<_>>();
    let mut line_index = 0;

    Ok(FixtureGeneration {
        name: name.trim().to_owned(),
        operations: parse_generation_operations(&lines, &mut line_index)?,
        expectations: parse_generation_expectations(&lines, &mut line_index)?,
    })
}

fn parse_generation_operations(
    lines: &[&str],
    line_index: &mut usize,
) -> Result<Vec<FixtureOperation>, ParseFixtureError> {
    let mut operations = Vec::new();

    loop {
        skip_blank_lines(lines, line_index);

        if *line_index >= lines.len() || starts_with_directive(lines[*line_index], "#++++") {
            break;
        }

        let header = parse_named_directive(lines, line_index, "#----", "generation entry")?;
        operations.push(parse_generation_operation(lines, line_index, &header)?);
    }

    Ok(operations)
}

fn parse_generation_operation(
    lines: &[&str],
    line_index: &mut usize,
    header: &str,
) -> Result<FixtureOperation, ParseFixtureError> {
    if let Some(body) = header.strip_prefix("edit ") {
        return parse_edit_operation(body);
    }

    if let Some(body) = header.strip_prefix("move ") {
        let (source_path, destination_path) = body.split_once(" -> ").ok_or_else(|| {
            invalid_fixture_error("move operations must use `#---- move source -> destination`")
        })?;

        return Ok(FixtureOperation::MoveDocument {
            source_path: parse_fixture_path(source_path.trim()),
            destination_path: parse_fixture_path(destination_path.trim()),
        });
    }

    if let Some(path) = header.strip_prefix("delete ") {
        return Ok(FixtureOperation::DeleteDocument {
            path: parse_fixture_path(path.trim()),
        });
    }

    Ok(FixtureOperation::CreateDocument {
        path: parse_fixture_path(header.trim()),
        contents: collect_body_until_directive(lines, line_index),
    })
}

fn parse_generation_expectations(
    lines: &[&str],
    line_index: &mut usize,
) -> Result<Vec<FixtureExpectation>, ParseFixtureError> {
    let mut expectations = Vec::new();

    loop {
        skip_blank_lines(lines, line_index);

        if *line_index >= lines.len() {
            break;
        }

        let header = parse_named_directive(lines, line_index, "#++++", "generation expectation")?;

        if let Some(path) = header.strip_prefix("none ") {
            expectations.push(FixtureExpectation::Clear {
                path: parse_fixture_path(path.trim()),
            });
            continue;
        }

        expectations.push(FixtureExpectation::Set {
            path: parse_fixture_path(&header),
            expected: collect_body_until_directive(lines, line_index)
                .trim_end()
                .to_owned(),
        });
    }

    Ok(expectations)
}

fn parse_edit_operation(text: &str) -> Result<FixtureOperation, ParseFixtureError> {
    let (path_and_range, replacement_literal) = text.split_once(" -> ").ok_or_else(|| {
        invalid_fixture_error(
            "edit operations must use `#---- edit path start:end-start:end -> \"text\"`",
        )
    })?;
    let replacement_text = from_str::<String>(replacement_literal.trim()).map_err(|error| {
        invalid_fixture_error(&format!(
            "edit replacement text must be a valid JSON string literal: {error}"
        ))
    })?;

    let split_index = path_and_range.rfind(' ').ok_or_else(|| {
        invalid_fixture_error("edit operations must include a path and a range before `->`")
    })?;
    let path = path_and_range[..split_index].trim();
    let range_text = path_and_range[split_index + 1..].trim();

    Ok(FixtureOperation::EditDocument {
        path: parse_fixture_path(path),
        range: parse_range(range_text)?,
        replacement_text,
    })
}

fn parse_range(text: &str) -> Result<FixtureRange, ParseFixtureError> {
    let (start_text, end_text) = text.split_once('-').ok_or_else(|| {
        invalid_fixture_error("ranges must use `start_line:start_column-end_line:end_column`")
    })?;

    Ok(FixtureRange {
        start: parse_position(start_text)?,
        end: parse_position(end_text)?,
    })
}

fn parse_position(text: &str) -> Result<FixturePosition, ParseFixtureError> {
    let (line_number, column_number) = text
        .split_once(':')
        .ok_or_else(|| invalid_fixture_error("positions must use `line:column`"))?;

    Ok(FixturePosition {
        line_number: parse_positive_number(line_number, "line number")?,
        column_number: parse_positive_number(column_number, "column number")?,
    })
}

fn parse_positive_number(text: &str, label: &str) -> Result<usize, ParseFixtureError> {
    let value = text
        .trim()
        .parse::<usize>()
        .map_err(|error| invalid_fixture_error(&format!("invalid {label} `{text}`: {error}")))?;

    if value == 0 {
        return Err(invalid_fixture_error(&format!(
            "{label} values are 1-based"
        )));
    }

    Ok(value)
}

fn parse_named_directive(
    lines: &[&str],
    line_index: &mut usize,
    directive: &str,
    label: &str,
) -> Result<String, ParseFixtureError> {
    let line = lines
        .get(*line_index)
        .ok_or_else(|| invalid_fixture_error(&format!("expected {label}")))?;
    let content = directive_content(line, directive)
        .ok_or_else(|| invalid_fixture_error(&format!("expected {label}")))?;
    *line_index += 1;
    Ok(content.trim().to_owned())
}

fn parse_required_path(text: &str, label: &str) -> Result<PathBuf, ParseFixtureError> {
    let trimmed_text = text.trim();

    if trimmed_text.is_empty() {
        return Err(invalid_fixture_error(&format!("{label} must not be empty")));
    }

    Ok(PathBuf::from(trimmed_text))
}

fn parse_fixture_path(text: &str) -> FixturePath {
    let trimmed_text = text.trim();

    if trimmed_text.is_empty() {
        FixturePath::Main
    } else {
        FixturePath::Path(PathBuf::from(trimmed_text))
    }
}

fn collect_body_until_directive(lines: &[&str], line_index: &mut usize) -> String {
    let mut body = String::new();

    while *line_index < lines.len() && !is_directive_line(lines[*line_index]) {
        body.push_str(lines[*line_index]);
        *line_index += 1;
    }

    body
}

fn skip_blank_lines(lines: &[&str], line_index: &mut usize) {
    while *line_index < lines.len() && trim_line(lines[*line_index]).is_empty() {
        *line_index += 1;
    }
}

fn starts_with_directive_line(text: &str, directive: &str) -> bool {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| starts_with_directive(line, directive))
}

fn starts_with_directive(line: &str, directive: &str) -> bool {
    directive_content(line, directive).is_some()
}

fn directive_content<'a>(line: &'a str, directive: &str) -> Option<&'a str> {
    let trimmed_line = trim_line(line);
    let remainder = trimmed_line.strip_prefix(directive)?;

    if remainder.is_empty() {
        return Some("");
    }

    remainder
        .strip_prefix(' ')
        .or_else(|| directive.ends_with("++++").then_some(remainder))
}

fn is_directive_line(line: &str) -> bool {
    ["#====", "#----", "#....", "#++++"]
        .iter()
        .any(|directive| starts_with_directive(line, directive))
}

fn trim_line(line: &str) -> &str {
    line.strip_suffix('\n')
        .unwrap_or(line)
        .trim_end_matches('\r')
}

fn invalid_fixture_error(message: &str) -> ParseFixtureError {
    ParseFixtureError::InvalidFixture {
        message: message.to_owned(),
    }
}

enum CaseBodyKind {
    Undecided,
    MultiFile,
    GenerationalMultiFile,
}
