use {
    ropey::Rope,
    std::borrow::Cow,
    tower_lsp::lsp_types::{Position, Range},
};

pub fn position_to_index(position: Position, rope: &Rope) -> Result<usize, ropey::Error> {
    let line = position.line as usize;
    let line = rope.try_line_to_char(line)?;
    Ok(line + position.character as usize)
}

pub fn index_to_position(index: usize, rope: &Rope) -> Result<Position, ropey::Error> {
    let line = rope.try_char_to_line(index)?;
    let char = index - rope.line_to_char(line);
    Ok(Position {
        line: line as u32,
        character: char as u32,
    })
}

pub fn lsp_range_to_rope_range(
    range: Range,
    rope: &Rope,
) -> Result<std::ops::Range<usize>, ropey::Error> {
    let start = position_to_index(range.start, rope)?;
    let end = position_to_index(range.end, rope)?;
    Ok(start..end)
}

pub fn rope_range_to_lsp_range(
    range: std::ops::Range<usize>,
    rope: &Rope,
) -> Result<Range, ropey::Error> {
    let start = index_to_position(range.start, rope)?;
    let end = index_to_position(range.end, rope)?;
    Ok(Range { start, end })
}

// based on https://docs.rs/indent/latest/src/indent/lib.rs.html#27-32
pub fn indent_by<'a, S>(number_of_spaces: usize, input: S) -> String
where
    S: Into<Cow<'a, str>>,
{
    indent(" ".repeat(number_of_spaces), input)
}

fn indent<'a, S, T>(prefix: S, input: T) -> String
where
    S: Into<Cow<'a, str>>,
    T: Into<Cow<'a, str>>,
{
    let prefix = prefix.into();
    let input = input.into();
    let length = input.len();
    let mut output = String::with_capacity(length + length / 2);

    for (i, line) in input.lines().enumerate() {
        if i > 0 {
            output.push('\n');
        }

        if !line.is_empty() {
            output.push_str(&prefix);
        }

        output.push_str(line);
    }

    if input.ends_with('\n') {
        output.push('\n');
    }

    output
}
