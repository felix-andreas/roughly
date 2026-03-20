use crate::{
    interner::{Interner, Symbol},
    types::{AnnotationKind, Atomic, FunctionType, RecordField, SurfaceType},
};

pub fn render_type_syntax_item(item: &TypeSyntaxItem, interner: &Interner) -> String {
    match item {
        TypeSyntaxItem::SurfaceType(surface_type) => render_surface_type(surface_type, interner),
        TypeSyntaxItem::IfUnknown(surface_type) => {
            format!(
                "@if-unknown {}",
                render_surface_type(surface_type, interner)
            )
        }
        TypeSyntaxItem::Trust(surface_type) => {
            format!("@trust {}", render_surface_type(surface_type, interner))
        }
        TypeSyntaxItem::New(name) => {
            let name = interner.resolve(*name).unwrap_or("<unknown>");
            format!("@new {name}")
        }
        TypeSyntaxItem::TypeDefinition { name, surface_type } => {
            let name = interner.resolve(*name).unwrap_or("<unknown>");
            format!(
                "@type {name} {{{}}}",
                render_surface_type(surface_type, interner)
            )
        }
        TypeSyntaxItem::TypeAlias { name, surface_type } => {
            let name = interner.resolve(*name).unwrap_or("<unknown>");
            format!(
                "@alias {name} {{{}}}",
                render_surface_type(surface_type, interner)
            )
        }
    }
}

pub fn render_surface_type(surface_type: &SurfaceType, interner: &Interner) -> String {
    match surface_type {
        SurfaceType::Any => "Any".to_owned(),
        SurfaceType::Unknown => "Unknown".to_owned(),
        SurfaceType::Null => "NULL".to_owned(),
        SurfaceType::Nullable(inner_type) => {
            format!("{} | NULL", render_surface_type(inner_type, interner))
        }
        SurfaceType::Scalar(atomic) => match atomic {
            Atomic::Logical => "logical",
            Atomic::Integer => "integer",
            Atomic::Double => "double",
            Atomic::Complex => "complex",
            Atomic::Character => "character",
            Atomic::Raw => "raw",
        }
        .to_owned(),
        SurfaceType::Named(name) => interner.resolve(*name).unwrap_or("<unknown>").to_owned(),
        SurfaceType::Vector(inner_type) => {
            format!("{}[]", render_surface_type(inner_type, interner))
        }
        SurfaceType::NamedVector(inner_type) => {
            format!("{}[named]", render_surface_type(inner_type, interner))
        }
        SurfaceType::List(item_type) => {
            format!("list[{}]", render_surface_type(item_type, interner))
        }
        SurfaceType::NamedList(item_type) => {
            format!("list[named: {}]", render_surface_type(item_type, interner))
        }
        SurfaceType::Record(fields) => {
            let rendered_fields = fields
                .iter()
                .map(|field| {
                    let name = interner.resolve(field.name).unwrap_or("<unknown>");
                    format!("{name}: {}", render_surface_type(&field.value, interner))
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("list{{{rendered_fields}}}")
        }
        SurfaceType::Tuple(items) => {
            let rendered_items = items
                .iter()
                .map(|item| render_surface_type(item, interner))
                .collect::<Vec<_>>()
                .join(", ");
            format!("list{{{rendered_items}}}")
        }
        SurfaceType::Function(function_type) => {
            let rendered_parameters = function_type
                .parameters
                .iter()
                .map(|parameter| render_surface_type(parameter, interner))
                .collect::<Vec<_>>();
            let rendered_named_parameters = function_type
                .named_parameters
                .iter()
                .map(|parameter| {
                    let name = interner.resolve(parameter.name).unwrap_or("<unknown>");
                    format!(
                        "{name}: {}",
                        render_surface_type(&parameter.value, interner)
                    )
                })
                .collect::<Vec<_>>();
            let mut rendered_parts = rendered_parameters;
            rendered_parts.extend(rendered_named_parameters);
            format!(
                "fn({}) -> {}",
                rendered_parts.join(", "),
                render_surface_type(&function_type.return_type, interner)
            )
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeParseError {
    InvalidSyntax { message: String },
    UnknownType { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeSyntaxItem {
    SurfaceType(SurfaceType),
    IfUnknown(SurfaceType),
    Trust(SurfaceType),
    New(Symbol),
    TypeDefinition {
        name: Symbol,
        surface_type: SurfaceType,
    },
    TypeAlias {
        name: Symbol,
        surface_type: SurfaceType,
    },
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
        return Err(invalid_syntax("expected a type, but found empty input."));
    }

    let surface_text = if allow_annotation_kind_prefix {
        annotation_surface_text(trimmed_text)
            .ok_or_else(|| invalid_syntax("expected a type after the annotation prefix."))?
    } else {
        trimmed_text
    };

    let mut parser = TypeParser::new(interner, surface_text);
    let surface_type = parser.parse_type()?;
    parser.skip_ascii_whitespace();

    if !parser.is_at_end() {
        return Err(invalid_syntax(format!(
            "unexpected trailing input starting at byte {}.",
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
            "expected a type annotation, but found empty input.",
        ));
    }

    let normalized_lines = trimmed_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.strip_prefix("#:").map(str::trim).unwrap_or(line))
        .collect::<Vec<_>>();

    let Some(first_line) = normalized_lines.first().copied() else {
        return Err(invalid_syntax(
            "expected a type annotation, but found empty input.",
        ));
    };

    if let Some(expanded_block_text) = parse_expanded_annotation_block_text(&normalized_lines)? {
        let surface_type = parse_expanded_block_surface_type(interner, &expanded_block_text)?;
        return Ok(crate::types::Annotation::new(
            AnnotationKind::Checked,
            surface_type,
        ));
    }

    if normalized_lines.len() > 1 {
        return Err(invalid_syntax(
            "cannot use multiple compact annotations in the same `#:` block.",
        ));
    }

    let (kind, surface_text) = parse_compact_annotation_kind_and_surface_text(first_line)?;

    let surface_type = parse_surface_type(interner, surface_text)?;
    Ok(crate::types::Annotation::new(kind, surface_type))
}

pub fn parse_type_syntax_item(
    interner: &mut Interner,
    text: &str,
) -> Result<TypeSyntaxItem, TypeParseError> {
    let trimmed_text = text.trim();

    if trimmed_text.is_empty() {
        return Err(invalid_syntax("expected a type, but found empty input."));
    }

    if trimmed_text.starts_with('@') {
        return parse_directive_type_syntax_item(interner, trimmed_text);
    }

    parse_surface_type(interner, trimmed_text).map(TypeSyntaxItem::SurfaceType)
}

fn parse_directive_type_syntax_item(
    interner: &mut Interner,
    text: &str,
) -> Result<TypeSyntaxItem, TypeParseError> {
    let (directive_name, directive_body) = parse_annotation_directive_name_and_body(text)
        .ok_or_else(|| invalid_syntax("expected a type."))?;

    match directive_name {
        "type" => {
            let (name, surface_type) = parse_named_type_definition(interner, directive_body)?;
            Ok(TypeSyntaxItem::TypeDefinition { name, surface_type })
        }
        "alias" => {
            let (name, surface_type) = parse_named_type_definition(interner, directive_body)?;
            Ok(TypeSyntaxItem::TypeAlias { name, surface_type })
        }
        "if-unknown" => {
            let surface_text = keyword_surface_text(directive_body)
                .ok_or_else(|| invalid_syntax("expected a type after the annotation prefix."))?;
            let surface_type = parse_surface_type(interner, surface_text)?;
            Ok(TypeSyntaxItem::IfUnknown(surface_type))
        }
        "trust" => {
            let surface_text = keyword_surface_text(directive_body)
                .ok_or_else(|| invalid_syntax("expected a type after the annotation prefix."))?;
            let surface_type = parse_surface_type(interner, surface_text)?;
            Ok(TypeSyntaxItem::Trust(surface_type))
        }
        "new" => {
            let normalized_name = directive_body;
            if normalized_name.is_empty() {
                return Err(invalid_syntax(
                    "expected a type after the annotation prefix.",
                ));
            }

            let Some((name_start, name_end)) = identifier_span(normalized_name) else {
                return Err(invalid_syntax("expected a type."));
            };

            let name = &normalized_name[name_start..name_end];
            if name_start != 0
                || !normalized_name[name_end..].trim().is_empty()
                || parse_atomic_or_named_type(name).is_some()
            {
                return Err(invalid_syntax("expected a type."));
            }

            Ok(TypeSyntaxItem::New(interner.intern(name)))
        }
        _ => Err(invalid_syntax(format!(
            "unknown annotation directive `@{directive_name}`. expected one of `@type`, `@alias`, `@if-unknown`, `@trust`, or `@new`."
        ))),
    }
}

fn annotation_surface_text(text: &str) -> Option<&str> {
    parse_compact_annotation_directive(text)
        .ok()
        .flatten()
        .map(|(_, surface_text)| surface_text)
        .or_else(|| Some(text.trim()))
}

fn parse_compact_annotation_kind_and_surface_text(
    text: &str,
) -> Result<(AnnotationKind, &str), TypeParseError> {
    Ok(parse_compact_annotation_directive(text)?.unwrap_or((AnnotationKind::Checked, text)))
}

fn parse_compact_annotation_directive(
    text: &str,
) -> Result<Option<(AnnotationKind, &str)>, TypeParseError> {
    let Some((directive_name, directive_body)) = parse_annotation_directive_name_and_body(text)
    else {
        return Ok(None);
    };

    match directive_name {
        "if-unknown" => {
            let surface_text = keyword_surface_text(directive_body)
                .ok_or_else(|| invalid_syntax("expected a type after the annotation prefix."))?;
            Ok(Some((AnnotationKind::UnknownOnly, surface_text)))
        }
        "trust" => {
            let surface_text = keyword_surface_text(directive_body)
                .ok_or_else(|| invalid_syntax("expected a type after the annotation prefix."))?;
            Ok(Some((AnnotationKind::Trusted, surface_text)))
        }
        _ => Ok(None),
    }
}

fn keyword_surface_text(text: &str) -> Option<&str> {
    let trimmed_text = text.trim();
    if trimmed_text.is_empty() {
        None
    } else {
        Some(trimmed_text)
    }
}

fn parse_annotation_directive_name_and_body(text: &str) -> Option<(&str, &str)> {
    let trimmed_text = text.trim();
    let remainder = trimmed_text.strip_prefix('@')?;
    let directive_end = remainder
        .char_indices()
        .find(|(_, character)| character.is_whitespace())
        .map(|(index, _)| index)
        .unwrap_or(remainder.len());
    let directive_name = &remainder[..directive_end];
    let directive_body = remainder[directive_end..].trim();
    Some((directive_name, directive_body))
}

pub fn parse_expanded_block_surface_type(
    interner: &mut Interner,
    text: &str,
) -> Result<SurfaceType, TypeParseError> {
    let trimmed_text = text.trim();

    if trimmed_text.is_empty() {
        return Err(invalid_syntax(
            "expected an expanded type annotation block, but found empty input.",
        ));
    }

    let mut named_parameters = Vec::new();
    let mut return_type = SurfaceType::Null;
    let mut seen_return = false;

    for_each_expanded_annotation_directive(trimmed_text, |directive| {
        if let Some(parameter_text) = directive.strip_prefix("@param") {
            if seen_return {
                return Err(invalid_syntax(
                    "`@param` directives must appear before `@return` or `@returns` in the same `#:` block.",
                ));
            }
            let (type_text, name_text) = parse_braced_type_and_tail(parameter_text.trim())
                .ok_or_else(|| {
                    invalid_syntax("expected `@param {TYPE} name` in the expanded annotation.")
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
            if seen_return {
                return Err(invalid_syntax(
                    "cannot use more than one `@return` or `@returns` directive in the same `#:` block.",
                ));
            }
            let return_text = if let Some(return_text) = directive.strip_prefix("@returns") {
                return_text.trim()
            } else {
                directive.trim_start_matches("@return").trim()
            };
            let (type_text, trailing_text) = parse_braced_type_and_tail(return_text).ok_or_else(
                || {
                    invalid_syntax(
                        "expected `@return {TYPE}` or `@returns {TYPE}` in the expanded annotation.",
                    )
                },
            )?;
            if !trailing_text.trim().is_empty() {
                return Err(invalid_syntax(
                    "did not expect text after the return type in the expanded annotation.",
                ));
            }
            return_type = parse_surface_type(interner, type_text)?;
            seen_return = true;
        } else {
            return Err(invalid_syntax(
                "expected `@param`, `@return`, or `@returns` in the expanded annotation.",
            ));
        }

        Ok(())
    })?;

    Ok(SurfaceType::Function(FunctionType::new(
        Vec::new(),
        named_parameters,
        return_type,
    )))
}

fn is_expanded_annotation_line(text: &str) -> bool {
    if let Some((directive_name, _)) = parse_annotation_directive_name_and_body(text) {
        directive_name == "param" || directive_name == "return" || directive_name == "returns"
    } else {
        false
    }
}

fn parse_expanded_annotation_block_text(lines: &[&str]) -> Result<Option<String>, TypeParseError> {
    let Some(first_line) = lines.first().copied() else {
        return Ok(None);
    };

    if !is_expanded_annotation_line(first_line) {
        return Ok(None);
    }

    let mut expanded_block_text = String::from(first_line);

    for line in &lines[1..] {
        if !is_expanded_annotation_line(line) {
            return Err(invalid_syntax(
                "cannot mix compact and expanded annotations in the same `#:` block.",
            ));
        }

        expanded_block_text.push('\n');
        expanded_block_text.push_str(line);
    }

    Ok(Some(expanded_block_text))
}

fn for_each_expanded_annotation_directive(
    text: &str,
    mut visit_directive: impl FnMut(&str) -> Result<(), TypeParseError>,
) -> Result<(), TypeParseError> {
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
                    let missing_closer =
                        utils::expected_closer(*delimiter_stack.last().unwrap_or(&'{'));
                    return Err(invalid_syntax(format!(
                        "missing closing delimiter {missing_closer}"
                    )));
                }
                visit_directive(current_directive.trim())?;
                current_directive.clear();
            }
        } else if current_directive.is_empty() {
            return Err(invalid_syntax(
                "expected an expanded annotation directive starting with `@param`, `@return`, or `@returns`.",
            ));
        } else {
            current_directive.push('\n');
        }

        current_directive.push_str(content);

        for character in content.chars() {
            match character {
                '(' | '[' | '{' => delimiter_stack.push(character),
                ')' | ']' | '}' => {
                    if !utils::pop_matching_delimiter(&mut delimiter_stack, character) {
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
            "expected at least one expanded annotation directive.",
        ));
    }

    if !delimiter_stack.is_empty() {
        let missing_closer = utils::expected_closer(*delimiter_stack.last().unwrap_or(&'{'));
        return Err(invalid_syntax(format!(
            "missing closing delimiter {missing_closer}"
        )));
    }

    visit_directive(current_directive.trim())
}

fn parse_named_type_definition(
    interner: &mut Interner,
    text: &str,
) -> Result<(Symbol, SurfaceType), TypeParseError> {
    let trimmed_text = text.trim();

    if trimmed_text.is_empty() {
        return Err(invalid_syntax("expected a type name."));
    }

    let Some((name_start, name_end)) = identifier_span(trimmed_text) else {
        return Err(invalid_syntax("expected a type name."));
    };

    if name_start != 0 {
        return Err(invalid_syntax("expected a type name."));
    }

    let name_text = &trimmed_text[name_start..name_end];
    let remainder = trimmed_text[name_end..].trim();

    if remainder.is_empty() {
        return Err(invalid_syntax("expected `{TYPE}` after the type name."));
    }

    let (type_text, trailing_text) = parse_braced_type_and_tail(remainder)
        .ok_or_else(|| invalid_syntax("expected `{TYPE}` after the type name."))?;

    if !trailing_text.trim().is_empty() {
        return Err(invalid_syntax(
            "did not expect trailing text after the named type definition.",
        ));
    }

    let name = interner.intern(name_text);
    let surface_type = parse_surface_type(interner, type_text)?;
    Ok((name, surface_type))
}

fn identifier_span(text: &str) -> Option<(usize, usize)> {
    utils::identifier_span_at(text, 0)
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

            if matches!(surface_type, SurfaceType::Nullable(_)) {
                return Err(invalid_syntax(
                    "only nullable unions with a single `NULL` member are supported.",
                ));
            }

            self.position += 1;
            self.skip_ascii_whitespace();

            let right_type = self.parse_primary(stop_context)?;
            surface_type = match (surface_type, right_type) {
                (SurfaceType::Null, SurfaceType::Null) => {
                    return Err(invalid_syntax(
                        "user-facing type syntax does not allow `NULL | NULL`.",
                    ));
                }
                (SurfaceType::Null, right_type) => SurfaceType::Nullable(Box::new(right_type)),
                (left_type, SurfaceType::Null) => SurfaceType::Nullable(Box::new(left_type)),
                (_left_type, _right_type) => {
                    return Err(invalid_syntax(
                        "only nullable unions with a single `NULL` member are supported.",
                    ));
                }
            };
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
                "expected `[` or `{` after `list` in a list type.",
            ));
        }

        if self.consume_keyword("fn") {
            self.skip_ascii_whitespace();
            if !self.consume_byte(b'(') {
                return Err(invalid_syntax("expected `(` after `fn`."));
            }
            return self.parse_function_type();
        }

        let identifier_span = self
            .parse_identifier_span()
            .ok_or_else(|| invalid_syntax("expected a type."))?;
        let identifier = &self.source[identifier_span.0..identifier_span.1];

        let mut surface_type = parse_atomic_or_named_type(identifier)
            .unwrap_or_else(|| SurfaceType::Named(self.interner.intern(identifier)));

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
                return Err(invalid_syntax("expected `:` after `named` in `list[...]`."));
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
            self.expect_byte(b'{', "expected `{` after `list` in a list type.")?;
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
                                "cannot mix named and unnamed items in `list{...}`.",
                            ));
                        }
                        Some(ListBraceItemKind::Field) => {}
                    }

                    record_fields.push(RecordField::new(name, value));

                    self.skip_ascii_whitespace();
                    if self.consume_byte(b',') {
                        continue;
                    }
                    self.expect_byte(b'}', "expected `}` to close `list{...}`.")?;
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
                        "cannot mix named and unnamed items in `list{...}`.",
                    ));
                }
                Some(ListBraceItemKind::Item) => {}
            }

            tuple_items.push(value);

            self.skip_ascii_whitespace();
            if self.consume_byte(b',') {
                continue;
            }
            self.expect_byte(b'}', "expected `}` to close `list{...}`.")?;
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
                self.expect_byte(b')', "expected `)` to close `fn(...)`.")?;
                break;
            }
        }

        self.skip_ascii_whitespace();
        let return_type = if self.consume_byte(b'-') {
            self.expect_byte(b'>', "expected `>` after `-` in function return type.")?;
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
        let span = utils::identifier_span_at(self.source, self.position)?;
        self.position = span.1;
        Some(span)
    }

    fn peek_record_field_name(&self) -> Option<(usize, usize)> {
        let mut position = self.position;
        let remaining = &self.source[position..];

        for (index, character) in remaining.char_indices() {
            if character.is_whitespace() {
                position += character.len_utf8();
                continue;
            }

            position += index;
            break;
        }

        utils::identifier_span_at(self.source, position)
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

fn find_matching_closer(text: &str, opener: char, _closer: char) -> Option<usize> {
    let mut delimiter_stack = vec![opener];

    for (index, character) in text.char_indices() {
        match character {
            '(' | '[' | '{' => delimiter_stack.push(character),
            ')' | ']' | '}' => {
                if !utils::pop_matching_delimiter(&mut delimiter_stack, character) {
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

mod utils {
    pub(super) fn identifier_span_at(text: &str, start: usize) -> Option<(usize, usize)> {
        let remaining = &text[start..];
        let mut characters = remaining.char_indices();
        let (_, first_character) = characters.next()?;
        if !is_identifier_start(first_character) {
            return None;
        }

        let mut end = start + first_character.len_utf8();
        for (index, character) in characters {
            if is_identifier_continue(character) {
                end = start + index + character.len_utf8();
            } else {
                break;
            }
        }

        Some((start, end))
    }

    pub(super) fn pop_matching_delimiter(delimiter_stack: &mut Vec<char>, closer: char) -> bool {
        let Some(opener) = delimiter_stack.pop() else {
            return false;
        };

        matches!((opener, closer), ('(', ')') | ('[', ']') | ('{', '}'))
    }

    pub(super) fn expected_closer(opener: char) -> char {
        match opener {
            '(' => ')',
            '[' => ']',
            '{' => '}',
            _ => '}',
        }
    }

    fn is_identifier_start(character: char) -> bool {
        character == '_' || character.is_alphabetic()
    }

    fn is_identifier_continue(character: char) -> bool {
        character == '_' || character.is_alphanumeric()
    }
}
