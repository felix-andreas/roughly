use {
    crate::{
        hir::{Definition, DefinitionKind},
        interner::{Interner, Symbol},
        types::{
            Annotation, Atomic, BinderParameter, Constraint, FunctionType, NamedTypeRef,
            RecordField, SurfaceType, TypeAnnotationKind,
        },
    },
    ropey::Rope,
    tree_sitter::{Point, Range},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeSyntax {
    Annotation(Annotation),
    Definitions(Vec<Definition>),
}

// Recursive-descent depth bound for type-syntax parsing (`list[...]`, `fn(...)`, nested generic
// arguments). A pathologically nested annotation would otherwise recurse until the stack overflows
// before typecheck ever runs; this turns it into a clean diagnostic. It sits above the inference
// `RECURSION_LIMIT` (128) so a merely deep — but parseable — annotation is rejected by the inference
// "nested too deeply to check" guard rather than here, yet stays well below the ~200-frame overflow
// of the 2 MB worker/test-thread stack.
pub(crate) const TYPE_SYNTAX_RECURSION_LIMIT: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeParseError {
    InvalidSyntax { message: String },
    InvalidSemantics { message: String },
    UnsupportedConstruct { message: String },
    UnknownType { name: String },
    RecursionLimitExceeded { limit: usize },
}

pub fn render_type_syntax(item: &TypeSyntax, interner: &Interner) -> String {
    match item {
        TypeSyntax::Annotation(annotation) => render_annotation(annotation, interner),
        TypeSyntax::Definitions(definitions) => definitions
            .iter()
            .map(|definition| render_definition(definition, interner))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn render_annotation(annotation: &Annotation, interner: &Interner) -> String {
    match annotation {
        Annotation::Type { kind, surface_type } => match kind {
            TypeAnnotationKind::Checked => render_surface_type(surface_type, interner),
            TypeAnnotationKind::UnknownOnly => {
                format!(
                    "@if-unknown {}",
                    render_surface_type(surface_type, interner)
                )
            }
            TypeAnnotationKind::Trusted => {
                format!("@trust {}", render_surface_type(surface_type, interner))
            }
        },
        Annotation::New { nominal_type } => {
            format!("@new {}", render_named_type_ref(nominal_type, interner))
        }
    }
}

fn render_definition(definition: &Definition, interner: &Interner) -> String {
    let directive = match definition.kind {
        DefinitionKind::Type => "@type",
        DefinitionKind::Alias => "@alias",
    };
    let name = interner.resolve(definition.name).unwrap_or("<unknown>");
    let params = if definition.type_parameters.is_empty() {
        String::new()
    } else {
        let rendered_params = definition
            .type_parameters
            .iter()
            .map(|&p| interner.resolve(p).unwrap_or("<unknown>").to_owned())
            .collect::<Vec<_>>()
            .join(", ");
        format!("<{rendered_params}>")
    };
    format!(
        "{directive} {name}{params} {{{}}}",
        render_surface_type(&definition.surface_type, interner)
    )
}

pub fn render_surface_type(surface_type: &SurfaceType, interner: &Interner) -> String {
    match surface_type {
        SurfaceType::Any => "Any".to_owned(),
        SurfaceType::Unknown => "Unknown".to_owned(),
        SurfaceType::Null => "NULL".to_owned(),
        SurfaceType::Union(members) => members
            .iter()
            .map(|member| {
                let rendered = render_surface_type(member, interner);
                // A bare function member would render identically to a function *returning* a
                // union (`fn() -> integer | NULL` is ambiguous), so it is parenthesized.
                if matches!(member, SurfaceType::Function(_)) {
                    format!("({rendered})")
                } else {
                    rendered
                }
            })
            .collect::<Vec<_>>()
            .join(" | "),
        SurfaceType::Scalar(atomic) => match atomic {
            Atomic::Logical => "logical",
            Atomic::Integer => "integer",
            Atomic::Double => "double",
            Atomic::Complex => "complex",
            Atomic::Character => "character",
            Atomic::Raw => "raw",
        }
        .to_owned(),
        SurfaceType::Named(name, type_arguments) => {
            let base = interner.resolve(*name).unwrap_or("<unknown>").to_owned();
            if type_arguments.is_empty() {
                base
            } else {
                let rendered_args = type_arguments
                    .iter()
                    .map(|arg| render_surface_type(arg, interner))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{base}<{rendered_args}>")
            }
        }
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
                    let rendered_name = if parameter.optional {
                        format!("[{name}]")
                    } else {
                        name.to_owned()
                    };
                    format!(
                        "{rendered_name}: {}",
                        render_surface_type(&parameter.value, interner)
                    )
                })
                .collect::<Vec<_>>();
            let mut rendered_parts = rendered_parameters;
            rendered_parts.extend(rendered_named_parameters);
            if let Some(variadic_element) = &function_type.variadic {
                rendered_parts.push(format!(
                    "...: {}",
                    render_surface_type(variadic_element, interner)
                ));
            }
            format!(
                "fn({}) -> {}",
                rendered_parts.join(", "),
                render_surface_type(&function_type.return_type, interner)
            )
        }
        SurfaceType::Binders(type_parameters, inner_type) => {
            let rendered_params = type_parameters
                .iter()
                .map(|parameter| {
                    let name = interner.resolve(parameter.name).unwrap_or("<unknown>");
                    match parameter.constraint {
                        Constraint::Numeric => format!("{name}: numeric"),
                        Constraint::AtomicElement => format!("{name}: atomic"),
                        Constraint::ScalarNumeric | Constraint::Unconstrained => name.to_owned(),
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "<{rendered_params}> {}",
                render_surface_type(inner_type, interner)
            )
        }
    }
}

// The writable constraint vocabulary for `<T: ...>` / `@forall T: ...` binders. `numeric` and
// `atomic` match the display names inferred signatures use; the internal scalar-numeric meet is
// inference-only and not writable.
fn binder_constraint_from_name(name: &str) -> Option<Constraint> {
    match name {
        "numeric" => Some(Constraint::Numeric),
        "atomic" => Some(Constraint::AtomicElement),
        _ => None,
    }
}

pub fn render_named_type_ref(named_type_ref: &NamedTypeRef, interner: &Interner) -> String {
    let name = interner.resolve(named_type_ref.name).unwrap_or("<unknown>");
    if named_type_ref.type_arguments.is_empty() {
        name.to_owned()
    } else {
        let rendered_args = named_type_ref
            .type_arguments
            .iter()
            .map(|argument| render_surface_type(argument, interner))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{name}<{rendered_args}>")
    }
}

pub fn parse_type_syntax(
    text: &str,
    interner: &mut Interner,
) -> Result<TypeSyntax, TypeParseError> {
    let trimmed_text = text.trim();

    if trimmed_text.is_empty() {
        return Err(invalid_syntax(
            "expected a type annotation, but found empty input.",
        ));
    }

    let block_items = split_type_syntax_block(trimmed_text)?;

    let Some(first_item) = block_items.first() else {
        return Err(invalid_syntax(
            "expected a type annotation, but found empty input.",
        ));
    };

    match first_item.kind {
        BlockItemKind::Definition => {
            let mut definitions = Vec::with_capacity(block_items.len());
            for item in block_items {
                definitions.push(parse_definition_directive(&item.text, interner)?);
            }
            Ok(TypeSyntax::Definitions(definitions))
        }
        BlockItemKind::Expanded => {
            let expanded_block_text = block_items
                .into_iter()
                .map(|item| item.text)
                .collect::<Vec<_>>()
                .join("\n");
            let surface_type = parse_expanded_block_surface_type(&expanded_block_text, interner)?;
            Ok(TypeSyntax::Annotation(Annotation::checked(surface_type)))
        }
        BlockItemKind::Compact => parse_compact_annotation(&first_item.text, interner),
    }
}

pub fn parse_surface_type(
    text: &str,
    interner: &mut Interner,
) -> Result<SurfaceType, TypeParseError> {
    parse_annotation_type(text, interner, false)
}

pub fn parse_annotation_type(
    text: &str,
    interner: &mut Interner,
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

    validate_surface_type(&surface_type)?;

    Ok(surface_type)
}

// The role a token in the type notation plays, for editor highlighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeTokenRole {
    // A type name or type constructor keyword (`integer`, `list`, `fn`, `NULL`, `Any`, a nominal name).
    TypeName,
    // A type parameter, both at its `<...>` binder and at its uses.
    TypeParameter,
    // A parameter or field name that precedes a `:` (`x` in `x: integer`).
    ParameterName,
    // The `:` that separates a parameter or field name from its type.
    Separator,
    // The `->` return arrow.
    Operator,
    // The `...` rest-parameter marker.
    Variadic,
    // An `@`-directive keyword (`@type`, `@alias`, `@new`, `@param`, `@return`, `@forall`, …), including
    // its leading `@`.
    Directive,
}

// One classified token in the type notation, as a byte range into the text passed to
// [`semantic_tokens`] and its highlighting role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeToken {
    pub start: usize,
    pub end: usize,
    pub role: TypeTokenRole,
}

// Classifies the type-notation tokens in `text` for editor highlighting, returning their byte ranges
// and roles in source order.
//
// `text` is the body of a `#:` annotation (with the `#:` prefix already stripped) or a stub
// declaration line. This is a single lexing pass that shares the identifier and member-name lexers with
// the parser; it is a highlighter, not the parser — the parser discards per-token spans into interned
// symbols, so highlighting cannot recover them from a parsed `SurfaceType` and classifies the surface
// text directly. Classification is heuristic where the grammar is context-sensitive (an identifier is a
// parameter name when a `:` follows it, otherwise a type name), which is sufficient for coloring.
pub fn semantic_tokens(text: &str) -> Vec<TypeToken> {
    // What a `<...>` group means for the identifiers directly inside it. A type-parameter *binder*
    // (`<T> fn(...)`, `@type Wrapper<T> {...}`) declares type parameters; a generic *application*'s
    // argument list (`Wrapper<Person>`) holds ordinary type positions. Which one a `<` opens is decided
    // by what precedes it: after an identifier it is an application (except right after a `@type`/
    // `@alias` definition name, whose `<...>` declares that definition's parameters); anywhere else —
    // the start of the notation, after a stub declaration's `name :` separator — it is a binder.
    #[derive(Clone, Copy, PartialEq)]
    enum AngleContext {
        Binder,
        Application,
    }

    // The previous classified token, reduced to what the `<` decision and the `@type`-name tracking
    // need.
    #[derive(Clone, Copy, PartialEq)]
    enum PreviousToken {
        None,
        // A `@type`/`@alias` directive: the next identifier is the definition's name.
        DefinitionDirective,
        // The definition name right after `@type`/`@alias`: a following `<...>` binds its parameters.
        DefinitionName,
        Identifier,
        Separator,
        Other,
    }

    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut position = 0;
    let mut angle_stack: Vec<AngleContext> = Vec::new();
    let mut previous = PreviousToken::None;
    // Inside a `@param` directive, an identifier outside the braced `{TYPE}` is the parameter name
    // being annotated, not a type. Tracking the brace depth (rather than the position relative to the
    // braces) keeps this correct for both `@param {TYPE} name` and a flipped `@param name {TYPE}`.
    let mut in_param_directive = false;
    let mut brace_depth: usize = 0;

    while position < bytes.len() {
        let byte = bytes[position];

        if byte.is_ascii_whitespace() {
            position += 1;
            continue;
        }

        // An `@`-directive: the `@` plus the name of the directive, admitting interior `-` so
        // `@if-unknown` is a single token (the member-name lexer would stop at the dash).
        if byte == b'@' {
            let name_end = directive_name_end(text, position + 1);
            let name = &text[position + 1..name_end];
            tokens.push(TypeToken {
                start: position,
                end: name_end,
                role: TypeTokenRole::Directive,
            });
            in_param_directive = name == "param";
            previous = if name == "type" || name == "alias" {
                PreviousToken::DefinitionDirective
            } else {
                PreviousToken::Other
            };
            position = name_end;
            continue;
        }

        if text[position..].starts_with("...") {
            tokens.push(TypeToken {
                start: position,
                end: position + 3,
                role: TypeTokenRole::Variadic,
            });
            previous = PreviousToken::Other;
            position += 3;
            continue;
        }

        if text[position..].starts_with("->") {
            tokens.push(TypeToken {
                start: position,
                end: position + 2,
                role: TypeTokenRole::Operator,
            });
            previous = PreviousToken::Other;
            position += 2;
            continue;
        }

        if byte == b'<' {
            angle_stack.push(match previous {
                PreviousToken::Identifier => AngleContext::Application,
                _ => AngleContext::Binder,
            });
            previous = PreviousToken::Other;
            position += 1;
            continue;
        }
        if byte == b'>' {
            let _ = angle_stack.pop();
            previous = PreviousToken::Other;
            position += 1;
            continue;
        }

        if byte == b':' {
            tokens.push(TypeToken {
                start: position,
                end: position + 1,
                role: TypeTokenRole::Separator,
            });
            previous = PreviousToken::Separator;
            position += 1;
            continue;
        }

        // A member-name lexer admits interior dots, so a dotted parameter name (`na.rm`) is one token.
        if let Some((start, end)) = utils::member_name_span_at(text, position) {
            let role = if angle_stack.last() == Some(&AngleContext::Binder) {
                TypeTokenRole::TypeParameter
            } else if (in_param_directive && brace_depth == 0)
                || next_non_space_byte(bytes, end) == Some(b':')
            {
                // A `@param` name outside the type braces, or any name directly before `:`
                // (parameter and record-field positions), is a parameter name, not a type.
                TypeTokenRole::ParameterName
            } else {
                TypeTokenRole::TypeName
            };
            tokens.push(TypeToken { start, end, role });
            previous = match (previous, role) {
                (PreviousToken::DefinitionDirective, TypeTokenRole::TypeName) => {
                    PreviousToken::DefinitionName
                }
                _ => PreviousToken::Identifier,
            };
            position = end;
            continue;
        }

        if byte == b'{' {
            brace_depth += 1;
        }
        if byte == b'}' {
            brace_depth = brace_depth.saturating_sub(1);
        }
        // Any other byte (a delimiter such as `(`, `)`, `[`, `]`, `{`, `}`, `,`, or `|`) carries no
        // highlight; skip it.
        previous = PreviousToken::Other;
        position += 1;
    }

    tokens
}

// The end of a directive name starting at `start` (just past the `@`). Like an identifier but with
// interior `-` permitted, so `@if-unknown` lexes as one directive token; a trailing `-` is excluded.
fn directive_name_end(text: &str, start: usize) -> usize {
    let mut end = start;
    for (index, character) in text[start..].char_indices() {
        if character.is_alphanumeric() || character == '_' {
            end = start + index + character.len_utf8();
        } else if character != '-' {
            break;
        }
    }
    end
}

fn next_non_space_byte(bytes: &[u8], mut position: usize) -> Option<u8> {
    while let Some(&byte) = bytes.get(position) {
        if byte.is_ascii_whitespace() {
            position += 1;
        } else {
            return Some(byte);
        }
    }
    None
}

// A type-notation token located at its position in the document, rather than at a body-relative offset.
// This is what lets IDE features and diagnostics point at an individual type name inside a `#:`
// annotation without threading spans through the parser: the annotation's document text is re-lexed and
// each token is rebased to its document range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentTypeToken {
    pub range: Range,
    pub role: TypeTokenRole,
    pub text: String,
}

// Re-lexes the `#:` type notation covered by `annotation_range` and returns its tokens with document
// ranges. Reuses [`semantic_tokens`] as the single type-notation lexer: each source line in the range is
// stripped of its `#:` prefix, its body is lexed, and every token offset is rebased to the document by
// adding the body's start column on that line. A single token never spans lines, so per-line lexing is
// sufficient and multi-line annotation blocks work without a cross-line offset map.
pub fn type_tokens_in_range(rope: &Rope, annotation_range: Range) -> Vec<DocumentTypeToken> {
    let mut tokens = Vec::new();
    for row in annotation_range.start_point.row..=annotation_range.end_point.row {
        let Some(line) = rope.get_line(row) else {
            continue;
        };
        let line = line.to_string();
        let trimmed = line.trim_start();
        let Some(body) = trimmed.strip_prefix("#:") else {
            continue;
        };
        let leading_whitespace = line.len() - trimmed.len();
        let body_column = leading_whitespace + "#:".len();
        let Ok(line_start_byte) = rope.try_line_to_byte(row) else {
            continue;
        };

        for token in semantic_tokens(body) {
            let start_column = body_column + token.start;
            let end_column = body_column + token.end;
            tokens.push(DocumentTypeToken {
                range: Range {
                    start_byte: line_start_byte + start_column,
                    end_byte: line_start_byte + end_column,
                    start_point: Point::new(row, start_column),
                    end_point: Point::new(row, end_column),
                },
                role: token.role,
                text: body[token.start..token.end].to_string(),
            });
        }
    }
    tokens
}

// The document range of the first type-name token spelled `name` inside the `#:` annotation covered by
// `annotation_range`. Diagnostics about an offending type name (an unresolved name, for instance) use
// this to underline just that name rather than the whole annotation. `None` when the annotation does not
// mention the name as a type name (it should, since the diagnostic was raised from the same notation).
pub fn type_name_token_range(rope: &Rope, annotation_range: Range, name: &str) -> Option<Range> {
    type_tokens_in_range(rope, annotation_range)
        .into_iter()
        .find(|token| token.role == TypeTokenRole::TypeName && token.text == name)
        .map(|token| token.range)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockItemKind {
    Compact,
    Expanded,
    Definition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BlockItem {
    kind: BlockItemKind,
    text: String,
}

fn split_type_syntax_block(text: &str) -> Result<Vec<BlockItem>, TypeParseError> {
    let mut items = Vec::new();
    let mut current_kind = None;
    let mut current_text = String::new();
    let mut delimiter_stack = Vec::new();

    for raw_line in text.lines() {
        let trimmed_line = raw_line.trim();
        if trimmed_line.is_empty() {
            continue;
        }

        if current_kind.is_none() || delimiter_stack.is_empty() {
            let next_kind = classify_block_line(trimmed_line);

            if let Some(existing_kind) = current_kind {
                validate_block_item_sequence(existing_kind, next_kind)?;
                items.push(BlockItem {
                    kind: existing_kind,
                    text: std::mem::take(&mut current_text),
                });
            }

            current_kind = Some(next_kind);
            current_text.push_str(trimmed_line);
        } else {
            current_text.push('\n');
            current_text.push_str(trimmed_line);
        }

        update_delimiter_stack(trimmed_line, &mut delimiter_stack);
    }

    if let Some(kind) = current_kind {
        items.push(BlockItem {
            kind,
            text: current_text,
        });
    }

    Ok(items)
}

fn classify_block_line(text: &str) -> BlockItemKind {
    match parse_annotation_directive_name_and_body(text) {
        Some(("type", _)) | Some(("alias", _)) => BlockItemKind::Definition,
        _ if is_expanded_annotation_line(text) => BlockItemKind::Expanded,
        _ => BlockItemKind::Compact,
    }
}

fn validate_block_item_sequence(
    existing_kind: BlockItemKind,
    next_kind: BlockItemKind,
) -> Result<(), TypeParseError> {
    match (existing_kind, next_kind) {
        (BlockItemKind::Definition, BlockItemKind::Definition)
        | (BlockItemKind::Expanded, BlockItemKind::Expanded) => Ok(()),
        (BlockItemKind::Compact, BlockItemKind::Compact) => Err(invalid_semantics(
            "cannot use multiple compact annotations in the same `#:` block.",
        )),
        (BlockItemKind::Definition, _) | (_, BlockItemKind::Definition) => Err(invalid_semantics(
            "cannot mix definition and annotation directives in the same `#:` block.",
        )),
        (BlockItemKind::Compact, BlockItemKind::Expanded)
        | (BlockItemKind::Expanded, BlockItemKind::Compact) => Err(invalid_semantics(
            "cannot mix compact and expanded annotations in the same `#:` block.",
        )),
    }
}

fn update_delimiter_stack(text: &str, delimiter_stack: &mut Vec<char>) {
    for character in text.chars() {
        match character {
            '(' | '[' | '{' => delimiter_stack.push(character),
            ')' | ']' | '}' => {
                let _ = utils::pop_matching_delimiter(delimiter_stack, character);
            }
            _ => {}
        }
    }
}

fn parse_compact_annotation(
    text: &str,
    interner: &mut Interner,
) -> Result<TypeSyntax, TypeParseError> {
    if parse_annotation_directive_name_and_body(text).is_some() {
        match parse_directive_syntax(text, interner)? {
            annotation @ TypeSyntax::Annotation(_) => return Ok(annotation),
            TypeSyntax::Definitions(_) => {
                return Err(invalid_syntax(
                    "definition directives must not be mixed with annotations in the same `#:` block.",
                ));
            }
        }
    }

    let surface_type = parse_surface_type(text, interner)?;
    Ok(TypeSyntax::Annotation(Annotation::checked(surface_type)))
}

fn validate_surface_type(surface_type: &SurfaceType) -> Result<(), TypeParseError> {
    match surface_type {
        SurfaceType::Named(_, args) => {
            // Arity against the declaration is a resolution-time check (`typecheck` has the
            // definition environment); the parser validates only structure.
            for arg in args {
                validate_surface_type(arg)?;
            }
        }
        SurfaceType::Union(members) => {
            for member in members {
                validate_surface_type(member)?;
            }
        }
        SurfaceType::Vector(inner)
        | SurfaceType::NamedVector(inner)
        | SurfaceType::List(inner)
        | SurfaceType::NamedList(inner) => validate_surface_type(inner)?,
        SurfaceType::Record(fields) => {
            for field in fields {
                validate_surface_type(&field.value)?;
            }
        }
        SurfaceType::Tuple(items) => {
            for item in items {
                validate_surface_type(item)?;
            }
        }
        SurfaceType::Function(func) => {
            for param in &func.parameters {
                validate_surface_type(param)?;
            }
            for param in &func.named_parameters {
                validate_surface_type(&param.value)?;
            }
            validate_surface_type(&func.return_type)?;
        }
        SurfaceType::Binders(_, inner) => validate_surface_type(inner)?,
        SurfaceType::Any | SurfaceType::Unknown | SurfaceType::Null | SurfaceType::Scalar(_) => {}
    }
    Ok(())
}

fn parse_directive_syntax(
    text: &str,
    interner: &mut Interner,
) -> Result<TypeSyntax, TypeParseError> {
    let (directive_name, directive_body) = parse_annotation_directive_name_and_body(text)
        .ok_or_else(|| invalid_syntax("expected a type."))?;

    match directive_name {
        "type" => {
            let (name, type_parameters, surface_type) =
                parse_named_type_definition(directive_body, interner)?;
            Ok(TypeSyntax::Definitions(vec![Definition {
                kind: DefinitionKind::Type,
                name,
                type_parameters,
                surface_type,
            }]))
        }
        "alias" => {
            let (name, type_parameters, surface_type) =
                parse_named_type_definition(directive_body, interner)?;
            Ok(TypeSyntax::Definitions(vec![Definition {
                kind: DefinitionKind::Alias,
                name,
                type_parameters,
                surface_type,
            }]))
        }
        "if-unknown" => {
            let surface_text = keyword_surface_text(directive_body)
                .ok_or_else(|| invalid_syntax("expected a type after the annotation prefix."))?;
            let surface_type = parse_surface_type(surface_text, interner)?;
            Ok(TypeSyntax::Annotation(Annotation::unknown_only(
                surface_type,
            )))
        }
        "trust" => {
            let surface_text = keyword_surface_text(directive_body)
                .ok_or_else(|| invalid_syntax("expected a type after the annotation prefix."))?;
            let surface_type = parse_surface_type(surface_text, interner)?;
            Ok(TypeSyntax::Annotation(Annotation::trusted(surface_type)))
        }
        "new" => {
            if directive_body.is_empty() {
                return Err(invalid_syntax(
                    "expected a type after the annotation prefix.",
                ));
            }

            let nominal_type =
                parse_named_type_ref(directive_body, interner).map_err(|error| match error {
                    TypeParseError::InvalidSyntax { message } if message == "expected a type." => {
                        invalid_syntax("expected a nominal type reference after `@new`.")
                    }
                    other_error => other_error,
                })?;

            Ok(TypeSyntax::Annotation(Annotation::new(nominal_type)))
        }
        _ => Err(invalid_syntax(format!(
            "unknown annotation directive `@{directive_name}`. expected one of `@type`, `@alias`, `@if-unknown`, `@trust`, or `@new`."
        ))),
    }
}

fn parse_definition_directive(
    text: &str,
    interner: &mut Interner,
) -> Result<Definition, TypeParseError> {
    match parse_directive_syntax(text, interner)? {
        TypeSyntax::Definitions(definitions) => definitions
            .into_iter()
            .next()
            .ok_or_else(|| invalid_syntax("expected `@type` or `@alias` in a definition block.")),
        _ => Err(invalid_syntax(
            "expected `@type` or `@alias` in a definition block.",
        )),
    }
}

fn annotation_surface_text(text: &str) -> Option<&str> {
    parse_compact_annotation_directive(text)
        .ok()
        .flatten()
        .map(|(_, surface_text)| surface_text)
        .or_else(|| Some(text.trim()))
}

fn parse_compact_annotation_directive(
    text: &str,
) -> Result<Option<(TypeAnnotationKind, &str)>, TypeParseError> {
    let Some((directive_name, directive_body)) = parse_annotation_directive_name_and_body(text)
    else {
        return Ok(None);
    };

    match directive_name {
        "if-unknown" => {
            let surface_text = keyword_surface_text(directive_body)
                .ok_or_else(|| invalid_syntax("expected a type after the annotation prefix."))?;
            Ok(Some((TypeAnnotationKind::UnknownOnly, surface_text)))
        }
        "trust" => {
            let surface_text = keyword_surface_text(directive_body)
                .ok_or_else(|| invalid_syntax("expected a type after the annotation prefix."))?;
            Ok(Some((TypeAnnotationKind::Trusted, surface_text)))
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
    text: &str,
    interner: &mut Interner,
) -> Result<SurfaceType, TypeParseError> {
    let trimmed_text = text.trim();

    if trimmed_text.is_empty() {
        return Err(invalid_syntax(
            "expected an expanded type annotation block, but found empty input.",
        ));
    }

    let mut type_parameters = Vec::new();
    let mut named_parameters = Vec::new();
    let mut return_type = SurfaceType::Null;
    let mut seen_return = false;

    for_each_expanded_annotation_directive(trimmed_text, |directive| {
        if let Some(forall_text) = directive.strip_prefix("@forall") {
            if !named_parameters.is_empty() || seen_return {
                return Err(invalid_semantics(
                    "`@forall` directives must appear before `@param`, `@return`, or `@returns` in the same `#:` block.",
                ));
            }
            let forall_text = forall_text.trim();
            if forall_text.is_empty() {
                return Err(invalid_syntax(
                    "expected a type parameter name after `@forall`.",
                ));
            }
            for param in forall_text.split(',') {
                let param = param.trim();
                if param.is_empty() {
                    return Err(invalid_syntax(
                        "expected a type parameter name in `@forall`.",
                    ));
                }
                // `@forall T` or `@forall T: numeric` — the same constraint vocabulary as the
                // compact `<T: numeric>` binder.
                let (name_text, constraint) = match param.split_once(':') {
                    Some((name_text, constraint_text)) => {
                        let constraint_text = constraint_text.trim();
                        let constraint = binder_constraint_from_name(constraint_text)
                            .ok_or_else(|| {
                                invalid_semantics(format!(
                                    "unknown type-parameter constraint `{constraint_text}`; the available constraints are `numeric` and `atomic`."
                                ))
                            })?;
                        (name_text.trim(), constraint)
                    }
                    None => (param, Constraint::Unconstrained),
                };
                let param_symbol = interner.intern(name_text);
                if type_parameters
                    .iter()
                    .any(|parameter: &BinderParameter| parameter.name == param_symbol)
                {
                    return Err(invalid_semantics(format!(
                        "duplicate type parameter name `{name_text}` in expanded function annotation."
                    )));
                }
                type_parameters.push(BinderParameter {
                    name: param_symbol,
                    constraint,
                });
            }
        } else if let Some(parameter_text) = directive.strip_prefix("@param") {
            if seen_return {
                return Err(invalid_semantics(
                    "`@param` directives must appear before `@return` or `@returns` in the same `#:` block.",
                ));
            }
            let parameter_text = parameter_text.trim();
            // The braced type comes after the name (`@param name {TYPE}`), matching `@type` and
            // `@alias` and keeping a wrapped multi-line type as the directive's trailing element.
            // The old JSDoc order gets a targeted migration error rather than a generic one.
            if parameter_text.starts_with('{') {
                return Err(invalid_syntax(
                    "`@param` takes the parameter name first: write `@param name {TYPE}`.",
                ));
            }
            let Some(brace_index) = parameter_text.find('{') else {
                return Err(invalid_syntax(
                    "expected `@param name {TYPE}` in the expanded annotation.",
                ));
            };
            let trimmed_name = parameter_text[..brace_index].trim();
            let is_optional = trimmed_name.starts_with('[') && trimmed_name.ends_with(']');
            let normalized_name = trimmed_name
                .strip_prefix('[')
                .and_then(|name| name.strip_suffix(']'))
                .unwrap_or(trimmed_name)
                .trim();
            if normalized_name.is_empty() {
                return Err(invalid_syntax(
                    "expected a parameter name after `@param`, before the braced type.",
                ));
            }
            if normalized_name.contains(char::is_whitespace) {
                return Err(invalid_syntax(
                    "a `@param` name must be a single identifier: write `@param name {TYPE}`.",
                ));
            }
            let (type_text, trailing_text) =
                parse_braced_type_and_tail(&parameter_text[brace_index..]).ok_or_else(|| {
                    invalid_syntax("expected `@param name {TYPE}` in the expanded annotation.")
                })?;
            if !trailing_text.trim().is_empty() {
                return Err(invalid_syntax(
                    "did not expect text after the parameter type in the expanded annotation.",
                ));
            }
            let name = interner.intern(normalized_name);
            let surface_type = parse_surface_type(type_text, interner)?;
            named_parameters.push(RecordField::with_optional(name, surface_type, is_optional));
        } else if directive.starts_with("@returns") || directive.starts_with("@return") {
            if seen_return {
                return Err(invalid_semantics(
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
            return_type = parse_surface_type(type_text, interner)?;
            seen_return = true;
        } else {
            return Err(invalid_syntax(
                "expected `@param`, `@return`, or `@returns` in the expanded annotation.",
            ));
        }

        Ok(())
    })?;

    let function_type =
        SurfaceType::Function(FunctionType::new(Vec::new(), named_parameters, return_type));

    if type_parameters.is_empty() {
        Ok(function_type)
    } else {
        Ok(SurfaceType::Binders(
            type_parameters,
            Box::new(function_type),
        ))
    }
}

fn is_expanded_annotation_line(text: &str) -> bool {
    if let Some((directive_name, _)) = parse_annotation_directive_name_and_body(text) {
        directive_name == "param"
            || directive_name == "return"
            || directive_name == "returns"
            || directive_name == "forall"
    } else {
        false
    }
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
            || content.starts_with("@returns")
            || content.starts_with("@forall");

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
                "expected an expanded annotation directive starting with `@forall`, `@param`, `@return`, or `@returns`.",
            ));
        } else {
            current_directive.push('\n');
        }

        current_directive.push_str(content);

        for character in content.chars() {
            match character {
                '(' | '[' | '{' => delimiter_stack.push(character),
                ')' | ']' | '}'
                    if !utils::pop_matching_delimiter(&mut delimiter_stack, character) =>
                {
                    return Err(invalid_syntax(format!(
                        "unexpected closing delimiter {character}"
                    )));
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
    text: &str,
    interner: &mut Interner,
) -> Result<(Symbol, Vec<Symbol>, SurfaceType), TypeParseError> {
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
    let mut remainder = trimmed_text[name_end..].trim();

    let mut type_parameters = Vec::new();
    if remainder.starts_with('<') {
        if let Some(end) = remainder.find('>') {
            let params_text = &remainder[1..end];
            for param in params_text.split(',') {
                let param = param.trim();
                if param.is_empty() {
                    return Err(invalid_syntax("expected a type parameter name."));
                }
                let param_symbol = interner.intern(param);
                if type_parameters.contains(&param_symbol) {
                    return Err(invalid_semantics(format!(
                        "duplicate type parameter name `{param}` in named type definition."
                    )));
                }
                type_parameters.push(param_symbol);
            }
            remainder = remainder[end + 1..].trim();
        } else {
            return Err(invalid_syntax("expected `>` to close type parameters."));
        }
    }

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
    let surface_type = parse_surface_type(type_text, interner)?;

    if matches!(surface_type, SurfaceType::Binders(_, _)) {
        return Err(invalid_syntax("expected a type."));
    }

    Ok((name, type_parameters, surface_type))
}

fn parse_named_type_ref(
    text: &str,
    interner: &mut Interner,
) -> Result<NamedTypeRef, TypeParseError> {
    let surface_type = parse_surface_type(text, interner)?;
    match surface_type {
        SurfaceType::Named(name, type_arguments) => Ok(NamedTypeRef::new(name, type_arguments)),
        _ => Err(invalid_syntax("expected a type.")),
    }
}

fn identifier_span(text: &str) -> Option<(usize, usize)> {
    utils::member_name_span_at(text, 0)
}

fn invalid_syntax(message: impl Into<String>) -> TypeParseError {
    TypeParseError::InvalidSyntax {
        message: message.into(),
    }
}

fn invalid_semantics(message: impl Into<String>) -> TypeParseError {
    TypeParseError::InvalidSemantics {
        message: message.into(),
    }
}

fn unsupported_construct(message: impl Into<String>) -> TypeParseError {
    TypeParseError::UnsupportedConstruct {
        message: message.into(),
    }
}

impl TypeParseError {
    fn with_context(self, context: impl AsRef<str>) -> Self {
        match self {
            Self::InvalidSyntax { message } => Self::InvalidSyntax {
                message: format!("{message} ({})", context.as_ref()),
            },
            Self::InvalidSemantics { message } => Self::InvalidSemantics {
                message: format!("{message} ({})", context.as_ref()),
            },
            Self::UnsupportedConstruct { message } => Self::UnsupportedConstruct {
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
    right_angle: bool,
}

impl StopContext {
    const ROOT: Self = Self {
        comma: false,
        right_bracket: false,
        right_brace: false,
        right_paren: false,
        right_angle: false,
    };

    const LIST_BRACE_ITEM: Self = Self {
        comma: true,
        right_bracket: false,
        right_brace: true,
        right_paren: false,
        right_angle: false,
    };

    const FUNCTION_PARAMETER: Self = Self {
        comma: true,
        right_bracket: false,
        right_brace: false,
        right_paren: true,
        right_angle: false,
    };

    const GENERIC_ARG: Self = Self {
        comma: true,
        right_bracket: false,
        right_brace: false,
        right_paren: false,
        right_angle: true,
    };

    fn stops_on(self, byte: u8) -> bool {
        (self.comma && byte == b',')
            || (self.right_bracket && byte == b']')
            || (self.right_brace && byte == b'}')
            || (self.right_paren && byte == b')')
            || (self.right_angle && byte == b'>')
    }
}

struct TypeParser<'a> {
    interner: &'a mut Interner,
    source: &'a str,
    position: usize,
    depth: usize,
}

impl<'a> TypeParser<'a> {
    fn new(interner: &'a mut Interner, source: &'a str) -> Self {
        Self {
            interner,
            source,
            position: 0,
            depth: 0,
        }
    }

    fn parse_type(&mut self) -> Result<SurfaceType, TypeParseError> {
        self.skip_ascii_whitespace();
        let mut type_parameters = Vec::new();

        if self.consume_byte(b'<') {
            loop {
                self.skip_ascii_whitespace();
                if self.consume_byte(b'>') {
                    break;
                }

                let span = self
                    .parse_identifier_span()
                    .ok_or_else(|| invalid_syntax("expected a type parameter name in `<...>`."))?;
                let param = &self.source[span.0..span.1];
                let param_symbol = self.interner.intern(param);

                if type_parameters
                    .iter()
                    .any(|parameter: &BinderParameter| parameter.name == param_symbol)
                {
                    return Err(invalid_semantics(format!(
                        "duplicate type parameter name `{param}` in `<...>`."
                    )));
                }

                // An optional declared constraint: `<T: numeric>` / `<T: atomic>`, the same
                // vocabulary inferred signatures display with.
                self.skip_ascii_whitespace();
                let mut constraint = Constraint::Unconstrained;
                if self.peek_byte() == Some(b':') {
                    self.consume_byte(b':');
                    self.skip_ascii_whitespace();
                    let constraint_span = self.parse_identifier_span().ok_or_else(|| {
                        invalid_syntax("expected a constraint name after `:` in `<...>`.")
                    })?;
                    let constraint_name = &self.source[constraint_span.0..constraint_span.1];
                    constraint = binder_constraint_from_name(constraint_name).ok_or_else(|| {
                        invalid_semantics(format!(
                            "unknown type-parameter constraint `{constraint_name}`; the available constraints are `numeric` and `atomic`."
                        ))
                    })?;
                }

                type_parameters.push(BinderParameter {
                    name: param_symbol,
                    constraint,
                });

                self.skip_ascii_whitespace();
                if self.consume_byte(b',') {
                    self.skip_ascii_whitespace();
                    if self.peek_byte() == Some(b'>') {
                        return Err(invalid_syntax("expected a type parameter name in `<...>`."));
                    }
                    continue;
                } else if self.peek_byte() == Some(b'>') {
                    self.consume_byte(b'>');
                    break;
                } else {
                    return Err(invalid_syntax("expected `,` or `>` in `<...>`."));
                }
            }
            if type_parameters.is_empty() {
                return Err(invalid_syntax(
                    "expected at least one type parameter in `<...>`.",
                ));
            }
        }

        let inner_type = self.parse_type_until(StopContext::ROOT)?;

        if type_parameters.is_empty() {
            Ok(inner_type)
        } else {
            Ok(SurfaceType::Binders(type_parameters, Box::new(inner_type)))
        }
    }

    fn parse_type_until(
        &mut self,
        stop_context: StopContext,
    ) -> Result<SurfaceType, TypeParseError> {
        if self.depth >= TYPE_SYNTAX_RECURSION_LIMIT {
            return Err(TypeParseError::RecursionLimitExceeded {
                limit: TYPE_SYNTAX_RECURSION_LIMIT,
            });
        }
        self.depth += 1;
        let result = self.parse_type_until_inner(stop_context);
        self.depth -= 1;
        result
    }

    fn parse_type_until_inner(
        &mut self,
        stop_context: StopContext,
    ) -> Result<SurfaceType, TypeParseError> {
        self.skip_ascii_whitespace();
        let first_member = self.parse_primary(stop_context)?;
        let mut members = vec![first_member];

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
            members.push(self.parse_primary(stop_context)?);
        }

        if members.len() > 1 && members.iter().all(|member| *member == SurfaceType::Null) {
            return Err(unsupported_construct(
                "`NULL | NULL` is not valid type syntax.",
            ));
        }

        if members.len() == 1 {
            return Ok(members.pop().expect("length was checked"));
        }
        Ok(SurfaceType::union_of(members))
    }

    fn parse_primary(&mut self, stop_context: StopContext) -> Result<SurfaceType, TypeParseError> {
        self.skip_ascii_whitespace();

        if self.peek_byte() == Some(b'<') {
            return Err(unsupported_construct(
                "higher-rank polymorphism is not supported. type parameter binders may only appear at the outermost level.",
            ));
        }

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

        // Type names admit interior dots like member names do: R's own class names are dotted
        // (`data.frame`, `POSIXct`), and stub `@type` declarations must be able to name them.
        let identifier_span = self
            .parse_member_name_span()
            .ok_or_else(|| invalid_syntax("expected a type."))?;
        let identifier = &self.source[identifier_span.0..identifier_span.1];

        let mut surface_type = parse_atomic_or_named_type(identifier);

        if surface_type.is_none() {
            let mut type_arguments = Vec::new();
            self.skip_ascii_whitespace();
            if self.consume_byte(b'<') {
                loop {
                    self.skip_ascii_whitespace();
                    if self.consume_byte(b'>') {
                        break;
                    }

                    type_arguments.push(self.parse_type_until(StopContext::GENERIC_ARG)?);

                    self.skip_ascii_whitespace();
                    if self.consume_byte(b',') {
                        self.skip_ascii_whitespace();
                        if self.peek_byte() == Some(b'>') {
                            return Err(invalid_syntax("expected a type."));
                        }
                        continue;
                    } else if self.peek_byte() == Some(b'>') {
                        self.consume_byte(b'>');
                        break;
                    } else {
                        return Err(invalid_syntax("expected `,` or `>` in type argument list."));
                    }
                }
                if type_arguments.is_empty() {
                    return Err(invalid_syntax(
                        "expected at least one type argument in generic type application.",
                    ));
                }
            }
            surface_type = Some(SurfaceType::Named(
                self.interner.intern(identifier),
                type_arguments,
            ));
        }

        let mut surface_type = surface_type.unwrap();

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
            right_angle: caller_stop_context.right_angle,
        })
    }

    fn parse_list_braces(&mut self) -> Result<SurfaceType, TypeParseError> {
        self.skip_ascii_whitespace();
        if self.consume_byte(b'}') {
            return Ok(SurfaceType::Tuple(Vec::new()));
        }

        let mut tuple_items = Vec::new();
        let mut record_fields: Vec<RecordField<SurfaceType>> = Vec::new();
        let mut item_kind = None;

        loop {
            self.skip_ascii_whitespace();

            // A trailing comma before the closing brace is allowed.
            if self.consume_byte(b'}') {
                break;
            }

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

                    if record_fields.iter().any(|field| field.name == name) {
                        return Err(invalid_semantics(format!(
                            "duplicate field `{field_name}` in `list{{...}}`."
                        )));
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
        let mut variadic = None;

        if !self.consume_byte(b')') {
            loop {
                self.skip_ascii_whitespace();

                // A rest parameter `...name: TYPE` (or bare `...` ≡ `...: Any`) makes the function
                // variadic. It must be the last parameter, so after parsing it the only legal token is
                // the closing `)`.
                if self.source[self.position..].starts_with("...") {
                    self.position += "...".len();
                    // An optional rest-parameter name is accepted for readability but discarded: the
                    // variadic carries only its element type, since rest arguments are matched by
                    // position, never by name.
                    let _ = self.parse_member_name_span();
                    self.skip_ascii_whitespace();
                    let element_type = if self.consume_byte(b':') {
                        self.parse_type_until(StopContext::FUNCTION_PARAMETER)
                            .map_err(|error| {
                                error.with_context("while parsing rest parameter type")
                            })?
                    } else {
                        SurfaceType::Any
                    };
                    variadic = Some(element_type);

                    self.skip_ascii_whitespace();
                    self.expect_byte(
                        b')',
                        "a `...` rest parameter must be the last parameter in `fn(...)`.",
                    )?;
                    break;
                }

                if self.consume_byte(b'[') {
                    let parsed_name = self.parse_member_name_span();
                    self.skip_ascii_whitespace();
                    let Some((start, end)) = parsed_name.filter(|_| self.consume_byte(b']')) else {
                        return Err(invalid_syntax(
                            "expected `[name]: TYPE` for an optional parameter.",
                        ));
                    };
                    self.skip_ascii_whitespace();
                    self.expect_byte(
                        b':',
                        "expected `:` after `[name]` in an optional parameter.",
                    )?;
                    let name = self.interner.intern(&self.source[start..end]);
                    let value = self
                        .parse_type_until(StopContext::FUNCTION_PARAMETER)
                        .map_err(|error| {
                            error.with_context("while parsing optional parameter type")
                        })?;
                    named_parameters.push(RecordField::optional(name, value));

                    self.skip_ascii_whitespace();
                    if self.consume_byte(b',') {
                        continue;
                    }
                    self.expect_byte(b')', "expected `)` to close `fn(...)`.")?;
                    break;
                }

                let parameter_start = self.position;

                let parsed_name = self.parse_member_name_span();
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

        Ok(SurfaceType::Function(FunctionType::with_variadic(
            parameters,
            named_parameters,
            variadic,
            return_type,
        )))
    }

    fn parse_identifier_span(&mut self) -> Option<(usize, usize)> {
        self.skip_ascii_whitespace();
        let span = utils::identifier_span_at(self.source, self.position)?;
        self.position = span.1;
        Some(span)
    }

    // Lexes a parameter, field, or type name, which may contain interior `.` (`na.rm`,
    // `data.frame`). Type-parameter binders keep the dot-free `identifier_span_at`.
    fn parse_member_name_span(&mut self) -> Option<(usize, usize)> {
        self.skip_ascii_whitespace();
        let span = utils::member_name_span_at(self.source, self.position)?;
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

        utils::member_name_span_at(self.source, position)
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
        if let Some(next_byte) = bytes.get(list_end).copied()
            && (next_byte == b'_' || next_byte.is_ascii_alphanumeric())
        {
            return false;
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
        if let Some(next_byte) = self.source.as_bytes().get(end).copied()
            && (next_byte == b'_' || next_byte.is_ascii_alphanumeric())
        {
            return false;
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

    // A parameter or field name: like an identifier but with interior `.` permitted (`na.rm`,
    // `length.out`). The leading character must still be a letter or `_`, and the name may not end in
    // `.` — the dot is interior only, so a trailing dot ends the span before it.
    pub(super) fn member_name_span_at(text: &str, start: usize) -> Option<(usize, usize)> {
        let remaining = &text[start..];
        let mut characters = remaining.char_indices();
        let (_, first_character) = characters.next()?;
        if !is_identifier_start(first_character) {
            return None;
        }

        // `end` tracks the last non-dot character, so a trailing `.` (which R identifiers do not use as
        // a final character) is excluded from the returned span rather than dangling in the name.
        let mut end = start + first_character.len_utf8();
        for (index, character) in characters {
            if is_identifier_continue(character) {
                end = start + index + character.len_utf8();
            } else if character != '.' {
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

#[cfg(test)]
mod semantic_token_tests {
    use super::{TypeToken, TypeTokenRole, semantic_tokens};

    // Renders each classified token as `slice=role` so the span and role are both asserted.
    fn rendered(text: &str) -> Vec<String> {
        semantic_tokens(text)
            .into_iter()
            .map(|TypeToken { start, end, role }| {
                let role = match role {
                    TypeTokenRole::TypeName => "type",
                    TypeTokenRole::TypeParameter => "typeparam",
                    TypeTokenRole::ParameterName => "param",
                    TypeTokenRole::Separator => "sep",
                    TypeTokenRole::Operator => "op",
                    TypeTokenRole::Variadic => "variadic",
                    TypeTokenRole::Directive => "directive",
                };
                format!("{}={role}", &text[start..end])
            })
            .collect()
    }

    #[test]
    fn classifies_named_parameters_and_return_arrow() {
        assert_eq!(
            rendered("fn(count: integer) -> logical"),
            [
                "fn=type",
                "count=param",
                ":=sep",
                "integer=type",
                "->=op",
                "logical=type"
            ]
        );
    }

    #[test]
    fn classifies_dotted_parameter_name_as_one_token() {
        assert_eq!(
            rendered("fn(x: double, na.rm: logical) -> double"),
            [
                "fn=type",
                "x=param",
                ":=sep",
                "double=type",
                "na.rm=param",
                ":=sep",
                "logical=type",
                "->=op",
                "double=type"
            ]
        );
    }

    #[test]
    fn classifies_type_parameters_at_binder_and_use() {
        assert_eq!(
            rendered("<T> fn(x: T) -> T"),
            [
                "T=typeparam",
                "fn=type",
                "x=param",
                ":=sep",
                "T=type",
                "->=op",
                "T=type"
            ]
        );
    }

    #[test]
    fn classifies_variadic_rest_parameter() {
        assert_eq!(
            rendered("fn(...: character) -> character"),
            [
                "fn=type",
                "...=variadic",
                ":=sep",
                "character=type",
                "->=op",
                "character=type"
            ]
        );
    }

    #[test]
    fn classifies_bare_value_type() {
        assert_eq!(rendered("double"), ["double=type"]);
    }

    #[test]
    fn classifies_directives_with_their_own_role() {
        assert_eq!(
            rendered("@type Person {list{name: character}}"),
            [
                "@type=directive",
                "Person=type",
                "list=type",
                "name=param",
                ":=sep",
                "character=type",
            ]
        );
        assert_eq!(rendered("@new Person"), ["@new=directive", "Person=type"]);
    }

    #[test]
    fn classifies_generic_application_arguments_as_type_names() {
        assert_eq!(rendered("Wrapper<Person>"), ["Wrapper=type", "Person=type"]);
        assert_eq!(
            rendered("fn(x: Wrapper<Pair<integer, character>>) -> Person"),
            [
                "fn=type",
                "x=param",
                ":=sep",
                "Wrapper=type",
                "Pair=type",
                "integer=type",
                "character=type",
                "->=op",
                "Person=type"
            ]
        );
    }

    #[test]
    fn classifies_definition_type_parameters_as_binders() {
        assert_eq!(
            rendered("@type Wrapper<T> {list{value: T}}"),
            [
                "@type=directive",
                "Wrapper=type",
                "T=typeparam",
                "list=type",
                "value=param",
                ":=sep",
                "T=type",
            ]
        );
    }

    #[test]
    fn classifies_binder_after_stub_declaration_separator() {
        assert_eq!(
            rendered("lapply : <T, U> fn(x: list[T], f: fn(T) -> U) -> list[U]"),
            [
                "lapply=param",
                ":=sep",
                "T=typeparam",
                "U=typeparam",
                "fn=type",
                "x=param",
                ":=sep",
                "list=type",
                "T=type",
                "f=param",
                ":=sep",
                "fn=type",
                "T=type",
                "->=op",
                "U=type",
                "->=op",
                "list=type",
                "U=type",
            ]
        );
    }

    #[test]
    fn classifies_param_directive_name_as_parameter_in_either_order() {
        assert_eq!(
            rendered("@param {integer} count"),
            ["@param=directive", "integer=type", "count=param"]
        );
        assert_eq!(
            rendered("@param {list{name: character}} [record]"),
            [
                "@param=directive",
                "list=type",
                "name=param",
                ":=sep",
                "character=type",
                "record=param"
            ]
        );
        // The queued syntax flip puts the name first; the brace-depth rule classifies it the same way.
        assert_eq!(
            rendered("@param count {integer}"),
            ["@param=directive", "count=param", "integer=type"]
        );
    }

    #[test]
    fn lexes_if_unknown_as_one_directive_token() {
        assert_eq!(
            rendered("@if-unknown integer"),
            ["@if-unknown=directive", "integer=type"]
        );
    }
}
