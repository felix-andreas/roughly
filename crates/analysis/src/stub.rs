use {
    crate::{
        interner::{Interner, Symbol},
        type_syntax::{
            TypeParseError, TypeToken, TypeTokenRole, parse_surface_type, semantic_tokens,
        },
        types::SurfaceType,
    },
    tree_sitter::{Point, Range},
};

// One parsed declaration from a stub file: a name bound to the type expression declared for it. The
// range spans the declaration line within its stub source, so a later goto-definition into a stub can
// point at the declaration the way it would point at an ordinary binding.
#[derive(Debug, Clone)]
pub struct StubDeclaration {
    pub name: Symbol,
    pub surface_type: SurfaceType,
    pub range: Range,
    // Declared `name : @masked fn(...)`: calls to this function evaluate the arguments its rest
    // parameter absorbs inside a data mask (dplyr-style non-standard evaluation), so bare names
    // there are column references, not unresolved variables.
    pub masked: bool,
}

// One parsed type declaration from a stub file: `@type NAME` declares an opaque nominal type — a
// named type with no inspectable representation, for standard-library values the type grammar
// cannot describe structurally (`data.frame`, `connection`, `factor`, ...). Values of an opaque
// nominal come only from functions declared to return it, and it is compatible only with itself.
#[derive(Debug, Clone)]
pub struct StubTypeDeclaration {
    pub name: Symbol,
    pub range: Range,
}

// A declaration line that did not parse, reported with the (zero-based) line it occurred on so a stub
// author can locate it. A malformed line never aborts loading: the loader collects the valid
// declarations and surfaces the errors, so one bad line in a project override cannot block analysis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StubParseError {
    pub line: usize,
    pub message: String,
}

// Everything one stub source parses into: value declarations, opaque type declarations, and the
// per-line errors for what did not parse.
#[derive(Debug, Clone, Default)]
pub struct StubFile {
    pub values: Vec<StubDeclaration>,
    pub types: Vec<StubTypeDeclaration>,
    pub errors: Vec<StubParseError>,
}

// Parses a stub source into its declarations and the errors for lines that did not parse.
//
// A stub file is declaration-only: each non-blank, non-comment line is `name : <type-expr>`, where the
// name is an R identifier and the type expression reuses the `#:` annotation grammar verbatim (including
// `<T> fn(...)` generics). There is no place to write a function body, so "declaration-only" is enforced
// structurally rather than by convention. Blank lines and `#` comments (whole-line or trailing) are
// ignored. Repeating a name is permitted at the grammar level so overload sets can be adopted later
// without rewriting the corpus; the loader decides how repeats combine.
pub fn parse_stub_file(source: &str, interner: &mut Interner) -> StubFile {
    let mut stub_file = StubFile::default();

    let mut byte_offset = 0;
    for (line_index, raw_line) in source.lines().enumerate() {
        let line_start_byte = byte_offset;
        // `lines()` strips the newline, so advance past this line plus its separator for the next line's
        // start. This tracks byte offsets even though the parser itself is line-oriented.
        byte_offset += raw_line.len() + 1;

        let content = strip_comment(raw_line).trim();
        if content.is_empty() {
            continue;
        }

        if let Some(type_name) = content.strip_prefix("@type") {
            let type_name = type_name.trim();
            if type_name.is_empty() {
                stub_file.errors.push(StubParseError {
                    line: line_index,
                    message: "expected a type name after `@type`.".to_owned(),
                });
            } else if !is_stub_name(type_name) {
                stub_file.errors.push(StubParseError {
                    line: line_index,
                    message: format!("`{type_name}` is not a valid type name."),
                });
            } else {
                stub_file.types.push(StubTypeDeclaration {
                    name: interner.intern(type_name),
                    range: line_range(line_index, line_start_byte, raw_line),
                });
            }
            continue;
        }

        match parse_declaration_line(content, interner) {
            Ok((name, surface_type, masked)) => stub_file.values.push(StubDeclaration {
                name,
                surface_type,
                range: line_range(line_index, line_start_byte, raw_line),
                masked,
            }),
            Err(message) => stub_file.errors.push(StubParseError {
                line: line_index,
                message,
            }),
        }
    }

    stub_file
}

// The historical entry point: value declarations and errors only. Kept for callers that do not
// consume type declarations.
pub fn parse_stub_declarations(
    source: &str,
    interner: &mut Interner,
) -> (Vec<StubDeclaration>, Vec<StubParseError>) {
    let stub_file = parse_stub_file(source, interner);
    (stub_file.values, stub_file.errors)
}

// Splits a declaration into its name and type-expression halves at the first top-level `:`, then interns
// the name and parses the type expression. The `:` scan must not descend into a bracketed type such as
// `list[a: b]`, so it tracks delimiter depth; the first `:` at depth zero is the declaration separator.
fn parse_declaration_line(
    content: &str,
    interner: &mut Interner,
) -> Result<(Symbol, SurfaceType, bool), String> {
    let separator = top_level_colon(content)
        .ok_or_else(|| "expected `name : type` but found no `:` separator.".to_owned())?;

    let name_text = content[..separator].trim();
    let mut type_text = content[separator + 1..].trim();

    if name_text.is_empty() {
        return Err("expected a name before `:`.".to_owned());
    }
    if !is_stub_name(name_text) {
        return Err(format!("`{name_text}` is not a valid declaration name."));
    }

    let masked = if let Some(rest) = type_text.strip_prefix("@masked") {
        type_text = rest.trim_start();
        true
    } else {
        false
    };
    if type_text.is_empty() {
        return Err(format!("expected a type after `{name_text} :`."));
    }

    let surface_type =
        parse_surface_type(type_text, interner).map_err(|error| render_type_error(&error))?;
    if masked
        && !matches!(&surface_type, SurfaceType::Function(function) if function.variadic.is_some())
    {
        return Err(format!(
            "`@masked` on `{name_text}` requires a variadic function type — the mask covers the \
             arguments the `...` rest parameter absorbs."
        ));
    }
    Ok((interner.intern(name_text), surface_type, masked))
}

fn top_level_colon(content: &str) -> Option<usize> {
    let mut depth: usize = 0;
    for (byte_index, character) in content.char_indices() {
        match character {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth = depth.saturating_sub(1),
            ':' if depth == 0 => return Some(byte_index),
            _ => {}
        }
    }
    None
}

// A stub name is an R identifier: it may contain letters, digits, `.` and `_`, and must not start with a
// digit. This admits dotted base names such as `is.null` and `as.integer`.
fn is_stub_name(name: &str) -> bool {
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    if first.is_ascii_digit() {
        return false;
    }
    name.chars()
        .all(|character| character.is_alphanumeric() || character == '.' || character == '_')
}

// Classifies one stub-file line for editor highlighting: a `@type NAME` declaration yields the
// directive and the declared type name; a `name : <type-expr>` declaration yields the name (as a
// parameter-style token), the separator, and the type expression's own tokens. Offsets are byte
// ranges into `line`. Comments and blank lines yield nothing — the editor's comment highlighting
// already covers them.
pub fn stub_line_tokens(line: &str) -> Vec<TypeToken> {
    let content = strip_comment(line);
    let trimmed = content.trim_end();
    let leading = trimmed.len() - trimmed.trim_start().len();
    let body = trimmed.trim_start();
    if body.is_empty() {
        return Vec::new();
    }

    if let Some(type_name) = body.strip_prefix("@type") {
        let mut tokens = vec![TypeToken {
            start: leading,
            end: leading + "@type".len(),
            role: TypeTokenRole::Directive,
        }];
        let name = type_name.trim();
        if !name.is_empty() {
            let name_start = leading + body.len() - type_name.trim_start().len();
            tokens.push(TypeToken {
                start: name_start,
                end: name_start + name.len(),
                role: TypeTokenRole::TypeName,
            });
        }
        return tokens;
    }

    let Some(separator) = top_level_colon(body) else {
        return Vec::new();
    };
    let name = body[..separator].trim_end();
    let mut tokens = Vec::new();
    if !name.is_empty() {
        tokens.push(TypeToken {
            start: leading,
            end: leading + name.len(),
            role: TypeTokenRole::ParameterName,
        });
    }
    tokens.push(TypeToken {
        start: leading + separator,
        end: leading + separator + 1,
        role: TypeTokenRole::Separator,
    });
    let type_text = &body[separator + 1..];
    let type_offset = leading + separator + 1;
    for token in semantic_tokens(type_text) {
        tokens.push(TypeToken {
            start: type_offset + token.start,
            end: type_offset + token.end,
            role: token.role,
        });
    }
    tokens
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(index) => &line[..index],
        None => line,
    }
}

fn line_range(line_index: usize, line_start_byte: usize, line: &str) -> Range {
    Range {
        start_byte: line_start_byte,
        end_byte: line_start_byte + line.len(),
        start_point: Point {
            row: line_index,
            column: 0,
        },
        end_point: Point {
            row: line_index,
            column: line.len(),
        },
    }
}

fn render_type_error(error: &TypeParseError) -> String {
    match error {
        TypeParseError::InvalidSyntax { message }
        | TypeParseError::InvalidSemantics { message }
        | TypeParseError::UnsupportedConstruct { message } => message.clone(),
        TypeParseError::UnknownType { name } => format!("unknown type `{name}`."),
        TypeParseError::RecursionLimitExceeded { limit } => {
            format!("type nests deeper than the limit of {limit}.")
        }
    }
}
