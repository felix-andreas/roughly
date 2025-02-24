use {
    ropey::Rope,
    std::borrow::Cow,
    tower_lsp::lsp_types::{Position, Range},
    tree_sitter::{Node, TreeCursor},
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
pub fn indent_by<'a, S>(number_of_spaces: usize, input: S, line_ending: &str) -> String
where
    S: Into<Cow<'a, str>>,
{
    indent(" ".repeat(number_of_spaces), input, false, line_ending)
}

pub fn indent_by_with_newlines<'a, S>(
    number_of_spaces: usize,
    input: S,
    line_ending: &str,
) -> String
where
    S: Into<Cow<'a, str>>,
{
    indent(" ".repeat(number_of_spaces), input, true, line_ending)
}

fn indent<'a, S, T>(prefix: S, input: T, newlines: bool, line_ending: &str) -> String
where
    S: Into<Cow<'a, str>>,
    T: Into<Cow<'a, str>>,
{
    let prefix = prefix.into();
    let input = input.into();
    let length = input.len();
    let mut output = String::with_capacity(length + length / 2);

    for (i, line) in input.lines().enumerate() {
        if i > 0 || newlines {
            output.push_str(line_ending);
        }

        if !line.is_empty() {
            output.push_str(&prefix);
        }

        output.push_str(line);
    }

    if input.ends_with(line_ending) || newlines {
        output.push_str(line_ending);
    }

    output
}

pub fn add_indent_prefix(input: &str) -> String {
    let mut output = String::with_capacity(input.len() + 5);
    for char in input.chars() {
        output.push(char);
        if char == '\n' {
            output.push('\x02')
        }
    }
    output
}

pub fn remove_indent_prefix(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut tmp = String::with_capacity(256);
    let mut push_to_temp = true;
    for char in input.chars() {
        if char == '\x02' {
            push_to_temp = false;
            continue;
        }

        if push_to_temp {
            tmp.push(char);
        } else {
            output.push(char);
        }

        if char == '\n' {
            if push_to_temp {
                output.push_str(&tmp);
            }
            tmp.clear();
            push_to_temp = true;
        }
    }
    output.push_str(&tmp);

    output
}

pub fn format_node(node: Node) -> String {
    fn traverse(cursor: &mut TreeCursor, output: &mut String) {
        let indent = "  ".repeat(cursor.depth() as usize);
        let node = cursor.node();
        if node.child_count() > 0 {
            output.push('(');
        }

        output.push_str(node.kind());
        if node.is_missing() {
            output.push_str(" MISSING");
        } else if node.is_error() && node.kind() != "ERROR" {
            output.push_str(" ERROR");
        }

        if cursor.goto_first_child() {
            loop {
                output.push('\n');
                output.push_str(&indent);
                output.push_str("  ");

                if let Some(field_name) = cursor.field_name() {
                    output.push_str(field_name);
                    output.push_str(": ");
                }

                traverse(cursor, output);

                if !cursor.goto_next_sibling() {
                    break;
                }
            }

            cursor.goto_parent();

            output.push('\n');
            output.push_str(&indent);
            output.push(')');
        }
    }

    let mut result = String::new();
    let mut cursor = node.walk();
    traverse(&mut cursor, &mut result);
    result
}

#[cfg(test)]
mod test {
    use crate::utils::{add_indent_prefix, remove_indent_prefix};

    #[test]
    fn test_add_indent_prefix() {
        assert_eq!(add_indent_prefix("foo\nbar\nbaz"), "foo\n\x02bar\n\x02baz");
        assert_eq!(
            add_indent_prefix("foo\nbar\nbaz\n"),
            "foo\n\x02bar\n\x02baz\n\x02"
        );
    }

    #[test]
    fn test_removeindent_prefix() {
        assert_eq!(
            remove_indent_prefix("foo\n  \x02  bar\n  \x02  baz\n\x02"),
            "foo\n  bar\n  baz\n"
        );
    }
}
