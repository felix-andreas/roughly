use {
    serde_json::from_str,
    std::{ops::Range, path::PathBuf},
    thiserror::Error,
};

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
    Simple(SimpleFixture),
    MultiFile(MultiFileFixture),
}

#[derive(Debug)]
pub struct SimpleFixture {
    pub input: String,
    pub expected: String,
}

#[derive(Debug)]
pub struct MultiFileFixture {
    pub initial_generation: InitialFixtureGeneration,
    pub generations: Vec<FixtureGeneration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitialFixtureGeneration {
    pub name: String,
    pub entries: Vec<FixtureGenerationEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureGeneration {
    pub name: String,
    pub entries: Vec<FixtureGenerationEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureGenerationEntry {
    pub operation: FixtureOperation,
    pub expectation: Option<FixtureGenerationExpectation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureGenerationExpectation {
    pub path: PathBuf,
    pub output: FixtureExpectedOutput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixtureExpectedOutput {
    Exact(String),
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixtureOperation {
    CreateDocument {
        path: PathBuf,
        contents: String,
    },
    EditDocument {
        path: PathBuf,
        range: FixtureRange,
        replacement_text: String,
    },
    MoveDocument {
        source_path: PathBuf,
        destination_path: PathBuf,
    },
    DeleteDocument {
        path: PathBuf,
    },
    Action {
        action: String,
        path: PathBuf,
        contents: String,
    },
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

// Byte ranges, into the source `.test` text, of every `#++++` expectation block's content body
// (the bytes a human edits between a `#++++` directive line and the next directive line or EOF).
// These are captured only on the bless path; they are deliberately kept out of the parsed fixture
// structs so normal runs and their equality semantics stay unchanged. Every `#++++` directive in a
// valid fixture file maps to exactly one parsed expectation, so the ranges returned here line up
// one-to-one, in file order, with the `Some` expectations enumerated from `parse_file`.
pub fn expectation_content_spans(text: &str) -> Vec<Range<usize>> {
    let lines = text.split_inclusive('\n').collect::<Vec<_>>();
    let mut line_offsets = Vec::with_capacity(lines.len() + 1);
    let mut running_offset = 0;
    for line in &lines {
        line_offsets.push(running_offset);
        running_offset += line.len();
    }
    line_offsets.push(running_offset);

    let mut spans = Vec::new();
    let mut line_index = 0;
    while line_index < lines.len() {
        if !starts_with_directive(lines[line_index], "#++++") {
            line_index += 1;
            continue;
        }

        let body_start = line_offsets[line_index + 1];
        let mut body_end_line = line_index + 1;
        while body_end_line < lines.len() && !is_directive_line(lines[body_end_line]) {
            body_end_line += 1;
        }
        spans.push(body_start..line_offsets[body_end_line]);
        line_index = body_end_line;
    }

    spans
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
            let kind = if starts_with_directive_line(&body, "#----")
                || starts_with_directive_line(&body, "#....")
                || starts_with_directive_line(&body, "#!!!!")
            {
                FixtureKind::MultiFile(parse_multi_file_case(&name, &body)?)
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

        while line_index < lines.len() {
            if is_case_header_line(&lines, line_index) {
                break;
            }

            line_index += 1;
        }

        cases.push((case_name, lines[body_start..line_index].concat()));
    }

    Ok(cases)
}

fn parse_simple_case(text: &str) -> Result<SimpleFixture, ParseFixtureError> {
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

    Ok(SimpleFixture {
        input: input.to_owned(),
        expected: expected_block.trim_end().to_owned(),
    })
}

fn parse_multi_file_case(
    fixture_name: &str,
    text: &str,
) -> Result<MultiFileFixture, ParseFixtureError> {
    let lines = text.split_inclusive('\n').collect::<Vec<_>>();
    let mut line_index = 0;
    let (initial_generation_name, entries) =
        if line_index < lines.len() && starts_with_directive(lines[line_index], "#....") {
            let initial_generation_name =
                parse_named_directive(&lines, &mut line_index, "#....", "initial generation")?;
            (
                initial_generation_name,
                parse_generation_entries(&lines, &mut line_index)?,
            )
        } else {
            (
                fixture_name.to_owned(),
                parse_generation_entries(&lines, &mut line_index)?,
            )
        };

    validate_initial_generation_entries(&entries)?;

    if !entries
        .iter()
        .any(|entry| matches!(entry.operation, FixtureOperation::CreateDocument { .. }))
    {
        return Err(invalid_fixture_error(
            "multi-file fixture cases must include at least one input file",
        ));
    }

    Ok(MultiFileFixture {
        initial_generation: InitialFixtureGeneration {
            name: initial_generation_name,
            entries,
        },
        generations: parse_remaining_generations(&lines, &mut line_index)?,
    })
}

fn parse_remaining_generations(
    lines: &[&str],
    line_index: &mut usize,
) -> Result<Vec<FixtureGeneration>, ParseFixtureError> {
    let mut generations = Vec::new();

    loop {
        skip_blank_lines(lines, line_index);

        if *line_index >= lines.len() {
            break;
        }

        if !starts_with_directive(lines[*line_index], "#....") {
            return Err(invalid_fixture_error("expected generation header"));
        }

        let generation_name =
            parse_named_directive(lines, line_index, "#....", "fixture generation")?;
        let entries = parse_generation_entries(lines, line_index)?;
        generations.push(FixtureGeneration {
            name: generation_name,
            entries,
        });
    }

    Ok(generations)
}

fn validate_initial_generation_entries(
    entries: &[FixtureGenerationEntry],
) -> Result<(), ParseFixtureError> {
    for entry in entries {
        if entry.expectation.is_none() {
            return Err(invalid_fixture_error(
                "initial multi-file generation entries must have an output expectation",
            ));
        }

        match &entry.operation {
            FixtureOperation::CreateDocument { .. } | FixtureOperation::Action { .. } => {}
            FixtureOperation::EditDocument { .. }
            | FixtureOperation::MoveDocument { .. }
            | FixtureOperation::DeleteDocument { .. } => {
                return Err(invalid_fixture_error(
                    "initial multi-file generation may only create whole documents or actions",
                ));
            }
        }
    }

    Ok(())
}

fn parse_generation_entries(
    lines: &[&str],
    line_index: &mut usize,
) -> Result<Vec<FixtureGenerationEntry>, ParseFixtureError> {
    let mut entries = Vec::new();

    loop {
        skip_blank_lines(lines, line_index);

        if *line_index >= lines.len() {
            break;
        }

        if starts_with_directive(lines[*line_index], "#....") {
            break;
        }

        let operation = if starts_with_directive(lines[*line_index], "#----") {
            let header = parse_named_directive(lines, line_index, "#----", "generation entry")?;
            parse_generation_operation(lines, line_index, &header)?
        } else if starts_with_directive(lines[*line_index], "#!!!!") {
            let header = parse_named_directive(lines, line_index, "#!!!!", "IDE action")?;
            parse_action_operation(lines, line_index, &header)?
        } else {
            return Err(invalid_fixture_error(
                "expected generation entry or IDE action",
            ));
        };
        let expectation = parse_optional_generation_expectation(
            lines,
            line_index,
            operation_output_path(&operation),
        )?;
        entries.push(FixtureGenerationEntry {
            operation,
            expectation,
        });
    }

    Ok(entries)
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
            source_path: parse_required_path(source_path.trim(), "move source path")?,
            destination_path: parse_required_path(
                destination_path.trim(),
                "move destination path",
            )?,
        });
    }

    if let Some(path) = header.strip_prefix("delete ") {
        return Ok(FixtureOperation::DeleteDocument {
            path: parse_required_path(path.trim(), "delete path")?,
        });
    }

    Ok(FixtureOperation::CreateDocument {
        path: parse_required_path(header.trim(), "document path")?,
        contents: collect_body_until_directive(lines, line_index),
    })
}

fn parse_action_operation(
    lines: &[&str],
    line_index: &mut usize,
    header: &str,
) -> Result<FixtureOperation, ParseFixtureError> {
    let (action, path) = header
        .split_once(' ')
        .ok_or_else(|| invalid_fixture_error("IDE actions must use `#!!!! action path`"))?;
    let path = parse_required_path(path.trim(), "IDE action path")?;
    let contents = collect_body_until_directive(lines, line_index);

    Ok(FixtureOperation::Action {
        action: action.trim().to_owned(),
        path,
        contents,
    })
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
        path: parse_required_path(path, "edit path")?,
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

fn parse_optional_generation_expectation(
    lines: &[&str],
    line_index: &mut usize,
    default_path: Option<&PathBuf>,
) -> Result<Option<FixtureGenerationExpectation>, ParseFixtureError> {
    skip_blank_lines(lines, line_index);

    if *line_index >= lines.len() || !starts_with_directive(lines[*line_index], "#++++") {
        return Ok(None);
    }

    let header = parse_named_directive(lines, line_index, "#++++", "output expectation")?;
    let expected_output = if header == "any" {
        let body = collect_body_until_directive(lines, line_index);
        if !body.trim().is_empty() {
            return Err(invalid_fixture_error(
                "`#++++ any` must not have an expectation body",
            ));
        }
        FixtureExpectedOutput::Any
    } else {
        FixtureExpectedOutput::Exact(
            collect_body_until_directive(lines, line_index)
                .trim_end()
                .to_owned(),
        )
    };

    let path = if header.is_empty() || header == "any" {
        default_path.cloned().ok_or_else(|| {
            invalid_fixture_error(
                "output expectations without a target path require an operation with an output path",
            )
        })?
    } else {
        parse_required_path(&header, "output expectation path")?
    };

    Ok(Some(FixtureGenerationExpectation {
        path,
        output: expected_output,
    }))
}

fn operation_output_path(operation: &FixtureOperation) -> Option<&PathBuf> {
    match operation {
        FixtureOperation::CreateDocument { path, .. } => Some(path),
        FixtureOperation::EditDocument { path, .. } => Some(path),
        FixtureOperation::MoveDocument {
            destination_path, ..
        } => Some(destination_path),
        FixtureOperation::DeleteDocument { .. } => None,
        FixtureOperation::Action { path, .. } => Some(path),
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
    ["#====", "#----", "#....", "#++++", "#!!!!"]
        .iter()
        .any(|directive| starts_with_directive(line, directive))
}

fn trim_line(line: &str) -> &str {
    line.strip_suffix('\n')
        .unwrap_or(line)
        .trim_end_matches('\r')
}

fn is_case_header_line(lines: &[&str], line_index: usize) -> bool {
    starts_with_directive(lines[line_index], "#----")
        && (line_index == 0 || trim_line(lines[line_index - 1]).is_empty())
}

fn invalid_fixture_error(message: &str) -> ParseFixtureError {
    ParseFixtureError::InvalidFixture {
        message: message.to_owned(),
    }
}
