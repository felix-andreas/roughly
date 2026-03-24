use {
    crate::{
        Interner,
        diagnostic::Diagnostic,
        lower::{LoweringContext, lower_root_with_rope_with_diagnostics},
        text,
        typecheck::{BuiltinKind, InferenceState},
    },
    ropey::Rope,
    tree_sitter::Node,
};

#[derive(Debug, Default)]
pub struct AnalysisState {
    lowering_context: LoweringContext,
}

impl AnalysisState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn interner(&self) -> &Interner {
        self.lowering_context.interner()
    }

    pub fn interner_mut(&mut self) -> &mut Interner {
        self.lowering_context.interner_mut()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub diagnostics: Vec<Diagnostic>,
}

impl CheckResult {
    pub fn render(&self, source: &str) -> String {
        if self.diagnostics.is_empty() {
            return "No diagnostics.\n".to_owned();
        }

        let mut rendered = String::new();

        for (index, diagnostic) in self.diagnostics.iter().enumerate() {
            if index > 0 {
                rendered.push('\n');
            }
            rendered.push_str(&diagnostic.render(source));
        }

        rendered
    }
}

pub fn check(node: Node<'_>, rope: &Rope, analysis_state: &mut AnalysisState) -> CheckResult {
    let mut diagnostics = Vec::new();

    if node.has_error() {
        collect_syntax_errors(node, rope, &mut diagnostics);
        return CheckResult { diagnostics };
    }

    let lowering_result =
        lower_root_with_rope_with_diagnostics(node, rope, &mut analysis_state.lowering_context);
    diagnostics.extend(lowering_result.diagnostics);

    if !diagnostics.is_empty() {
        return CheckResult { diagnostics };
    }

    let module = lowering_result.module;

    let naming_context = crate::naming::NamingContext::new(&module.arena);
    let naming_result = naming_context.resolve_module(&module);
    diagnostics.extend(naming_result.diagnostics.clone());

    if !diagnostics.is_empty() {
        return CheckResult { diagnostics };
    }

    let mut inference_state = InferenceState::new();
    bind_builtins(&mut inference_state, &mut analysis_state.lowering_context);

    if let Err(error) = inference_state.infer_module(&module) {
        diagnostics.push(Diagnostic::from_inference_error(
            &error,
            fallback_range(rope),
            analysis_state.lowering_context.interner(),
        ));
    }

    CheckResult { diagnostics }
}

fn bind_builtins(inference_state: &mut InferenceState, lowering_context: &mut LoweringContext) {
    let plus_symbol = lowering_context.intern("+");
    let minus_symbol = lowering_context.intern("-");
    let multiply_symbol = lowering_context.intern("*");
    let divide_symbol = lowering_context.intern("/");
    let power_symbol = lowering_context.intern("**");
    let and_symbol = lowering_context.intern("&&");
    let or_symbol = lowering_context.intern("||");
    let combine_symbol = lowering_context.intern("c");
    let list_symbol = lowering_context.intern("list");

    inference_state.bind_builtin(plus_symbol, BuiltinKind::Plus);
    inference_state.bind_builtin(minus_symbol, BuiltinKind::Minus);
    inference_state.bind_builtin(multiply_symbol, BuiltinKind::Multiply);
    inference_state.bind_builtin(divide_symbol, BuiltinKind::Divide);
    inference_state.bind_builtin(power_symbol, BuiltinKind::Power);
    inference_state.bind_builtin(and_symbol, BuiltinKind::And);
    inference_state.bind_builtin(or_symbol, BuiltinKind::Or);
    inference_state.bind_builtin(combine_symbol, BuiltinKind::Combine);
    inference_state.bind_builtin(list_symbol, BuiltinKind::List);
}

fn collect_syntax_errors(node: Node<'_>, rope: &Rope, diagnostics: &mut Vec<Diagnostic>) {
    if node.is_error() {
        diagnostics.push(Diagnostic::syntax_error(
            node.range(),
            format!("Unexpected syntax: {}", snippet(node, rope)),
        ));
        return;
    }

    if node.is_missing() {
        diagnostics.push(Diagnostic::syntax_error(
            node.range(),
            format!(
                "Missing syntax near {}",
                point_label(node.range().start_point)
            ),
        ));
        return;
    }

    let child_count = node.child_count();
    for child_index in 0..child_count {
        if let Some(child) = node.child(child_index) {
            collect_syntax_errors(child, rope, diagnostics);
        }
    }
}

fn snippet(node: Node<'_>, rope: &Rope) -> String {
    let compact = text::compact_node_text(rope, node);

    if compact == "<empty>" || compact == "<unavailable>" {
        compact
    } else if compact.len() > 40 {
        format!("{:?}…", &compact[..40])
    } else {
        format!("{compact:?}")
    }
}

fn point_label(point: tree_sitter::Point) -> String {
    format!("{}:{}", point.row + 1, point.column + 1)
}

fn fallback_range(rope: &Rope) -> tree_sitter::Range {
    let line = text::first_line_text(rope);
    tree_sitter::Range {
        start_byte: 0,
        end_byte: line.len(),
        start_point: tree_sitter::Point { row: 0, column: 0 },
        end_point: tree_sitter::Point {
            row: 0,
            column: line.len(),
        },
    }
}
