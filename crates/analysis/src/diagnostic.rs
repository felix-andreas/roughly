use {
    crate::{
        interner::{Interner, Symbol},
        typecheck::{InferenceError, OperandExpectation, RECURSION_LIMIT, SubscriptKind},
        types::{
            Atomic, Constraint, CoreType, FunctionType, InferenceVariableId, QuantifiedVariable,
            TypeScheme,
        },
    },
    std::{collections::BTreeMap, fmt},
    tree_sitter::{Point, Range},
};

pub type DocumentDiagnostics = (std::path::PathBuf, Vec<Diagnostic>);
pub type Diagnostics = Vec<Diagnostic>;

pub fn render_diagnostics(source: &str, diagnostics: &[Diagnostic]) -> String {
    if diagnostics.is_empty() {
        return "No diagnostics.\n".to_owned();
    }

    let mut rendered = String::new();

    for (index, diagnostic) in diagnostics.iter().enumerate() {
        if index > 0 {
            rendered.push('\n');
        }
        rendered.push_str(&diagnostic.render(source));
    }

    rendered
}

pub fn render_core_type(interner: &Interner, core_type: &CoreType) -> String {
    let mut renderer = TypeRenderer::fixture(interner);
    renderer.render_core_type(core_type)
}

pub fn render_type_scheme(interner: &Interner, type_scheme: &TypeScheme) -> String {
    let mut renderer = TypeRenderer::fixture(interner);
    renderer.render_type_scheme(type_scheme)
}

// Renders a `TypeScheme` for user-facing surfaces (completion detail), naming its quantified
// variables `T`/`U`/… exactly as hover renders the same scheme; `render_type_scheme` above keeps the
// internal `?N` fixture style.
pub fn render_user_facing_scheme(interner: &Interner, type_scheme: &TypeScheme) -> String {
    let mut renderer = TypeRenderer::user_facing(interner);
    renderer.render_type_scheme(type_scheme)
}

// Renders a `CoreType` for display in user-facing surfaces (hover, inlay hints), presenting a function
// type's free inference variables as a quantified scheme so a generalized binding reads with a readable
// `<T, U>` binder and named type parameters (`<T> fn(x: T) -> T`) rather than raw variable ids. The IDE
// layer records a binding's inferred type as a `CoreType` (not a `TypeScheme`), so the free variables are
// collected here in first-occurrence order and quantified for the render only — this changes nothing
// about inference. A non-function type never shows a binder (a `<T> T` hover on a parameter use would
// misread as a polymorphic scheme); its variables still render with the user-facing `T`, `U`, … names.
pub fn render_generalized_type(interner: &Interner, core_type: &CoreType) -> String {
    if let CoreType::Function(function_type) = core_type {
        return render_function_signature(interner, function_type).label;
    }
    let mut renderer = TypeRenderer::user_facing(interner);
    renderer.render_core_type(core_type)
}

/// A function type rendered as a signature-help label, with the byte span of each parameter
/// (positional, then named, then the trailing `...` element when variadic) inside [`label`]. The label
/// is exactly what [`render_generalized_type`] produces for the same function type; spans are recorded
/// during that one render so the LSP layer can emit precise parameter-label offsets that can never
/// drift from the label text.
///
/// [`label`]: RenderedSignature::label
pub struct RenderedSignature {
    pub label: String,
    pub parameters: Vec<std::ops::Range<usize>>,
}

pub fn render_function_signature(
    interner: &Interner,
    function_type: &FunctionType<CoreType>,
) -> RenderedSignature {
    let mut renderer = TypeRenderer::user_facing(interner);

    // One renderer spans binder, parameters, and return type, so a type variable keeps a single name
    // across the whole signature (`<T, U> fn(x: list[T], f: fn(T) -> U) -> list[U]`); rendering the
    // fragments with separate renderers would restart the naming and collapse distinct variables.
    let mut free_variables = Vec::new();
    collect_function_free_variables(function_type, &mut free_variables);
    let quantified_variables = free_variables
        .into_iter()
        .map(|variable| QuantifiedVariable::new(variable, Constraint::Unconstrained))
        .collect::<Vec<_>>();
    let binder = renderer.register_quantified(&quantified_variables);

    let mut label = binder;
    if !label.is_empty() {
        label.push(' ');
    }
    label.push_str("fn(");
    let mut parameters = Vec::new();
    for part in renderer.render_function_parts(function_type) {
        if !parameters.is_empty() {
            label.push_str(", ");
        }
        let start = label.len();
        label.push_str(&part);
        parameters.push(start..label.len());
    }
    label.push_str(") -> ");
    label.push_str(&renderer.render_core_type(&function_type.return_type));

    RenderedSignature { label, parameters }
}

// Collects a type's inference variables in first-occurrence order (deduplicated), so the display
// renderer can name them `T, U, …` in reading order.
fn collect_free_variables(core_type: &CoreType, out: &mut Vec<InferenceVariableId>) {
    match core_type {
        CoreType::Variable(variable) => {
            if !out.contains(variable) {
                out.push(*variable);
            }
        }
        CoreType::List(inner) | CoreType::NamedList(inner) => collect_free_variables(inner, out),
        CoreType::Union(members) => {
            for member in members {
                collect_free_variables(member, out);
            }
        }
        CoreType::Nominal(_, arguments) | CoreType::Tuple(arguments) => {
            for argument in arguments {
                collect_free_variables(argument, out);
            }
        }
        CoreType::Record(fields) => {
            for field in fields {
                collect_free_variables(&field.value, out);
            }
        }
        CoreType::Function(function_type) => collect_function_free_variables(function_type, out),
        CoreType::Any
        | CoreType::Unknown
        | CoreType::Null
        | CoreType::Scalar(_)
        | CoreType::Vector(_)
        | CoreType::NamedVector(_) => {}
    }
}

fn collect_function_free_variables(
    function_type: &FunctionType<CoreType>,
    out: &mut Vec<InferenceVariableId>,
) {
    for parameter in &function_type.parameters {
        collect_free_variables(parameter, out);
    }
    for parameter in &function_type.named_parameters {
        collect_free_variables(&parameter.value, out);
    }
    if let Some(variadic) = &function_type.variadic {
        collect_free_variables(variadic, out);
    }
    collect_free_variables(&function_type.return_type, out);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: DiagnosticCode,
    pub message: String,
    pub range: Range,
}

impl Diagnostic {
    pub fn lint_error(range: Range, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code: DiagnosticCode::Lint,
            message: message.into(),
            range,
        }
    }

    pub fn lint_warning(range: Range, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            code: DiagnosticCode::Lint,
            message: message.into(),
            range,
        }
    }

    pub fn naming_warning(range: Range, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            code: DiagnosticCode::Naming,
            message: message.into(),
            range,
        }
    }

    pub fn naming_error(range: Range, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code: DiagnosticCode::Naming,
            message: message.into(),
            range,
        }
    }

    pub fn unused_warning(range: Range, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            code: DiagnosticCode::Unused,
            message: message.into(),
            range,
        }
    }

    pub fn syntax_error(range: Range, message: impl Into<String>) -> Self {
        let mut message = message.into();
        if !message.starts_with("Syntax Error: ") {
            message = format!("Syntax Error: {message}");
        }

        Self {
            severity: Severity::Error,
            code: DiagnosticCode::SyntaxError,
            message,
            range,
        }
    }

    pub fn type_error(range: Range, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code: DiagnosticCode::TypeError,
            message: message.into(),
            range,
        }
    }

    pub fn strict(range: Range, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            code: DiagnosticCode::Strict,
            message: message.into(),
            range,
        }
    }

    pub fn annotation_error(range: Range, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code: DiagnosticCode::AnnotationError,
            message: message.into(),
            range,
        }
    }

    pub fn missing_annotation_target(range: Range) -> Self {
        Self::annotation_error(
            range,
            "This `#:` type comment must be followed immediately by an expression.",
        )
    }

    pub fn invalid_annotation_syntax(range: Range) -> Self {
        Self::annotation_error(
            range,
            "This `#:` type comment contains invalid type syntax.",
        )
    }

    pub fn unknown_annotation_type(range: Range, name: impl Into<String>) -> Self {
        let name = name.into();
        Self::annotation_error(range, format!("I could not resolve type `{name}`."))
    }

    pub fn duplicate_annotation(range: Range) -> Self {
        Self::annotation_error(
            range,
            "This `#:` type comment cannot be followed by another `#:` type comment.",
        )
    }

    pub fn from_inference_error(
        error: &InferenceError,
        fallback_range: Range,
        interner: &Interner,
    ) -> Self {
        let (range, message) = match error {
            InferenceError::UnknownInferenceVariable(variable) => (
                fallback_range,
                format!(
                    "The type checker lost track of inference variable t{}.",
                    variable.0
                ),
            ),
            InferenceError::UnknownName {
                symbol,
                range,
                expression_id: _,
            } => {
                let name = interner.resolve(*symbol).unwrap_or("<unknown>");
                (*range, format!("I could not find `{name}` in scope."))
            }
            InferenceError::AliasCycle {
                symbol,
                range,
                expression_id: _,
            } => {
                let name = interner.resolve(*symbol).unwrap_or("<unknown>");
                (*range, format!("Type alias `{name}` expands in a cycle."))
            }
            InferenceError::ExpectedFunction {
                actual_type,
                range,
                expression_id: _,
            } => {
                let mut type_renderer = TypeRenderer::user_facing(interner);
                (
                    *range,
                    format!(
                        "expected function, found `{}`",
                        type_renderer.render(actual_type)
                    ),
                )
            }
            InferenceError::OccursCheckFailed {
                variable,
                in_type,
                range,
                expression_id: _,
            } => {
                let mut type_renderer = TypeRenderer::user_facing(interner);
                let variable_name = type_renderer.render_variable(*variable);
                (
                    range.unwrap_or(fallback_range),
                    format!(
                        "I cannot construct an infinite type: {variable_name} occurs inside `{}`.",
                        type_renderer.render(in_type)
                    ),
                )
            }
            InferenceError::TypeMismatch {
                expected,
                actual,
                range,
                expression_id: _,
            } => {
                let mut type_renderer = TypeRenderer::user_facing(interner);
                (
                    range.unwrap_or(fallback_range),
                    format!(
                        "expected `{}`, found `{}`",
                        type_renderer.render(expected),
                        type_renderer.render(actual)
                    ),
                )
            }
            InferenceError::UnresolvedAnnotationType { symbol } => {
                let name = interner.resolve(*symbol).unwrap_or("<unknown>");
                (
                    fallback_range,
                    format!("I could not resolve type `{name}`."),
                )
            }
            InferenceError::NoMatchingOverload {
                symbol,
                candidate_count,
                range,
                expression_id: _,
                first_error,
            } => {
                let name = interner.resolve(*symbol).unwrap_or("<unknown>");
                let mut message = format!(
                    "no overload of `{name}` matches these arguments — I tried all {candidate_count} declared signatures"
                );
                if let Some(first_error) = first_error {
                    let inner = Self::from_inference_error(first_error, *range, interner);
                    message.push_str(&format!(
                        "; the first candidate fails with: {}",
                        inner.message
                    ));
                }
                (*range, message)
            }
            InferenceError::AnnotationParameterNameMismatch { name, range } => {
                let name = interner.resolve(*name).unwrap_or("<unknown>");
                (
                    range.unwrap_or(fallback_range),
                    format!(
                        "this annotation names a parameter `{name}`, but the function does not define one — annotation parameter names must match the function's parameter names"
                    ),
                )
            }
            InferenceError::ConstraintViolation {
                constraint,
                actual,
                range,
                expression_id: _,
            } => {
                let mut type_renderer = TypeRenderer::user_facing(interner);
                let expected_description = match constraint {
                    Constraint::Unconstrained => "a value",
                    Constraint::Numeric => "a numeric value (`integer` or `double`)",
                };
                (
                    range.unwrap_or(fallback_range),
                    format!(
                        "expected {expected_description}, found `{}`",
                        type_renderer.render(actual)
                    ),
                )
            }
            InferenceError::InvalidOperand {
                expected,
                actual,
                range,
                expression_id: _,
            } => {
                let mut type_renderer = TypeRenderer::user_facing(interner);
                let expected_description = match expected {
                    OperandExpectation::Numeric => "a numeric value (`integer` or `double`)",
                    OperandExpectation::ScalarNumeric => {
                        "a scalar numeric value (`integer` or `double`)"
                    }
                    OperandExpectation::Logical => "a `logical` value",
                    OperandExpectation::Comparable => {
                        "a comparable value (numeric, `character`, or `logical`)"
                    }
                };
                (
                    *range,
                    format!(
                        "expected {expected_description}, found `{}`",
                        type_renderer.render(actual)
                    ),
                )
            }
            InferenceError::TupleLengthMismatch {
                expected,
                actual,
                range,
                expression_id: _,
            } => (
                range.unwrap_or(fallback_range),
                format!(
                    "This tuple has {} item(s), but {} item(s) were expected.",
                    actual, expected
                ),
            ),
            InferenceError::MixedListElements {
                range,
                expression_id: _,
            } => (
                range.unwrap_or(fallback_range),
                "All elements in `list(...)` must be either all named or all unnamed.".to_owned(),
            ),
            InferenceError::RecordFieldMismatch {
                expected_fields,
                actual_fields,
                range,
                expression_id: _,
            } => (
                range.unwrap_or(fallback_range),
                format!(
                    "This record has fields `{}`, but fields `{}` were expected.",
                    render_symbols(actual_fields, interner),
                    render_symbols(expected_fields, interner)
                ),
            ),
            InferenceError::FunctionArityMismatch {
                expected,
                actual,
                range,
                expression_id: _,
            } => (
                range.unwrap_or(fallback_range),
                format!(
                    "This call passes {actual} positional argument(s), but the function accepts {expected}."
                ),
            ),
            InferenceError::NamedParameterMismatch {
                expected_parameters,
                actual_parameters,
                range,
                expression_id: _,
            } => (
                range.unwrap_or(fallback_range),
                format!(
                    "This call uses named argument(s) {}, but the function accepts named parameter(s) {}.",
                    render_symbols(actual_parameters, interner),
                    render_symbols(expected_parameters, interner)
                ),
            ),
            InferenceError::NotAList {
                actual,
                range,
                expression_id: _,
            } => {
                let mut type_renderer = TypeRenderer::user_facing(interner);
                (
                    *range,
                    format!("expected a list, found `{}`", type_renderer.render(actual)),
                )
            }
            InferenceError::FieldDoesNotExist {
                field,
                container,
                range,
                expression_id: _,
            } => {
                let name = interner.resolve(*field).unwrap_or("<unknown>");
                let mut type_renderer = TypeRenderer::user_facing(interner);
                (
                    *range,
                    format!(
                        "field `{name}` does not exist in `{}`",
                        type_renderer.render(container)
                    ),
                )
            }
            InferenceError::PositionDoesNotExist {
                position,
                container,
                range,
                expression_id: _,
            } => {
                let mut type_renderer = TypeRenderer::user_facing(interner);
                (
                    *range,
                    format!(
                        "position {position} does not exist in `{}`",
                        type_renderer.render(container)
                    ),
                )
            }
            InferenceError::NonLiteralSubscript {
                container,
                by,
                range,
                expression_id: _,
            } => {
                let detail = match by {
                    SubscriptKind::Position => "position",
                    SubscriptKind::FieldName => "field name",
                };
                let mut type_renderer = TypeRenderer::user_facing(interner);
                (
                    *range,
                    format!(
                        "cannot index `{}` without a statically known {detail}",
                        type_renderer.render(container)
                    ),
                )
            }
            InferenceError::UnsupportedSubset {
                actual,
                range,
                expression_id: _,
            } => {
                let mut type_renderer = TypeRenderer::user_facing(interner);
                (
                    *range,
                    format!("`[` is not supported on `{}`", type_renderer.render(actual)),
                )
            }
            InferenceError::UnsupportedIndexShape {
                index_count,
                range,
                expression_id: _,
            } => (
                *range,
                match index_count {
                    0 => "indexing with an empty index (`x[]`) is not supported yet".to_owned(),
                    1 => "indexing with a named index argument is not supported yet".to_owned(),
                    count => format!(
                        "indexing with {count} indexes is not supported yet — matrix and data.frame subsetting is not modeled"
                    ),
                },
            ),
            InferenceError::RecursionLimitExceeded => (
                fallback_range,
                format!(
                    "This type is nested too deeply to check (more than {RECURSION_LIMIT} levels)."
                ),
            ),
        };

        Self::type_error(range, message)
    }

    pub fn render(&self, source: &str) -> String {
        let start = self.range.start_point;
        let end = self.range.end_point;
        let excerpt = excerpt_for_range(source, self.range);

        format!(
            "{severity}[{code}] {message}\n--> {start_line}:{start_column}-{end_line}:{end_column}\n{excerpt}\n",
            severity = self.severity,
            code = self.code,
            message = self.message,
            start_line = start.row + 1,
            start_column = start.column + 1,
            end_line = end.row + 1,
            end_column = end.column + 1,
            excerpt = excerpt,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl fmt::Display for Severity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error => formatter.write_str("error"),
            Self::Warning => formatter.write_str("warning"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticCode {
    Lint,
    Naming,
    // The unused (dead-store) check: an assignment whose value is never read.
    Unused,
    SyntaxError,
    TypeError,
    AnnotationError,
    Strict,
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lint => formatter.write_str("lint"),
            Self::Naming => formatter.write_str("naming"),
            Self::Unused => formatter.write_str("unused"),
            Self::SyntaxError => formatter.write_str("syntax-error"),
            Self::TypeError => formatter.write_str("type-error"),
            Self::AnnotationError => formatter.write_str("annotation-error"),
            Self::Strict => formatter.write_str("strict"),
        }
    }
}

pub fn point_label(point: Point) -> String {
    format!("{}:{}", point.row + 1, point.column + 1)
}

fn excerpt_for_range(source: &str, range: Range) -> String {
    let line_index = range.start_point.row;
    let line = source.lines().nth(line_index).unwrap_or("");

    if line.is_empty() {
        return "| <empty line>".to_owned();
    }

    format!("| {line}")
}

fn render_symbols(symbols: &[Symbol], interner: &Interner) -> String {
    if symbols.is_empty() {
        return "<none>".to_owned();
    }

    symbols
        .iter()
        .map(|symbol| format!("`{}`", interner.resolve(*symbol).unwrap_or("<unknown>")))
        .collect::<Vec<_>>()
        .join(", ")
}

// The one type-display renderer every surface goes through. Two styles exist for inference
// variables the render does not quantify: `UserFacing` names them from the same `T`, `U`, `V`, …
// pool as scheme binders (internal ids like `?1`/`type1` must never reach a user — diagnostics,
// hover, inlay hints, and signature help all use this style), while `Fixture` keeps the raw `?N`
// numbering that the internal typecheck fixture suites pin. Variable names are per-renderer state,
// so one renderer must span everything that has to share names (both sides of an expected/found
// message, a whole function signature) — a fresh renderer restarts the numbering.
struct TypeRenderer<'a> {
    interner: &'a Interner,
    variable_style: VariableRenderStyle,
    variable_names: BTreeMap<InferenceVariableId, String>,
    quantified_variable_names: BTreeMap<InferenceVariableId, String>,
    next_variable_index: usize,
}

#[derive(Clone, Copy)]
enum VariableRenderStyle {
    UserFacing,
    Fixture,
}

impl<'a> TypeRenderer<'a> {
    fn user_facing(interner: &'a Interner) -> Self {
        Self {
            interner,
            variable_style: VariableRenderStyle::UserFacing,
            variable_names: BTreeMap::new(),
            quantified_variable_names: BTreeMap::new(),
            next_variable_index: 0,
        }
    }

    fn fixture(interner: &'a Interner) -> Self {
        Self {
            interner,
            variable_style: VariableRenderStyle::Fixture,
            variable_names: BTreeMap::new(),
            quantified_variable_names: BTreeMap::new(),
            next_variable_index: 0,
        }
    }

    fn render_type_scheme(&mut self, type_scheme: &TypeScheme) -> String {
        let binder = self.register_quantified(&type_scheme.quantified_variables);
        let rendered_body = self.render_core_type(&type_scheme.body);

        if binder.is_empty() {
            rendered_body
        } else {
            format!("{binder} {rendered_body}")
        }
    }

    // Names the quantified variables and returns the rendered binder (`<T, U: numeric>`, or `""` when
    // there is nothing to quantify). The binder names come from the same pool as loose user-facing
    // variables and advance the shared counter, so a variable the binder does not cover can never
    // collide with a binder name later in the same render.
    fn register_quantified(&mut self, quantified_variables: &[QuantifiedVariable]) -> String {
        let quantified_names = quantified_variables
            .iter()
            .map(|quantified| {
                let name = quantified_variable_name(self.next_variable_index);
                self.next_variable_index += 1;
                self.quantified_variable_names
                    .insert(quantified.variable, name.clone());
                match quantified.constraint {
                    Constraint::Unconstrained => name,
                    Constraint::Numeric => format!("{name}: numeric"),
                }
            })
            .collect::<Vec<_>>();

        if quantified_names.is_empty() {
            String::new()
        } else {
            format!("<{}>", quantified_names.join(", "))
        }
    }

    fn render_core_type(&mut self, core_type: &CoreType) -> String {
        match core_type {
            CoreType::Any => "Any".to_owned(),
            CoreType::Unknown => "Unknown".to_owned(),
            CoreType::Null => "NULL".to_owned(),
            CoreType::Union(members) => members
                .iter()
                .map(|member| {
                    let rendered = self.render_core_type(member);
                    // A bare function member would render identically to a function *returning* a
                    // union (`fn() -> integer | NULL` is ambiguous), so it is parenthesized.
                    if matches!(member, CoreType::Function(_)) {
                        format!("({rendered})")
                    } else {
                        rendered
                    }
                })
                .collect::<Vec<_>>()
                .join(" | "),
            CoreType::Scalar(atomic) => render_atomic(*atomic).to_owned(),
            CoreType::Nominal(symbol, type_arguments) => {
                let name = self.interner.resolve(*symbol).unwrap_or("<unknown>");
                if type_arguments.is_empty() {
                    name.to_owned()
                } else {
                    let rendered_type_arguments = type_arguments
                        .iter()
                        .map(|type_argument| self.render_core_type(type_argument))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{name}<{rendered_type_arguments}>")
                }
            }
            CoreType::Vector(atomic) => format!("{}[]", render_atomic(*atomic)),
            CoreType::NamedVector(atomic) => format!("{}[named]", render_atomic(*atomic)),
            CoreType::List(item_type) => format!("list[{}]", self.render_core_type(item_type)),
            CoreType::NamedList(item_type) => {
                format!("list[named: {}]", self.render_core_type(item_type))
            }
            CoreType::Record(fields) => {
                let rendered_fields = fields
                    .iter()
                    .map(|field| {
                        let name = self.interner.resolve(field.name).unwrap_or("<unknown>");
                        format!("{name}: {}", self.render_core_type(&field.value))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("list{{{rendered_fields}}}")
            }
            CoreType::Tuple(items) => {
                let rendered_items = items
                    .iter()
                    .map(|item| self.render_core_type(item))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("list{{{rendered_items}}}")
            }
            CoreType::Function(function_type) => {
                format!(
                    "fn({}) -> {}",
                    self.render_function_parts(function_type).join(", "),
                    self.render_core_type(&function_type.return_type)
                )
            }
            CoreType::Variable(variable) => self.render_variable(*variable),
        }
    }

    // The rendered parameter parts of a function type, in display order: positional, named (optional
    // names bracketed), then the trailing `...` element when variadic. Shared by the plain `fn(...)`
    // render and the signature-help render, which additionally records each part's span in the label.
    fn render_function_parts(&mut self, function_type: &FunctionType<CoreType>) -> Vec<String> {
        let mut parts = Vec::new();
        for parameter in &function_type.parameters {
            parts.push(self.render_core_type(parameter));
        }
        for parameter in &function_type.named_parameters {
            let name = self.interner.resolve(parameter.name).unwrap_or("<unknown>");
            let rendered_name = if parameter.optional {
                format!("[{name}]")
            } else {
                name.to_owned()
            };
            parts.push(format!(
                "{rendered_name}: {}",
                self.render_core_type(&parameter.value)
            ));
        }
        if let Some(variadic_element) = &function_type.variadic {
            parts.push(format!("...: {}", self.render_core_type(variadic_element)));
        }
        parts
    }

    fn render_variable(&mut self, variable: InferenceVariableId) -> String {
        if let Some(name) = self.quantified_variable_names.get(&variable) {
            return name.clone();
        }
        if let Some(name) = self.variable_names.get(&variable) {
            return name.clone();
        }

        let name = match self.variable_style {
            VariableRenderStyle::UserFacing => {
                let name = quantified_variable_name(self.next_variable_index);
                self.next_variable_index += 1;
                name
            }
            // Fixture numbering counts only the loose variables themselves, so a raw-metavariable
            // snapshot is unaffected by how many binder names a scheme registered before it.
            VariableRenderStyle::Fixture => format!("?{}", self.variable_names.len() + 1),
        };
        self.variable_names.insert(variable, name.clone());
        name
    }

    fn render(&mut self, core_type: &CoreType) -> String {
        self.render_core_type(core_type)
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

fn quantified_variable_name(index: usize) -> String {
    const QUANTIFIED_NAMES: [&str; 7] = ["T", "U", "V", "W", "X", "Y", "Z"];

    QUANTIFIED_NAMES
        .get(index)
        .map(|name| (*name).to_owned())
        .unwrap_or_else(|| format!("T{}", index + 1))
}
