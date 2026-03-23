pub mod annotations;
pub mod check;
pub mod diagnostics;
pub mod interner;
pub mod lower;
pub mod parse;
pub mod text;
pub mod typecheck;
pub mod types;

pub use {
    crate::{
        annotations::{TypeParseError, parse_surface_type, render_surface_type},
        check::{AnalysisState, CheckResult, check, check_source},
        diagnostics::{Diagnostic, DiagnosticCode, Severity},
        interner::{Interner, Symbol},
        lower::{
            Argument, Expression, ExpressionId, ExpressionKind, LoweringContext, Module, Parameter,
            lower_root_with_rope,
        },
        parse::{new_parser, parse},
        text::{AnnotationBlock, annotation_block, line_text, node_text, point_label},
        types::{Atomic, CoreType, InferenceVariableId, SurfaceType, TypeScheme},
    },
    tree_sitter::Parser,
};
