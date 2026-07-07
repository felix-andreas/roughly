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
    render_generalized_type_with_constraints(interner, core_type, &|_| Constraint::Unconstrained)
}

// Like [`render_generalized_type`], but the caller supplies each display-quantified variable's
// constraint (looked up from the stored typecheck output), so a numeric parameter renders
// `<T: numeric>` instead of a bare `<T>`.
pub fn render_generalized_type_with_constraints(
    interner: &Interner,
    core_type: &CoreType,
    constraint_of: &dyn Fn(InferenceVariableId) -> Constraint,
) -> String {
    if let CoreType::Function(function_type) = core_type {
        return render_function_signature_with_constraints(interner, function_type, constraint_of)
            .label;
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
    render_function_signature_with_constraints(interner, function_type, &|_| {
        Constraint::Unconstrained
    })
}

pub fn render_function_signature_with_constraints(
    interner: &Interner,
    function_type: &FunctionType<CoreType>,
    constraint_of: &dyn Fn(InferenceVariableId) -> Constraint,
) -> RenderedSignature {
    let mut renderer = TypeRenderer::user_facing(interner);

    // One renderer spans binder, parameters, and return type, so a type variable keeps a single name
    // across the whole signature (`<T, U> fn(x: list[T], f: fn(T) -> U) -> list[U]`); rendering the
    // fragments with separate renderers would restart the naming and collapse distinct variables.
    let mut free_variables = Vec::new();
    collect_function_free_variables(function_type, &mut free_variables);
    let quantified_variables = free_variables
        .into_iter()
        .map(|variable| QuantifiedVariable::new(variable, constraint_of(variable)))
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
        CoreType::Vector(element) | CoreType::NamedVector(element) => {
            collect_free_variables(element, out)
        }
        CoreType::Any | CoreType::Unknown | CoreType::Null | CoreType::Scalar(_) => {}
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
        collect_free_variables(&variadic.element, out);
    }
    collect_free_variables(&function_type.return_type, out);
}

// The closest candidate to a misspelled name, for "did you mean" hints on unresolved references:
// the lexicographically first candidate at the smallest edit distance, when that distance is small
// relative to the name (≤ 1 for short names, ≤ 2 from length 5). Short names skip the hint —
// almost everything is within distance 2 of `x`.
pub fn nearest_name<'a>(name: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    if name.chars().count() < 3 {
        return None;
    }
    let budget = if name.chars().count() >= 5 { 2 } else { 1 };
    let mut best: Option<(usize, &str)> = None;
    for candidate in candidates {
        if candidate == name {
            continue;
        }
        let distance = edit_distance_within(name, candidate, budget);
        let Some(distance) = distance else {
            continue;
        };
        best = match best {
            Some((best_distance, best_name))
                if (best_distance, best_name) <= (distance, candidate) =>
            {
                Some((best_distance, best_name))
            }
            _ => Some((distance, candidate)),
        };
    }
    best.map(|(_, candidate)| candidate)
}

// Levenshtein distance, `None` when it exceeds `budget` (with a cheap length pre-check so the
// whole candidate set stays fast to scan).
fn edit_distance_within(left: &str, right: &str, budget: usize) -> Option<usize> {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.len().abs_diff(right.len()) > budget {
        return None;
    }
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0; right.len() + 1];
    for (row, left_character) in left.iter().enumerate() {
        current[0] = row + 1;
        let mut row_minimum = current[0];
        for (column, right_character) in right.iter().enumerate() {
            let substitution = previous[column] + usize::from(left_character != right_character);
            current[column + 1] = substitution
                .min(previous[column + 1] + 1)
                .min(current[column] + 1);
            row_minimum = row_minimum.min(current[column + 1]);
        }
        if row_minimum > budget {
            return None;
        }
        std::mem::swap(&mut previous, &mut current);
    }
    (previous[right.len()] <= budget).then_some(previous[right.len()])
}

// Applies `# roughly: allow(code, ...)` suppression comments: a diagnostic is dropped when a
// suppression naming its code (or `all`) sits on the diagnostic's own line (a trailing comment) or
// on the line directly above it. Codes are the user-facing strings diagnostics render with
// (`naming-style`, `unused`, `unresolved`, ...). Applied at diagnostic assembly, after severity
// decisions, so a suppressed escalated error is dropped like any other diagnostic.
pub fn apply_suppressions(diagnostics: Vec<Diagnostic>, source: &str) -> Vec<Diagnostic> {
    if !source.contains("roughly:") {
        return diagnostics;
    }
    let mut allowed_by_line: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (line_index, line) in source.lines().enumerate() {
        let Some(comment_start) = line.find('#') else {
            continue;
        };
        let comment = line[comment_start..].trim_start_matches('#').trim();
        let Some(rest) = comment.strip_prefix("roughly:") else {
            continue;
        };
        let Some(arguments) = rest
            .trim()
            .strip_prefix("allow(")
            .and_then(|rest| rest.split_once(')'))
            .map(|(inside, _)| inside)
        else {
            continue;
        };
        let codes = arguments
            .split(',')
            .map(|code| code.trim().to_owned())
            .filter(|code| !code.is_empty())
            .collect::<Vec<_>>();
        if !codes.is_empty() {
            allowed_by_line.entry(line_index).or_default().extend(codes);
        }
    }
    if allowed_by_line.is_empty() {
        return diagnostics;
    }
    diagnostics
        .into_iter()
        .filter(|diagnostic| {
            let row = diagnostic.range.start_point.row;
            let same_line = allowed_by_line.get(&row);
            let line_above = row
                .checked_sub(1)
                .and_then(|previous| allowed_by_line.get(&previous));
            let code = diagnostic.code.to_string();
            !same_line
                .into_iter()
                .chain(line_above)
                .flatten()
                .any(|allowed| *allowed == code || allowed == "all")
        })
        .collect()
}

// The individual style lints, each with a stable user-facing code: the code names the rule in
// diagnostics (`warning[naming-style]`), in suppression comments, and in configuration, so it must
// stay stable once shipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lint {
    MissingComma,
    TrailingComma,
    NamingStyle,
    AssignmentOperator,
    BooleanShorthand,
    UnusedParameter,
}

impl Lint {
    pub fn code(&self) -> &'static str {
        match self {
            Lint::MissingComma => "missing-comma",
            Lint::TrailingComma => "trailing-comma",
            Lint::NamingStyle => "naming-style",
            Lint::AssignmentOperator => "assignment-operator",
            Lint::BooleanShorthand => "boolean-shorthand",
            Lint::UnusedParameter => "unused-parameter",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: DiagnosticCode,
    pub message: String,
    pub range: Range,
}

impl Diagnostic {
    pub fn lint_error(lint: Lint, range: Range, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code: DiagnosticCode::Lint(lint),
            message: message.into(),
            range,
        }
    }

    pub fn lint_warning(lint: Lint, range: Range, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            code: DiagnosticCode::Lint(lint),
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

    pub fn unresolved_warning(range: Range, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            code: DiagnosticCode::Unresolved,
            message: message.into(),
            range,
        }
    }

    // Strict mode escalates unresolved references from warnings to errors: under strict, a name
    // the resolver cannot see is a hole in the checked surface, not a hint.
    pub fn escalate_unresolved_to_error(&mut self) {
        if self.code == DiagnosticCode::Unresolved {
            self.severity = Severity::Error;
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
            InferenceError::InvalidVectorElement { element } => {
                let mut type_renderer = TypeRenderer::user_facing(interner);
                let rendered = type_renderer.render(element);
                (
                    fallback_range,
                    format!(
                        "the element of a `[]` vector type must be an atomic type, found `{rendered}` — for a list of these, write `list[{rendered}]`"
                    ),
                )
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
                    Constraint::AtomicElement => {
                        "an atomic value (`logical`, `integer`, `double`, `complex`, `character`, or `raw`)"
                    }
                    Constraint::ScalarNumeric => "a scalar numeric value (`integer` or `double`)",
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
            InferenceError::DollarOnAtomicVector {
                actual,
                range,
                expression_id: _,
            } => {
                let mut type_renderer = TypeRenderer::user_facing(interner);
                (
                    *range,
                    format!(
                        "R's `$` operator is invalid on atomic vectors; this value is `{}` — extract an element with `[[` instead.",
                        type_renderer.render(actual)
                    ),
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
    Lint(Lint),
    Naming,
    // A reference the resolver could not resolve: an unknown bare name, an unknown package
    // namespace, or a name a known namespace does not export. Its own code (rather than `Naming`)
    // because strict mode escalates exactly this class to errors.
    Unresolved,
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
            Self::Lint(lint) => formatter.write_str(lint.code()),
            Self::Unresolved => formatter.write_str("unresolved"),
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
                    Constraint::AtomicElement => format!("{name}: atomic"),
                    Constraint::ScalarNumeric => format!("{name}: scalar numeric"),
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
            CoreType::Vector(element) => format!("{}[]", self.render_core_type(element)),
            CoreType::NamedVector(element) => {
                format!("{}[named]", self.render_core_type(element))
            }
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

    // The rendered parameter parts of a function type, in display order: positional, then named
    // (optional names bracketed) with the `...` element at its formal position among them. Shared
    // by the plain `fn(...)` render and the signature-help render, which additionally records each
    // part's span in the label.
    fn render_function_parts(&mut self, function_type: &FunctionType<CoreType>) -> Vec<String> {
        let mut parts = Vec::new();
        for parameter in &function_type.parameters {
            parts.push(self.render_core_type(parameter));
        }
        let rest_position = function_type
            .variadic
            .as_ref()
            .map(|variadic| variadic.preceding_named);
        for (index, parameter) in function_type.named_parameters.iter().enumerate() {
            if rest_position == Some(index) {
                let variadic = function_type
                    .variadic
                    .as_ref()
                    .expect("position implies rest");
                parts.push(format!("...: {}", self.render_core_type(&variadic.element)));
            }
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
        if let Some(variadic) = &function_type.variadic
            && variadic.preceding_named >= function_type.named_parameters.len()
        {
            parts.push(format!("...: {}", self.render_core_type(&variadic.element)));
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
