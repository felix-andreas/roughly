use {
    crate::{
        Interner,
        diagnostic::Diagnostic,
        hir::{DefinitionId, DefinitionItem, ExpressionId, ExpressionKind, HirArena, Module},
        lower::{LoweringContext, lower_with_diagnostics},
        naming::{NamingContext, NamingResult},
        text,
        typecheck::{BuiltinKind, InferenceState},
        workspace::{Document, Package},
    },
    std::{
        collections::{BTreeSet, HashMap},
        path::{Path, PathBuf},
    },
};

#[derive(Debug, Default)]
pub struct AnalysisState {
    lowering_context: LoweringContext,
    lowered_documents: HashMap<PathBuf, LoweredDocument>,
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
pub struct PackageLoweringAndNamingResult {
    pub modules: HashMap<PathBuf, Module>,
    pub naming: NamingResult,
    pub diagnostics: Vec<Diagnostic>,
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

pub fn run_lowering_and_naming(
    package: &Package,
    analysis_state: &mut AnalysisState,
) -> PackageLoweringAndNamingResult {
    refresh_lowered_documents(package, None, analysis_state);

    let mut diagnostics = Vec::new();
    let mut modules = HashMap::new();

    for (path, lowered_document) in &analysis_state.lowered_documents {
        modules.insert(path.clone(), lowered_document.module.clone());
        diagnostics.extend(lowered_document.diagnostics.clone());
    }

    if !diagnostics.is_empty() {
        return PackageLoweringAndNamingResult {
            modules,
            naming: NamingResult::default(),
            diagnostics,
        };
    }

    let (merged_module, modules) = merge_modules(sorted_modules(&modules));
    let naming = NamingContext::new(&merged_module.arena, analysis_state.interner())
        .resolve_module(&merged_module);
    diagnostics.extend(naming.diagnostics.clone());

    PackageLoweringAndNamingResult {
        modules,
        naming,
        diagnostics,
    }
}

pub fn run_lowering_and_naming_incremental(
    package: &Package,
    changed_document_path: &Path,
    analysis_state: &mut AnalysisState,
) -> PackageLoweringAndNamingResult {
    refresh_lowered_documents(package, Some(changed_document_path), analysis_state);

    let mut diagnostics = Vec::new();
    let mut modules = HashMap::new();

    for (path, lowered_document) in &analysis_state.lowered_documents {
        modules.insert(path.clone(), lowered_document.module.clone());
        diagnostics.extend(lowered_document.diagnostics.clone());
    }

    if !diagnostics.is_empty() {
        return PackageLoweringAndNamingResult {
            modules,
            naming: NamingResult::default(),
            diagnostics,
        };
    }

    let (merged_module, modules) = merge_modules(sorted_modules(&modules));
    let naming = NamingContext::new(&merged_module.arena, analysis_state.interner())
        .resolve_module(&merged_module);
    diagnostics.extend(naming.diagnostics.clone());

    PackageLoweringAndNamingResult {
        modules,
        naming,
        diagnostics,
    }
}

pub fn check(package: &Package, analysis_state: &mut AnalysisState) -> CheckResult {
    let package_result = run_lowering_and_naming(package, analysis_state);
    if !package_result.diagnostics.is_empty() {
        return CheckResult {
            diagnostics: package_result.diagnostics,
        };
    }

    let (merged_module, _) = merge_modules(sorted_modules(&package_result.modules));
    let mut inference_state = InferenceState::new();
    bind_builtins(&mut inference_state, &mut analysis_state.lowering_context);

    let mut diagnostics = Vec::new();
    if let Err(error) = inference_state.infer_module(&merged_module) {
        diagnostics.push(Diagnostic::from_inference_error(
            &error,
            fallback_range(package),
            analysis_state.lowering_context.interner(),
        ));
    }

    CheckResult { diagnostics }
}

fn refresh_lowered_documents(
    package: &Package,
    changed_document_path: Option<&Path>,
    analysis_state: &mut AnalysisState,
) {
    let package_documents = package_documents(package);
    let package_paths = package_documents
        .iter()
        .map(|(path, _)| path.clone())
        .collect::<BTreeSet<_>>();

    analysis_state
        .lowered_documents
        .retain(|path, _| package_paths.contains(path));

    for (path, document) in package_documents {
        let should_refresh = match changed_document_path {
            None => true,
            Some(changed_document_path) if changed_document_path == path.as_path() => true,
            Some(_) => !analysis_state.lowered_documents.contains_key(&path),
        };

        if should_refresh {
            analysis_state.lowered_documents.insert(
                path.clone(),
                lower_document(document, &mut analysis_state.lowering_context),
            );
        }
    }
}

fn lower_document(document: &Document, lowering_context: &mut LoweringContext) -> LoweredDocument {
    let root = document.tree().root_node();
    if root.has_error() {
        let mut diagnostics = Vec::new();
        collect_syntax_errors(root, document.rope(), &mut diagnostics);
        return LoweredDocument {
            module: Module::new(HirArena::new(), Vec::new(), Vec::new()),
            diagnostics,
        };
    }

    let lowering_result = lower_with_diagnostics(document, lowering_context);
    LoweredDocument {
        module: lowering_result.module,
        diagnostics: lowering_result.diagnostics,
    }
}

fn package_documents(package: &Package) -> Vec<(PathBuf, &Document)> {
    let mut package_documents = package
        .documents()
        .map(|(path, document)| (path.clone(), document))
        .collect::<Vec<_>>();
    package_documents.sort_by(|(left_path, _), (right_path, _)| left_path.cmp(right_path));

    let mut package_scripts = package
        .scripts()
        .map(|(path, document)| (path.clone(), document))
        .collect::<Vec<_>>();
    package_scripts.sort_by(|(left_path, _), (right_path, _)| left_path.cmp(right_path));

    package_documents.extend(package_scripts);
    package_documents
}

fn sorted_modules(modules: &HashMap<PathBuf, Module>) -> Vec<(PathBuf, Module)> {
    let mut sorted_modules = modules
        .iter()
        .map(|(path, module)| (path.clone(), module.clone()))
        .collect::<Vec<_>>();
    sorted_modules.sort_by(|(left_path, _), (right_path, _)| left_path.cmp(right_path));
    sorted_modules
}

fn merge_modules(modules: Vec<(PathBuf, Module)>) -> (Module, HashMap<PathBuf, Module>) {
    let mut arena = HirArena::new();
    let mut definitions = Vec::new();
    let mut expressions = Vec::new();
    let mut next_expression_id = 0u32;
    let mut next_definition_id = 0u32;
    let mut remapped_module_items = Vec::new();

    for (path, module) in modules {
        let expression_offset = next_expression_id;
        let definition_offset = next_definition_id;

        let remapped_expressions = module
            .arena
            .expressions()
            .iter()
            .cloned()
            .map(|mut expression| {
                expression.id = ExpressionId(expression.id.0 + expression_offset);
                remap_expression_kind(&mut expression.kind, expression_offset);
                expression
            })
            .collect::<Vec<_>>();
        next_expression_id +=
            u32::try_from(remapped_expressions.len()).expect("expression count exceeded u32");
        arena.expressions.extend(remapped_expressions.clone());

        let remapped_definitions = module
            .definitions
            .into_iter()
            .map(|definition| {
                DefinitionItem::new(
                    DefinitionId(definition.id.0 + definition_offset),
                    definition.range,
                    definition.definition,
                )
            })
            .collect::<Vec<_>>();
        next_definition_id +=
            u32::try_from(remapped_definitions.len()).expect("definition count exceeded u32");
        definitions.extend(remapped_definitions.clone());

        let remapped_module_expressions = module
            .expressions
            .into_iter()
            .map(|expression_id| ExpressionId(expression_id.0 + expression_offset))
            .collect::<Vec<_>>();
        expressions.extend(remapped_module_expressions.iter().copied());
        remapped_module_items.push((path, remapped_definitions, remapped_module_expressions));
    }

    let merged_module = Module::new(arena, definitions, expressions);
    let remapped_modules = remapped_module_items
        .into_iter()
        .map(|(path, module_definitions, module_expressions)| {
            (
                path,
                Module::new(
                    merged_module.arena.clone(),
                    module_definitions,
                    module_expressions,
                ),
            )
        })
        .collect::<HashMap<_, _>>();

    (merged_module, remapped_modules)
}

fn remap_expression_kind(expression_kind: &mut ExpressionKind, expression_offset: u32) {
    match expression_kind {
        ExpressionKind::Block { expressions, .. } => {
            for expression_id in expressions {
                *expression_id = ExpressionId(expression_id.0 + expression_offset);
            }
        }
        ExpressionKind::Assign { value, .. }
        | ExpressionKind::UnaryMinus { value }
        | ExpressionKind::Dollar { value, .. } => {
            *value = ExpressionId(value.0 + expression_offset);
        }
        ExpressionKind::Function { body, .. } | ExpressionKind::Repeat { body } => {
            *body = ExpressionId(body.0 + expression_offset);
        }
        ExpressionKind::While { condition, body } => {
            *condition = ExpressionId(condition.0 + expression_offset);
            *body = ExpressionId(body.0 + expression_offset);
        }
        ExpressionKind::For { sequence, body, .. } => {
            *sequence = ExpressionId(sequence.0 + expression_offset);
            *body = ExpressionId(body.0 + expression_offset);
        }
        ExpressionKind::If {
            condition,
            consequence,
            alternative,
        } => {
            *condition = ExpressionId(condition.0 + expression_offset);
            *consequence = ExpressionId(consequence.0 + expression_offset);
            if let Some(alternative) = alternative {
                *alternative = ExpressionId(alternative.0 + expression_offset);
            }
        }
        ExpressionKind::Call { callee, arguments }
        | ExpressionKind::Subset {
            value: callee,
            arguments,
        }
        | ExpressionKind::Subset2 {
            value: callee,
            arguments,
        } => {
            *callee = ExpressionId(callee.0 + expression_offset);
            for argument in arguments {
                argument.expression = ExpressionId(argument.expression.0 + expression_offset);
            }
        }
        ExpressionKind::Null
        | ExpressionKind::Logical(_)
        | ExpressionKind::Integer(_)
        | ExpressionKind::Double(_)
        | ExpressionKind::Character(_)
        | ExpressionKind::StringLiteralName(_)
        | ExpressionKind::Symbol(_)
        | ExpressionKind::Unsupported => {}
    }
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

fn collect_syntax_errors(
    node: tree_sitter::Node<'_>,
    rope: &ropey::Rope,
    diagnostics: &mut Vec<Diagnostic>,
) {
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

fn snippet(node: tree_sitter::Node<'_>, rope: &ropey::Rope) -> String {
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

fn fallback_range(package: &Package) -> tree_sitter::Range {
    if let Some((_, document)) = package
        .documents()
        .next()
        .or_else(|| package.scripts().next())
    {
        let line = text::first_line_text(document.rope());
        return tree_sitter::Range {
            start_byte: 0,
            end_byte: line.len(),
            start_point: tree_sitter::Point { row: 0, column: 0 },
            end_point: tree_sitter::Point {
                row: 0,
                column: line.len(),
            },
        };
    }

    tree_sitter::Range {
        start_byte: 0,
        end_byte: 0,
        start_point: tree_sitter::Point { row: 0, column: 0 },
        end_point: tree_sitter::Point { row: 0, column: 0 },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LoweredDocument {
    module: Module,
    diagnostics: Vec<Diagnostic>,
}
