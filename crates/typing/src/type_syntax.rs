use crate::{
    interner::Interner,
    types::{AnnotationKind, Atomic, FunctionType, RecordField, SurfaceType},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeParseError {
    InvalidSyntax,
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
        return Err(TypeParseError::InvalidSyntax);
    }

    let surface_text = if allow_annotation_kind_prefix {
        annotation_surface_text(trimmed_text).ok_or(TypeParseError::InvalidSyntax)?
    } else {
        trimmed_text
    };

    parse_surface_type_inner(interner, surface_text)
}

pub fn parse_annotation(
    interner: &mut Interner,
    text: &str,
) -> Result<crate::types::Annotation, TypeParseError> {
    let trimmed_text = text.trim();

    if trimmed_text.is_empty() {
        return Err(TypeParseError::InvalidSyntax);
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

    let surface_type = parse_surface_type_inner(interner, surface_text)?;
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
        return Err(TypeParseError::InvalidSyntax);
    }

    let mut named_parameters = Vec::new();
    let mut return_type = SurfaceType::Null;

    for line in trimmed_text.lines() {
        let trimmed_line = line.trim();
        if trimmed_line.is_empty() {
            continue;
        }
        let content = if let Some(content) = trimmed_line.strip_prefix("#:") {
            content.trim()
        } else {
            trimmed_line
        };

        if let Some(parameter_text) = content.strip_prefix("@param") {
            let (type_text, name_text) = parse_braced_type_and_tail(parameter_text.trim())
                .ok_or(TypeParseError::InvalidSyntax)?;
            let normalized_name = name_text
                .trim()
                .strip_prefix('[')
                .and_then(|name| name.strip_suffix(']'))
                .unwrap_or(name_text.trim());
            let name = interner.intern(normalized_name);
            let surface_type = parse_surface_type(interner, type_text)?;
            named_parameters.push(RecordField::new(name, surface_type));
        } else if content.starts_with("@returns") || content.starts_with("@return") {
            let return_text = if let Some(return_text) = content.strip_prefix("@returns") {
                return_text.trim()
            } else {
                content.trim_start_matches("@return").trim()
            };
            let (type_text, trailing_text) =
                parse_braced_type_and_tail(return_text).ok_or(TypeParseError::InvalidSyntax)?;
            if !trailing_text.trim().is_empty() {
                return Err(TypeParseError::InvalidSyntax);
            }
            return_type = parse_surface_type(interner, type_text)?;
        } else {
            return Err(TypeParseError::InvalidSyntax);
        }
    }

    Ok(SurfaceType::Function(FunctionType::new(
        Vec::new(),
        named_parameters,
        return_type,
    )))
}

fn parse_surface_type_inner(
    interner: &mut Interner,
    text: &str,
) -> Result<SurfaceType, TypeParseError> {
    let trimmed_text = text.trim();

    if trimmed_text.is_empty() {
        return Err(TypeParseError::InvalidSyntax);
    }

    if let Some(parameters_and_return) = trimmed_text.strip_prefix("fn(") {
        let closing_index = find_matching_closer(parameters_and_return, '(', ')')
            .ok_or(TypeParseError::InvalidSyntax)?;
        let parameters_text = &parameters_and_return[..closing_index];
        let trailing_text = parameters_and_return[closing_index + 1..].trim();
        let return_type = if let Some(return_text) = trailing_text.strip_prefix("->") {
            parse_surface_type_inner(interner, return_text.trim())?
        } else if trailing_text.is_empty() {
            SurfaceType::Null
        } else {
            return Err(TypeParseError::InvalidSyntax);
        };

        let mut parameters = Vec::new();
        let mut named_parameters = Vec::new();

        for parameter_text in split_top_level(parameters_text, ',') {
            if parameter_text.is_empty() {
                continue;
            }

            if let Some((name_text, type_text)) = split_once_top_level(parameter_text, ':') {
                let name_text = name_text.trim();
                if name_text.is_empty() {
                    return Err(TypeParseError::InvalidSyntax);
                }
                let name = interner.intern(name_text);
                named_parameters.push(RecordField::new(
                    name,
                    parse_surface_type_inner(interner, type_text)?,
                ));
            } else {
                parameters.push(parse_surface_type_inner(interner, parameter_text)?);
            }
        }

        return Ok(SurfaceType::Function(FunctionType::new(
            parameters,
            named_parameters,
            return_type,
        )));
    }

    if let Some(inner_text) = trimmed_text
        .strip_prefix("list[")
        .and_then(|inner_text| inner_text.strip_suffix(']'))
    {
        if let Some(value_text) = inner_text.strip_prefix("named:") {
            return Ok(SurfaceType::NamedList(Box::new(parse_surface_type_inner(
                interner,
                value_text.trim(),
            )?)));
        }

        return Ok(SurfaceType::List(Box::new(parse_surface_type_inner(
            interner, inner_text,
        )?)));
    }

    if let Some(inner_text) = trimmed_text
        .strip_prefix("list{")
        .and_then(|inner_text| inner_text.strip_suffix('}'))
    {
        let mut items = Vec::new();
        let mut fields = Vec::new();
        let mut saw_named_item = false;
        let mut saw_unnamed_item = false;

        for item_text in split_top_level(inner_text, ',') {
            if item_text.is_empty() {
                continue;
            }

            if let Some((name_text, value_text)) = split_once_top_level(item_text, ':') {
                let name_text = name_text.trim();
                if name_text.is_empty() {
                    return Err(TypeParseError::InvalidSyntax);
                }
                let name = interner.intern(name_text);
                fields.push(RecordField::new(
                    name,
                    parse_surface_type_inner(interner, value_text)?,
                ));
                saw_named_item = true;
            } else {
                items.push(parse_surface_type_inner(interner, item_text)?);
                saw_unnamed_item = true;
            }
        }

        if saw_named_item && saw_unnamed_item {
            return Err(TypeParseError::InvalidSyntax);
        }

        if saw_named_item {
            return Ok(SurfaceType::Record(fields));
        }

        return Ok(SurfaceType::Tuple(items));
    }

    if let Some((left_text, right_text)) = split_once_top_level(trimmed_text, '|') {
        let left_type = parse_surface_type_inner(interner, left_text)?;
        let right_type = parse_surface_type_inner(interner, right_text)?;
        return match (left_type, right_type) {
            (SurfaceType::Null, other_type) | (other_type, SurfaceType::Null) => {
                Ok(SurfaceType::Nullable(Box::new(other_type)))
            }
            _ => Err(TypeParseError::InvalidSyntax),
        };
    }

    if let Some(inner_text) = trimmed_text.strip_suffix("[named]") {
        return Ok(SurfaceType::NamedVector(Box::new(
            parse_surface_type_inner(interner, inner_text)?,
        )));
    }

    if let Some(inner_text) = trimmed_text.strip_suffix("[]") {
        return Ok(SurfaceType::Vector(Box::new(parse_surface_type_inner(
            interner, inner_text,
        )?)));
    }

    match trimmed_text {
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
        _ => Err(TypeParseError::InvalidSyntax),
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

fn split_top_level(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start_index = 0;
    let mut depth = 0;

    for (index, character) in text.char_indices() {
        match character {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                if depth == 0 {
                    return vec![text];
                }
                depth -= 1;
            }
            _ => {}
        }

        if character == separator && depth == 0 {
            parts.push(text[start_index..index].trim());
            start_index = index + character.len_utf8();
        }
    }

    if depth != 0 {
        return vec![text];
    }

    parts.push(text[start_index..].trim());
    parts
}

fn split_once_top_level(text: &str, separator: char) -> Option<(&str, &str)> {
    let mut depth = 0;

    for (index, character) in text.char_indices() {
        match character {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
            }
            _ => {}
        }

        if character == separator && depth == 0 {
            return Some((
                text[..index].trim(),
                text[index + character.len_utf8()..].trim(),
            ));
        }
    }

    if depth != 0 {
        return None;
    }

    None
}

fn find_matching_closer(text: &str, opener: char, closer: char) -> Option<usize> {
    let mut depth = 1;

    for (index, character) in text.char_indices() {
        if character == opener {
            depth += 1;
        } else if character == closer {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }

    None
}

fn parse_braced_type_and_tail(text: &str) -> Option<(&str, &str)> {
    let inner_text = text.strip_prefix('{')?;
    let closing_index = find_matching_closer(inner_text, '{', '}')?;
    let type_text = &inner_text[..closing_index];
    let trailing_text = &inner_text[closing_index + 1..];
    Some((type_text.trim(), trailing_text))
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
    use {
        super::{parse_surface_type, render_surface_type},
        crate::interner::Interner,
    };

    #[test]
    fn parses_named_list_of_tuple_like_lists() {
        let mut interner = Interner::new();
        let surface_type =
            parse_surface_type(&mut interner, "list[named:list{integer,character}]").unwrap();
        assert_eq!(
            render_surface_type(&interner, &surface_type),
            "list[named: list{integer, character}]"
        );
    }

    #[test]
    fn parses_function_returning_record_inside_record_field() {
        let mut interner = Interner::new();
        let surface_type = parse_surface_type(
            &mut interner,
            "list{render:fn(integer)->list{label:character}}",
        )
        .unwrap();
        assert_eq!(
            render_surface_type(&interner, &surface_type),
            "list{render: fn(integer) -> list{label: character}}"
        );
    }

    #[test]
    fn parses_nested_named_list_and_function_record_case() {
        let mut interner = Interner::new();
        let surface_type = parse_surface_type(
            &mut interner,
            "list{meta:list{items:list[named:list{integer,character}],render:fn(integer)->list{label:character}}}",
        )
        .unwrap();
        assert_eq!(
            render_surface_type(&interner, &surface_type),
            "list{meta: list{items: list[named: list{integer, character}], render: fn(integer) -> list{label: character}}}"
        );
    }
}
