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
    parser.expect_end()?;
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

struct TypeParser<'a, 'b> {
    interner: &'a mut Interner,
    text: &'b str,
    position: usize,
}

impl<'a, 'b> TypeParser<'a, 'b> {
    fn new(interner: &'a mut Interner, text: &'b str) -> Self {
        Self {
            interner,
            text,
            position: 0,
        }
    }

    fn parse_type(&mut self) -> Result<SurfaceType, TypeParseError> {
        self.parse_type_until(StopContext::root())
    }

    fn parse_type_until(
        &mut self,
        stop_context: StopContext,
    ) -> Result<SurfaceType, TypeParseError> {
        self.skip_whitespace();
        let left_type = self.parse_primary_type()?;
        self.skip_whitespace();

        if stop_context.should_stop_at(self.peek_char()) {
            return Ok(left_type);
        }

        if self.peek_char() == Some('|') && !stop_context.stops_before_union {
            self.consume_char('|');
            self.skip_whitespace();
            let right_type = self.parse_type_until(stop_context)?;
            return match (left_type, right_type) {
                (SurfaceType::Null, other_type) | (other_type, SurfaceType::Null) => {
                    Ok(SurfaceType::Nullable(Box::new(other_type)))
                }
                _ => Err(invalid_syntax(
                    "Only nullable unions with `NULL` are supported, like `T | NULL`.",
                )),
            };
        }

        Ok(left_type)
    }

    fn parse_primary_type(&mut self) -> Result<SurfaceType, TypeParseError> {
        self.skip_whitespace();

        if self.consume_keyword("list[") {
            return self.parse_list_bracket_type();
        }

        if self.consume_keyword("list{") {
            return self.parse_record_or_tuple_type();
        }

        if self.consume_keyword("fn(") {
            return self.parse_function_type();
        }

        if let Some(identifier) = self.parse_identifier_text() {
            return self.parse_identifier_type(&identifier);
        }

        Err(invalid_syntax("invalid syntax in type expression"))
    }

    fn parse_list_bracket_type(&mut self) -> Result<SurfaceType, TypeParseError> {
        self.skip_whitespace();
        let body_start = self.position;
        let body_end = self
            .find_matching_delimiter_from_current_position('[', ']')
            .ok_or_else(|| invalid_syntax("missing closing delimiter ]"))?;
        let body_text = self.text[body_start..body_end].trim();

        if body_text.is_empty() {
            return Err(invalid_syntax("invalid syntax in type expression"));
        }

        let parsed_type = if let Some(value_text) = body_text.strip_prefix("named:") {
            let value_type = parse_surface_type(self.interner, value_text.trim())
                .map_err(|error| error.with_context("while parsing named list element type"))?;
            SurfaceType::NamedList(Box::new(value_type))
        } else {
            let item_type = parse_surface_type(self.interner, body_text)
                .map_err(|error| error.with_context("while parsing list element type"))?;
            SurfaceType::List(Box::new(item_type))
        };

        self.position = body_end + ']'.len_utf8();
        Ok(parsed_type)
    }

    fn parse_record_or_tuple_type(&mut self) -> Result<SurfaceType, TypeParseError> {
        let mut items = Vec::new();
        let mut fields = Vec::new();
        let mut saw_named_item = false;
        let mut saw_unnamed_item = false;

        self.skip_whitespace();
        if self.peek_char() == Some('}') {
            self.consume_char('}');
            return Ok(SurfaceType::Tuple(Vec::new()));
        }

        loop {
            self.skip_whitespace();

            let start_position = self.position;
            if let Some(name_text) = self.parse_identifier_text() {
                self.skip_whitespace();
                if self.consume_char(':') {
                    self.skip_whitespace();
                    let name = self.interner.intern(&name_text);
                    let value_type = self
                        .parse_type_until(StopContext::for_record_or_tuple_item())
                        .map_err(|error| {
                            error.with_context(format!("while parsing field `{name_text}`"))
                        })?;
                    fields.push(RecordField::new(name, value_type));
                    saw_named_item = true;
                } else {
                    self.position = start_position;
                    let item_type = self
                        .parse_type_until(StopContext::for_record_or_tuple_item())
                        .map_err(|error| error.with_context("while parsing tuple item"))?;
                    items.push(item_type);
                    saw_unnamed_item = true;
                }
            } else {
                let item_type = self
                    .parse_type_until(StopContext::for_record_or_tuple_item())
                    .map_err(|error| error.with_context("while parsing tuple item"))?;
                items.push(item_type);
                saw_unnamed_item = true;
            }

            self.skip_whitespace();
            if self.consume_char(',') {
                self.skip_whitespace();
                if self.peek_char() == Some('}') {
                    self.consume_char('}');
                    break;
                }
                continue;
            }

            self.expect_char('}')?;
            break;
        }

        if saw_named_item && saw_unnamed_item {
            return Err(invalid_syntax(
                "Cannot mix named and unnamed items in the same `list{...}` type.",
            ));
        }

        if saw_named_item {
            return Ok(SurfaceType::Record(fields));
        }

        Ok(SurfaceType::Tuple(items))
    }

    fn parse_function_type(&mut self) -> Result<SurfaceType, TypeParseError> {
        let mut parameters = Vec::new();
        let mut named_parameters = Vec::new();

        self.skip_whitespace();
        if self.peek_char() == Some(')') {
            self.consume_char(')');
        } else {
            loop {
                self.skip_whitespace();

                let start_position = self.position;
                if let Some(name_text) = self.parse_identifier_text() {
                    self.skip_whitespace();
                    if self.consume_char(':') {
                        self.skip_whitespace();
                        let name = self.interner.intern(&name_text);
                        let parameter_type = self
                            .parse_type_until(StopContext::for_function_parameter())
                            .map_err(|error| {
                                error.with_context(format!(
                                    "while parsing function parameter `{name_text}`"
                                ))
                            })?;
                        named_parameters.push(RecordField::new(name, parameter_type));
                    } else {
                        self.position = start_position;
                        let parameter_type = self
                            .parse_type_until(StopContext::for_function_parameter())
                            .map_err(|error| {
                                error.with_context("while parsing function parameter")
                            })?;
                        parameters.push(parameter_type);
                    }
                } else {
                    let parameter_type = self
                        .parse_type_until(StopContext::for_function_parameter())
                        .map_err(|error| error.with_context("while parsing function parameter"))?;
                    parameters.push(parameter_type);
                }

                self.skip_whitespace();
                if self.consume_char(',') {
                    self.skip_whitespace();
                    if self.peek_char() == Some(')') {
                        self.consume_char(')');
                        break;
                    }
                    continue;
                }

                self.expect_char(')')?;
                break;
            }
        }

        self.skip_whitespace();
        let return_type = if self.consume_keyword("->") {
            self.skip_whitespace();
            self.parse_type()
                .map_err(|error| error.with_context("while parsing function return type"))?
        } else {
            SurfaceType::Null
        };

        Ok(SurfaceType::Function(FunctionType::new(
            parameters,
            named_parameters,
            return_type,
        )))
    }

    fn parse_identifier_type(&mut self, identifier: &str) -> Result<SurfaceType, TypeParseError> {
        self.skip_whitespace();

        if self.consume_keyword("[named]") {
            let inner_type = self.parse_atomic_identifier_type(identifier)?;
            return Ok(SurfaceType::NamedVector(Box::new(inner_type)));
        }

        if self.consume_keyword("[]") {
            let inner_type = self.parse_atomic_identifier_type(identifier)?;
            return Ok(SurfaceType::Vector(Box::new(inner_type)));
        }

        self.parse_atomic_identifier_type(identifier)
    }

    fn parse_atomic_identifier_type(
        &self,
        identifier: &str,
    ) -> Result<SurfaceType, TypeParseError> {
        match identifier {
            "logical" => Ok(SurfaceType::Scalar(Atomic::Logical)),
            "integer" => Ok(SurfaceType::Scalar(Atomic::Integer)),
            "double" => Ok(SurfaceType::Scalar(Atomic::Double)),
            "complex" => Ok(SurfaceType::Scalar(Atomic::Complex)),
            "character" => Ok(SurfaceType::Scalar(Atomic::Character)),
            "raw" => Ok(SurfaceType::Scalar(Atomic::Raw)),
            "NULL" | "null" => Ok(SurfaceType::Null),
            "Unknown" | "unknown" => Ok(SurfaceType::Unknown),
            "Any" | "any" => Ok(SurfaceType::Any),
            other_name if looks_like_identifier(other_name) => Err(TypeParseError::UnknownType {
                name: other_name.to_owned(),
            }),
            _ => Err(invalid_syntax("invalid syntax in type expression")),
        }
    }

    fn expect_end(&mut self) -> Result<(), TypeParseError> {
        self.skip_whitespace();
        if self.position == self.text.len() {
            Ok(())
        } else {
            Err(invalid_syntax("invalid syntax in type expression"))
        }
    }

    fn expect_char(&mut self, expected: char) -> Result<(), TypeParseError> {
        self.skip_whitespace();
        if self.consume_char(expected) {
            Ok(())
        } else {
            Err(invalid_syntax(format!(
                "missing closing delimiter {expected}"
            )))
        }
    }

    fn parse_identifier_text(&mut self) -> Option<String> {
        self.skip_whitespace();
        let start = self.position;
        let mut characters = self.text[self.position..].char_indices();

        let (_, first_character) = characters.next()?;
        if !(first_character == '_' || first_character.is_ascii_alphabetic()) {
            return None;
        }

        let mut end = start + first_character.len_utf8();
        for (offset, character) in characters {
            if character == '_' || character.is_ascii_alphanumeric() {
                end = start + offset + character.len_utf8();
            } else {
                break;
            }
        }

        self.position = end;
        Some(self.text[start..end].to_owned())
    }

    fn consume_keyword(&mut self, keyword: &str) -> bool {
        if self.text[self.position..].starts_with(keyword) {
            self.position += keyword.len();
            true
        } else {
            false
        }
    }

    fn consume_char(&mut self, expected: char) -> bool {
        let Some(character) = self.peek_char() else {
            return false;
        };
        if character == expected {
            self.position += character.len_utf8();
            true
        } else {
            false
        }
    }

    fn peek_char(&self) -> Option<char> {
        self.text[self.position..].chars().next()
    }

    fn find_matching_delimiter_from_current_position(
        &self,
        opener: char,
        closer: char,
    ) -> Option<usize> {
        let mut delimiter_stack = vec![opener];

        for (offset, character) in self.text[self.position..].char_indices() {
            match character {
                '(' | '[' | '{' => delimiter_stack.push(character),
                ')' | ']' | '}' => {
                    if !pop_matching_delimiter(&mut delimiter_stack, character) {
                        return None;
                    }
                    if delimiter_stack.is_empty() {
                        return Some(self.position + offset);
                    }
                }
                _ => {}
            }
        }

        let _ = closer;
        None
    }

    fn skip_whitespace(&mut self) {
        while let Some(character) = self.peek_char() {
            if character.is_whitespace() {
                self.position += character.len_utf8();
            } else {
                break;
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct StopContext {
    stop_at_comma: bool,
    stop_at_right_bracket: bool,
    stop_at_right_brace: bool,
    stop_at_right_paren: bool,
    stops_before_union: bool,
}

impl StopContext {
    fn root() -> Self {
        Self {
            stop_at_comma: false,
            stop_at_right_bracket: false,
            stop_at_right_brace: false,
            stop_at_right_paren: false,
            stops_before_union: false,
        }
    }

    fn for_list_bracket() -> Self {
        Self {
            stop_at_comma: false,
            stop_at_right_bracket: true,
            stop_at_right_brace: true,
            stop_at_right_paren: true,
            stops_before_union: false,
        }
    }

    fn for_record_or_tuple_item() -> Self {
        Self {
            stop_at_comma: true,
            stop_at_right_bracket: false,
            stop_at_right_brace: true,
            stop_at_right_paren: false,
            stops_before_union: false,
        }
    }

    fn for_function_parameter() -> Self {
        Self {
            stop_at_comma: true,
            stop_at_right_bracket: false,
            stop_at_right_brace: false,
            stop_at_right_paren: true,
            stops_before_union: false,
        }
    }

    fn should_stop_at(self, next_char: Option<char>) -> bool {
        matches!(
            next_char,
            Some(character)
                if self.stop_at_comma && character == ','
                    || self.stop_at_right_bracket && character == ']'
                    || self.stop_at_right_brace && character == '}'
                    || self.stop_at_right_paren && character == ')'
        )
    }
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

fn looks_like_identifier(text: &str) -> bool {
    let mut characters = text.chars();
    let Some(first_character) = characters.next() else {
        return false;
    };

    if !(first_character == '_' || first_character.is_ascii_alphabetic()) {
        return false;
    }

    characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
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
