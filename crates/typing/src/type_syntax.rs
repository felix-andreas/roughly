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

    let _surface_text = if allow_annotation_kind_prefix {
        annotation_surface_text(trimmed_text)
            .ok_or_else(|| invalid_syntax("Expected a type after the annotation prefix."))?
    } else {
        trimmed_text
    };

    let _ = interner;

    Err(invalid_syntax(
        "TODO: replace the current type-syntax stub with a recursive-descent parser.",
    ))
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
