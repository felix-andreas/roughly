use crate::{
    interner::Interner,
    types::{AnnotationKind, Atomic, FunctionType, RecordField, SurfaceType},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeParseError {
    InvalidSyntax { message: String },
    UnknownType { name: String },
}

pub fn parse_surface_type(
    interner: &mut Interner,
    text: &str,
) -> Result<SurfaceType, TypeParseError> {
    parse_annotation_type(interner, text, false)
}

pub fn parse_annotation_type(
    interner: &mut Interner,
    text: &str,
    allow_annotation_kind_prefix: bool,
) -> Result<SurfaceType, TypeParseError> {
    let trimmed_text = text.trim();

    if trimmed_text.is_empty() {
        return Err(invalid_syntax("Expected a type, but found empty input."));
    }

    let surface_text = if allow_annotation_kind_prefix {
        annotation_surface_text(trimmed_text)
            .ok_or_else(|| invalid_syntax("Expected a type after the annotation prefix."))?
    } else {
        trimmed_text
    };

    let mut parser = TypeParser::new(interner, surface_text);
    let surface_type = parser.parse_type()?;
    parser.skip_ascii_whitespace();

    if !parser.is_at_end() {
        return Err(invalid_syntax(format!(
            "Unexpected trailing input starting at byte {}.",
            parser.position
        )));
    }

    Ok(surface_type)
}

pub fn parse_annotation(
    interner: &mut Interner,
    text: &str,
) -> Result<crate::types::Annotation, TypeParseError> {
    let trimmed_text = text.trim();

    if trimmed_text.is_empty() {
        return Err(invalid_syntax(
            "Expected a type annotation, but found empty input.",
        ));
    }

    if trimmed_text
        .lines()
        .all(|line| line.trim().is_empty() || line.trim().starts_with("#:"))
    {
        let surface_type = parse_expanded_block_surface_type(interner, trimmed_text)?;
        return Ok(crate::types::Annotation::new(
            AnnotationKind::Checked,
            surface_type,
        ));
    }

    let (kind, surface_text) = if let Some(surface_text) = trimmed_text.strip_prefix('?') {
        (AnnotationKind::UnknownOnly, surface_text.trim())
    } else if let Some(surface_text) = trimmed_text.strip_prefix('!') {
        (AnnotationKind::Trusted, surface_text.trim())
    } else {
        (AnnotationKind::Checked, trimmed_text)
    };

    let surface_type = parse_surface_type(interner, surface_text)?;
    Ok(crate::types::Annotation::new(kind, surface_type))
}

pub fn render_surface_type(interner: &Interner, surface_type: &SurfaceType) -> String {
    let mut renderer = SurfaceTypeRenderer::new(interner);
    renderer.render(surface_type)
}

fn annotation_surface_text(text: &str) -> Option<&str> {
    if let Some(surface_text) = text.strip_prefix('?') {
        Some(surface_text.trim())
    } else if let Some(surface_text) = text.strip_prefix('!') {
        Some(surface_text.trim())
    } else {
        Some(text.trim())
    }
}

pub fn parse_expanded_block_surface_type(
    interner: &mut Interner,
    text: &str,
) -> Result<SurfaceType, TypeParseError> {
    let trimmed_text = text.trim();

    if trimmed_text.is_empty() {
        return Err(invalid_syntax(
            "Expected an expanded type annotation block, but found empty input.",
        ));
    }

    let mut named_parameters = Vec::new();
    let mut return_type = SurfaceType::Null;
    let directives = collect_expanded_annotation_directives(trimmed_text)?;

    for directive in directives {
        if let Some(parameter_text) = directive.strip_prefix("@param") {
            let (type_text, name_text) = parse_braced_type_and_tail(parameter_text.trim())
                .ok_or_else(|| {
                    invalid_syntax("Expected `@param {TYPE} name` in the expanded annotation.")
                })?;
            let normalized_name = name_text
                .trim()
                .strip_prefix('[')
                .and_then(|name| name.strip_suffix(']'))
                .unwrap_or(name_text.trim());
            if normalized_name.is_empty() {
                return Err(invalid_syntax(
                    "Expected a parameter name after `@param {TYPE}`.",
                ));
            }
            let name = interner.intern(normalized_name);
            let surface_type = parse_surface_type(interner, type_text)?;
            named_parameters.push(RecordField::new(name, surface_type));
        } else if directive.starts_with("@returns") || directive.starts_with("@return") {
            let return_text = if let Some(return_text) = directive.strip_prefix("@returns") {
                return_text.trim()
            } else {
                directive.trim_start_matches("@return").trim()
            };
            let (type_text, trailing_text) = parse_braced_type_and_tail(return_text).ok_or_else(
                || {
                    invalid_syntax(
                        "Expected `@return {TYPE}` or `@returns {TYPE}` in the expanded annotation.",
                    )
                },
            )?;
            if !trailing_text.trim().is_empty() {
                return Err(invalid_syntax(
                    "Did not expect text after the return type in the expanded annotation.",
                ));
            }
            return_type = parse_surface_type(interner, type_text)?;
        } else {
            return Err(invalid_syntax(
                "Expected `@param`, `@return`, or `@returns` in the expanded annotation.",
            ));
        }
    }

    Ok(SurfaceType::Function(FunctionType::new(
        Vec::new(),
        named_parameters,
        return_type,
    )))
}

fn invalid_syntax(message: impl Into<String>) -> TypeParseError {
    TypeParseError::InvalidSyntax {
        message: message.into(),
    }
}

impl TypeParseError {
    fn with_context(self, context: impl AsRef<str>) -> Self {
        match self {
            Self::InvalidSyntax { message } => Self::InvalidSyntax {
                message: format!("{message} ({})", context.as_ref()),
            },
            other_error => other_error,
        }
    }
}

#[derive(Clone, Copy)]
struct StopContext {
    comma: bool,
    right_bracket: bool,
    right_brace: bool,
    right_paren: bool,
}

impl StopContext {
    const ROOT: Self = Self {
        comma: false,
        right_bracket: false,
        right_brace: false,
        right_paren: false,
    };

    const LIST_BRACE_ITEM: Self = Self {
        comma: true,
        right_bracket: false,
        right_brace: true,
        right_paren: false,
    };

    const FUNCTION_PARAMETER: Self = Self {
        comma: true,
        right_bracket: false,
        right_brace: false,
        right_paren: true,
    };

    fn stops_on(self, byte: u8) -> bool {
        (self.comma && byte == b',')
            || (self.right_bracket && byte == b']')
            || (self.right_brace && byte == b'}')
            || (self.right_paren && byte == b')')
    }
}

struct TypeParser<'a> {
    interner: &'a mut Interner,
    source: &'a str,
    position: usize,
}

impl<'a> TypeParser<'a> {
    fn new(interner: &'a mut Interner, source: &'a str) -> Self {
        Self {
            interner,
            source,
            position: 0,
        }
    }

    fn parse_type(&mut self) -> Result<SurfaceType, TypeParseError> {
        self.parse_type_until(StopContext::ROOT)
    }

    fn parse_type_until(
        &mut self,
        stop_context: StopContext,
    ) -> Result<SurfaceType, TypeParseError> {
        self.skip_ascii_whitespace();
        let mut surface_type = self.parse_primary(stop_context)?;

        loop {
            self.skip_ascii_whitespace();
            let Some(byte) = self.peek_byte() else {
                break;
            };

            if stop_context.stops_on(byte) {
                break;
            }

            if byte != b'|' {
                break;
            }

            self.position += 1;
            self.skip_ascii_whitespace();

            if self.consume_keyword("NULL") || self.consume_keyword("null") {
                surface_type = SurfaceType::Nullable(Box::new(surface_type));
                continue;
            }

            return Err(invalid_syntax(
                "Expected `NULL` after `|` in a nullable type.",
            ));
        }

        Ok(surface_type)
    }

    fn parse_primary(&mut self, stop_context: StopContext) -> Result<SurfaceType, TypeParseError> {
        self.skip_ascii_whitespace();

        if self.consume_keyword("list") {
            self.skip_ascii_whitespace();
            if self.consume_byte(b'[') {
                return self.parse_list_brackets(stop_context);
            }
            if self.consume_byte(b'{') {
                return self.parse_list_braces();
            }
            return Err(invalid_syntax(
                "Expected `[` or `{` after `list` in a list type.",
            ));
        }

        if self.consume_keyword("fn") {
            self.skip_ascii_whitespace();
            if !self.consume_byte(b'(') {
                return Err(invalid_syntax("Expected `(` after `fn`."));
            }
            return self.parse_function_type();
        }

        let identifier_span = self
            .parse_identifier_span()
            .ok_or_else(|| invalid_syntax("Expected a type."))?;
        let identifier = &self.source[identifier_span.0..identifier_span.1];

        let mut surface_type = parse_atomic_or_named_type(identifier).ok_or_else(|| {
            invalid_syntax(format!(
                "Unknown type `{identifier}`{}",
                self.context_suffix(stop_context)
            ))
        })?;

        loop {
            self.skip_ascii_whitespace();

            if self.consume_atomic_vector_suffix() {
                surface_type = SurfaceType::Vector(Box::new(surface_type));
                continue;
            }

            if self.consume_atomic_named_vector_suffix() {
                surface_type = SurfaceType::NamedVector(Box::new(surface_type));
                continue;
            }

            break;
        }

        Ok(surface_type)
    }

    fn parse_list_brackets(
        &mut self,
        caller_stop_context: StopContext,
    ) -> Result<SurfaceType, TypeParseError> {
        self.skip_ascii_whitespace();

        let is_named_list = if self.consume_keyword("named") {
            self.skip_ascii_whitespace();
            if !self.consume_byte(b':') {
                return Err(invalid_syntax("Expected `:` after `named` in `list[...]`."));
            }
            true
        } else {
            false
        };

        let item_type = self
            .parse_list_bracket_item_type(caller_stop_context)
            .map(Box::new)?;

        self.skip_ascii_whitespace();
        self.expect_byte(b']', "missing closing delimiter ]")?;

        if is_named_list {
            Ok(SurfaceType::NamedList(item_type))
        } else {
            Ok(SurfaceType::List(item_type))
        }
    }

    fn parse_list_bracket_item_type(
        &mut self,
        caller_stop_context: StopContext,
    ) -> Result<SurfaceType, TypeParseError> {
        self.skip_ascii_whitespace();

        if self.starts_list_brace_type() {
            self.consume_keyword("list");
            self.skip_ascii_whitespace();
            self.expect_byte(b'{', "Expected `{` after `list` in a list type.")?;
            return self.parse_list_braces();
        }

        self.parse_type_until(StopContext {
            comma: caller_stop_context.comma,
            right_bracket: true,
            right_brace: caller_stop_context.right_brace,
            right_paren: caller_stop_context.right_paren,
        })
    }

    fn parse_list_braces(&mut self) -> Result<SurfaceType, TypeParseError> {
        self.skip_ascii_whitespace();
        if self.consume_byte(b'}') {
            return Ok(SurfaceType::Tuple(Vec::new()));
        }

        let mut tuple_items = Vec::new();
        let mut record_fields = Vec::new();
        let mut item_kind = None;

        loop {
            self.skip_ascii_whitespace();

            if let Some((field_start, field_end)) = self.peek_record_field_name() {
                let saved_position = self.position;
                self.position = field_end;
                self.skip_ascii_whitespace();

                if self.consume_byte(b':') {
                    let field_name = &self.source[field_start..field_end];
                    let name = self.interner.intern(&self.source[field_start..field_end]);
                    let value = match self.parse_type_until(StopContext::LIST_BRACE_ITEM) {
                        Ok(value) => value,
                        Err(error) => {
                            return Err(
                                error.with_context(format!("while parsing field `{field_name}`"))
                            );
                        }
                    };

                    match item_kind {
                        None => item_kind = Some(ListBraceItemKind::Field),
                        Some(ListBraceItemKind::Item) => {
                            return Err(invalid_syntax(
                                "Cannot mix named and unnamed items in `list{...}`.",
                            ));
                        }
                        Some(ListBraceItemKind::Field) => {}
                    }

                    record_fields.push(RecordField::new(name, value));

                    self.skip_ascii_whitespace();
                    if self.consume_byte(b',') {
                        continue;
                    }
                    self.expect_byte(b'}', "Expected `}` to close `list{...}`.")?;
                    break;
                }

                self.position = saved_position;
            }

            let value = self
                .parse_type_until(StopContext::LIST_BRACE_ITEM)
                .map_err(|error| error.with_context("while parsing tuple item"))?;

            match item_kind {
                None => item_kind = Some(ListBraceItemKind::Item),
                Some(ListBraceItemKind::Field) => {
                    return Err(invalid_syntax(
                        "Cannot mix named and unnamed items in `list{...}`.",
                    ));
                }
                Some(ListBraceItemKind::Item) => {}
            }

            tuple_items.push(value);

            self.skip_ascii_whitespace();
            if self.consume_byte(b',') {
                continue;
            }
            self.expect_byte(b'}', "Expected `}` to close `list{...}`.")?;
            break;
        }

        Ok(match item_kind {
            Some(ListBraceItemKind::Field) => SurfaceType::Record(record_fields),
            Some(ListBraceItemKind::Item) | None => SurfaceType::Tuple(tuple_items),
        })
    }

    fn parse_function_type(&mut self) -> Result<SurfaceType, TypeParseError> {
        self.skip_ascii_whitespace();
        let mut parameters = Vec::new();
        let mut named_parameters = Vec::new();

        if !self.consume_byte(b')') {
            loop {
                self.skip_ascii_whitespace();
                let parameter_start = self.position;

                let parsed_name = self.parse_identifier_span();
                let is_named = if let Some((start, end)) = parsed_name {
                    self.skip_ascii_whitespace();
                    if self.consume_byte(b':') {
                        let name = self.interner.intern(&self.source[start..end]);
                        let value = self
                            .parse_type_until(StopContext::FUNCTION_PARAMETER)
                            .map_err(|error| {
                                error.with_context("while parsing named parameter type")
                            })?;
                        named_parameters.push(RecordField::new(name, value));
                        true
                    } else {
                        self.position = parameter_start;
                        false
                    }
                } else {
                    false
                };

                if !is_named {
                    let parameter = self
                        .parse_type_until(StopContext::FUNCTION_PARAMETER)
                        .map_err(|error| {
                            error.with_context("while parsing positional parameter")
                        })?;
                    parameters.push(parameter);
                }

                self.skip_ascii_whitespace();
                if self.consume_byte(b',') {
                    continue;
                }
                self.expect_byte(b')', "Expected `)` to close `fn(...)`.")?;
                break;
            }
        }

        self.skip_ascii_whitespace();
        let return_type = if self.consume_byte(b'-') {
            self.expect_byte(b'>', "Expected `>` after `-` in function return type.")?;
            self.parse_type_until(StopContext::ROOT)?
        } else {
            SurfaceType::Null
        };

        Ok(SurfaceType::Function(FunctionType::new(
            parameters,
            named_parameters,
            return_type,
        )))
    }

    fn parse_identifier_span(&mut self) -> Option<(usize, usize)> {
        self.skip_ascii_whitespace();
        let start = self.position;
        let bytes = self.source.as_bytes();

        let first = *bytes.get(start)?;
        if !(first == b'_' || first.is_ascii_alphabetic()) {
            return None;
        }

        self.position += 1;
        while let Some(byte) = bytes.get(self.position).copied() {
            if byte == b'_' || byte.is_ascii_alphanumeric() {
                self.position += 1;
            } else {
                break;
            }
        }

        Some((start, self.position))
    }

    fn peek_record_field_name(&self) -> Option<(usize, usize)> {
        let mut position = self.position;
        let bytes = self.source.as_bytes();

        while let Some(byte) = bytes.get(position).copied() {
            if byte.is_ascii_whitespace() {
                position += 1;
            } else {
                break;
            }
        }

        let start = position;
        let first = *bytes.get(start)?;
        if !(first == b'_' || first.is_ascii_alphabetic()) {
            return None;
        }

        position += 1;
        while let Some(byte) = bytes.get(position).copied() {
            if byte == b'_' || byte.is_ascii_alphanumeric() {
                position += 1;
            } else {
                break;
            }
        }

        if let Some(byte) = bytes.get(position).copied() {
            if !byte.is_ascii() {
                return None;
            }
        }

        Some((start, position))
    }

    fn starts_list_brace_type(&self) -> bool {
        let mut position = self.position;
        let bytes = self.source.as_bytes();

        while let Some(byte) = bytes.get(position).copied() {
            if byte.is_ascii_whitespace() {
                position += 1;
            } else {
                break;
            }
        }

        if !self.source[position..].starts_with("list") {
            return false;
        }

        let list_end = position + 4;
        if let Some(next_byte) = bytes.get(list_end).copied() {
            if next_byte == b'_' || next_byte.is_ascii_alphanumeric() {
                return false;
            }
        }

        position = list_end;
        while let Some(byte) = bytes.get(position).copied() {
            if byte.is_ascii_whitespace() {
                position += 1;
            } else {
                break;
            }
        }

        bytes.get(position).copied() == Some(b'{')
    }

    fn context_suffix(&self, stop_context: StopContext) -> &'static str {
        if stop_context.right_bracket {
            " in `list[...]`"
        } else if stop_context.right_brace {
            " in `list{...}`"
        } else if stop_context.right_paren {
            " in `fn(...)`"
        } else {
            ""
        }
    }

    fn skip_ascii_whitespace(&mut self) {
        while let Some(byte) = self.peek_byte() {
            if byte.is_ascii_whitespace() {
                self.position += 1;
            } else {
                break;
            }
        }
    }

    fn peek_byte(&self) -> Option<u8> {
        self.source.as_bytes().get(self.position).copied()
    }

    fn consume_byte(&mut self, expected: u8) -> bool {
        if self.peek_byte() == Some(expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn expect_byte(&mut self, expected: u8, message: &str) -> Result<(), TypeParseError> {
        if self.consume_byte(expected) {
            Ok(())
        } else {
            Err(invalid_syntax(message))
        }
    }

    fn consume_keyword(&mut self, keyword: &str) -> bool {
        let remaining = &self.source[self.position..];
        if !remaining.starts_with(keyword) {
            return false;
        }

        let end = self.position + keyword.len();
        if let Some(next_byte) = self.source.as_bytes().get(end).copied() {
            if next_byte == b'_' || next_byte.is_ascii_alphanumeric() {
                return false;
            }
        }

        self.position = end;
        true
    }

    fn consume_atomic_vector_suffix(&mut self) -> bool {
        let saved_position = self.position;
        self.skip_ascii_whitespace();
        if self.consume_byte(b'[') {
            self.skip_ascii_whitespace();
            if self.consume_byte(b']') {
                return true;
            }
        }
        self.position = saved_position;
        false
    }

    fn consume_atomic_named_vector_suffix(&mut self) -> bool {
        let saved_position = self.position;
        self.skip_ascii_whitespace();
        if self.consume_byte(b'[') {
            self.skip_ascii_whitespace();
            if self.consume_keyword("named") {
                self.skip_ascii_whitespace();
                if self.consume_byte(b']') {
                    return true;
                }
            }
        }
        self.position = saved_position;
        false
    }

    fn is_at_end(&self) -> bool {
        self.position >= self.source.len()
    }
}

#[derive(Clone, Copy)]
enum ListBraceItemKind {
    Field,
    Item,
}

fn parse_atomic_or_named_type(text: &str) -> Option<SurfaceType> {
    match text {
        "Any" | "any" => Some(SurfaceType::Any),
        "Unknown" | "unknown" => Some(SurfaceType::Unknown),
        "NULL" | "null" => Some(SurfaceType::Null),
        "logical" => Some(SurfaceType::Scalar(Atomic::Logical)),
        "integer" => Some(SurfaceType::Scalar(Atomic::Integer)),
        "double" => Some(SurfaceType::Scalar(Atomic::Double)),
        "complex" => Some(SurfaceType::Scalar(Atomic::Complex)),
        "character" => Some(SurfaceType::Scalar(Atomic::Character)),
        "raw" => Some(SurfaceType::Scalar(Atomic::Raw)),
        _ => None,
    }
}

fn parse_braced_type_and_tail(text: &str) -> Option<(&str, &str)> {
    let inner_text = text.strip_prefix('{')?;
    let closing_index = find_matching_closer(inner_text, '{', '}')?;
    let type_text = &inner_text[..closing_index];
    let trailing_text = &inner_text[closing_index + 1..];
    Some((type_text.trim(), trailing_text))
}

fn collect_expanded_annotation_directives(text: &str) -> Result<Vec<String>, TypeParseError> {
    let mut directives = Vec::new();
    let mut current_directive = String::new();
    let mut delimiter_stack = Vec::new();

    for line in text.lines() {
        let trimmed_line = line.trim();
        if trimmed_line.is_empty() {
            continue;
        }

        let content = if let Some(content) = trimmed_line.strip_prefix("#:") {
            content.trim()
        } else {
            trimmed_line
        };

        let starts_new_directive = content.starts_with("@param")
            || content.starts_with("@return")
            || content.starts_with("@returns");

        if starts_new_directive {
            if !current_directive.is_empty() {
                if !delimiter_stack.is_empty() {
                    let missing_closer = expected_closer(*delimiter_stack.last().unwrap_or(&'{'));
                    return Err(invalid_syntax(format!(
                        "missing closing delimiter {missing_closer}"
                    )));
                }
                directives.push(current_directive.trim().to_owned());
                current_directive.clear();
            }
        } else if current_directive.is_empty() {
            return Err(invalid_syntax(
                "Expected an expanded annotation directive starting with `@param`, `@return`, or `@returns`.",
            ));
        } else {
            current_directive.push('\n');
        }

        current_directive.push_str(content);

        for character in content.chars() {
            match character {
                '(' | '[' | '{' => delimiter_stack.push(character),
                ')' | ']' | '}' => {
                    if !pop_matching_delimiter(&mut delimiter_stack, character) {
                        return Err(invalid_syntax(format!(
                            "unexpected closing delimiter {character}"
                        )));
                    }
                }
                _ => {}
            }
        }
    }

    if current_directive.is_empty() {
        return Err(invalid_syntax(
            "Expected at least one expanded annotation directive.",
        ));
    }

    if !delimiter_stack.is_empty() {
        let missing_closer = expected_closer(*delimiter_stack.last().unwrap_or(&'{'));
        return Err(invalid_syntax(format!(
            "missing closing delimiter {missing_closer}"
        )));
    }

    directives.push(current_directive.trim().to_owned());
    Ok(directives)
}

fn find_matching_closer(text: &str, opener: char, _closer: char) -> Option<usize> {
    let mut delimiter_stack = vec![opener];

    for (index, character) in text.char_indices() {
        match character {
            '(' | '[' | '{' => delimiter_stack.push(character),
            ')' | ']' | '}' => {
                if !pop_matching_delimiter(&mut delimiter_stack, character) {
                    return None;
                }
                if delimiter_stack.is_empty() {
                    return Some(index);
                }
            }
            _ => {}
        }
    }

    None
}

fn pop_matching_delimiter(delimiter_stack: &mut Vec<char>, closer: char) -> bool {
    let Some(opener) = delimiter_stack.pop() else {
        return false;
    };

    matches!((opener, closer), ('(', ')') | ('[', ']') | ('{', '}'))
}

fn expected_closer(opener: char) -> char {
    match opener {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        _ => '}',
    }
}

struct SurfaceTypeRenderer<'a> {
    interner: &'a Interner,
}

impl<'a> SurfaceTypeRenderer<'a> {
    fn new(interner: &'a Interner) -> Self {
        Self { interner }
    }

    fn render(&mut self, surface_type: &SurfaceType) -> String {
        match surface_type {
            SurfaceType::Any => "Any".to_owned(),
            SurfaceType::Unknown => "Unknown".to_owned(),
            SurfaceType::Null => "NULL".to_owned(),
            SurfaceType::Nullable(inner_type) => format!("{} | NULL", self.render(inner_type)),
            SurfaceType::Scalar(atomic) => render_atomic(*atomic).to_owned(),
            SurfaceType::Vector(inner_type) => format!("{}[]", self.render(inner_type)),
            SurfaceType::NamedVector(inner_type) => format!("{}[named]", self.render(inner_type)),
            SurfaceType::List(item_type) => format!("list[{}]", self.render(item_type)),
            SurfaceType::NamedList(item_type) => {
                format!("list[named: {}]", self.render(item_type))
            }
            SurfaceType::Record(fields) => {
                let rendered_fields = fields
                    .iter()
                    .map(|field| {
                        let name = self.interner.resolve(field.name).unwrap_or("<unknown>");
                        format!("{name}: {}", self.render(&field.value))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("list{{{rendered_fields}}}")
            }
            SurfaceType::Tuple(items) => {
                let rendered_items = items
                    .iter()
                    .map(|item| self.render(item))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("list{{{rendered_items}}}")
            }
            SurfaceType::Function(function_type) => {
                let rendered_parameters = function_type
                    .parameters
                    .iter()
                    .map(|parameter| self.render(parameter))
                    .collect::<Vec<_>>();
                let rendered_named_parameters = function_type
                    .named_parameters
                    .iter()
                    .map(|parameter| {
                        let name = self.interner.resolve(parameter.name).unwrap_or("<unknown>");
                        format!("{name}: {}", self.render(&parameter.value))
                    })
                    .collect::<Vec<_>>();
                let mut rendered_parts = rendered_parameters;
                rendered_parts.extend(rendered_named_parameters);
                format!(
                    "fn({}) -> {}",
                    rendered_parts.join(", "),
                    self.render(&function_type.return_type)
                )
            }
        }
    }
}

fn render_atomic(atomic: Atomic) -> &'static str {
    match atomic {
        Atomic::Logical => "logical",
        Atomic::Integer => "integer",
        Atomic::Double => "double",
        Atomic::Complex => "complex",
        Atomic::Character => "character",
        Atomic::Raw => "raw",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parser<'a>(source: &'a str) -> TypeParser<'a> {
        let interner = Box::new(Interner::new());
        TypeParser::new(Box::leak(interner), source)
    }

    #[test]
    fn nested_named_list_tuple_value_consumes_inner_record_before_outer_bracket() {
        let source = "list[named:list{integer,character}]}";
        let mut parser = parser(source);

        assert!(parser.consume_keyword("list"));
        assert!(parser.consume_byte(b'['));
        assert!(parser.consume_keyword("named"));
        assert!(parser.consume_byte(b':'));

        let inner_type = parser
            .parse_list_bracket_item_type(StopContext::ROOT)
            .expect("named-list inner tuple-like value should parse");

        assert_eq!(
            inner_type,
            SurfaceType::Tuple(vec![
                SurfaceType::Scalar(Atomic::Integer),
                SurfaceType::Scalar(Atomic::Character),
            ])
        );
        assert_eq!(parser.peek_byte(), Some(b']'));
        assert_eq!(parser.position, source.len() - 2);
    }

    #[test]
    fn nested_named_list_tuple_value_inside_record_stops_before_enclosing_record_closer() {
        let source = "items:list[named:list{integer,character}]}}";
        let mut parser = parser(source);

        let (field_start, field_end) = parser
            .peek_record_field_name()
            .expect("record field lookahead should find `items`");
        assert_eq!(&source[field_start..field_end], "items");

        parser.position = field_end;
        parser.skip_ascii_whitespace();
        assert!(parser.consume_byte(b':'));

        let field_value = parser
            .parse_type_until(StopContext::LIST_BRACE_ITEM)
            .expect("record field value should parse");

        assert_eq!(
            field_value,
            SurfaceType::NamedList(Box::new(SurfaceType::Tuple(vec![
                SurfaceType::Scalar(Atomic::Integer),
                SurfaceType::Scalar(Atomic::Character),
            ])))
        );
        assert_eq!(parser.peek_byte(), Some(b'}'));
        assert_eq!(parser.position, source.len() - 2);
    }

    #[test]
    fn non_ascii_record_field_name_is_not_treated_as_a_valid_field_name() {
        let source = "naïve:integer}";
        let mut parser = parser(source);

        assert!(
            parser.peek_record_field_name().is_none(),
            "ASCII-only field-name scanning should reject non-ASCII names for now"
        );

        let error = parser
            .parse_type_until(StopContext::LIST_BRACE_ITEM)
            .expect_err("non-ASCII field names should currently fail to parse");

        assert_eq!(
            error,
            TypeParseError::InvalidSyntax {
                message: "Unknown type `na` in `list{...}`".to_owned(),
            }
        );
    }
}
