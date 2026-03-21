use {
    ropey::Rope,
    std::{fmt::Write, ops::Range},
    tree_sitter::{Node, Point, Range as TreeSitterRange},
};

pub struct AnnotationBlock {
    pub text: String,
    pub range: TreeSitterRange,
    pub last_row: usize,
}

pub fn point_to_char(rope: &Rope, point: Point) -> Option<usize> {
    let line_index = point.row;
    let line_start = rope.try_line_to_char(line_index).ok()?;
    Some(line_start + point.column)
}

pub fn point_to_byte(rope: &Rope, point: Point) -> Option<usize> {
    let character_index = point_to_char(rope, point)?;
    rope.try_char_to_byte(character_index).ok()
}

pub fn line_text(rope: &Rope, line_index: usize) -> Option<String> {
    let line = rope.get_line(line_index)?;
    Some(line.to_string())
}

pub fn line_count(rope: &Rope) -> usize {
    rope.len_lines()
}

pub fn first_line_text(rope: &Rope) -> String {
    line_text(rope, 0).unwrap_or_default()
}

pub fn node_text(rope: &Rope, node: Node<'_>) -> Option<String> {
    byte_range_text(rope, node.start_byte()..node.end_byte())
}

pub fn byte_range_text(rope: &Rope, byte_range: Range<usize>) -> Option<String> {
    if byte_range.start > byte_range.end {
        return None;
    }

    let start_character = rope.try_byte_to_char(byte_range.start).ok()?;
    let end_character = rope.try_byte_to_char(byte_range.end).ok()?;
    Some(rope.slice(start_character..end_character).to_string())
}

pub fn compact_node_text(rope: &Rope, node: Node<'_>) -> String {
    let Some(text) = node_text(rope, node) else {
        return "<unavailable>".to_owned();
    };

    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        "<empty>".to_owned()
    } else {
        compact
    }
}

pub fn annotation_block(rope: &Rope, start_row: usize) -> Option<AnnotationBlock> {
    let first_line = line_text(rope, start_row)?;
    let first_trimmed_line = first_line.trim_start();
    let first_annotation_text = first_trimmed_line.strip_prefix("#:")?;

    let start_column = first_line.len() - first_trimmed_line.len();
    let mut row = start_row;
    let mut block_lines = vec![first_annotation_text.trim().to_owned()];

    while row + 1 < line_count(rope) {
        let Some(line) = line_text(rope, row + 1) else {
            break;
        };
        let trimmed_line = line.trim_start();
        let Some(annotation_text) = trimmed_line.strip_prefix("#:") else {
            break;
        };
        block_lines.push(annotation_text.trim().to_owned());
        row += 1;
    }

    let end_line = line_text(rope, row).unwrap_or_default();
    let end_trimmed_line = end_line.trim_end_matches(['\r', '\n']);
    let end_column = end_trimmed_line.len();

    Some(AnnotationBlock {
        text: block_lines.join("\n"),
        range: TreeSitterRange {
            start_byte: 0,
            end_byte: 0,
            start_point: Point {
                row: start_row,
                column: start_column,
            },
            end_point: Point {
                row,
                column: end_column,
            },
        },
        last_row: row,
    })
}

pub fn all_lines(rope: &Rope) -> Vec<String> {
    (0..line_count(rope))
        .filter_map(|line_index| line_text(rope, line_index))
        .collect()
}

pub fn point_label(point: Point) -> String {
    let mut label = String::new();
    let _ = write!(&mut label, "{}:{}", point.row + 1, point.column + 1);
    label
}
