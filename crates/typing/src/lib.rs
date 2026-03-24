pub mod annotations;
pub mod check;
pub mod diagnostics;
pub mod hir;
pub mod interner;
pub mod lower;
pub mod text;
pub mod typecheck;
pub mod types;

pub use {
    crate::{
        annotations::{TypeParseError, parse_surface_type, render_surface_type},
        check::{AnalysisState, CheckResult, check},
        diagnostics::{Diagnostic, DiagnosticCode, Severity},
        hir::{Argument, Expression, ExpressionId, ExpressionKind, Module, Parameter},
        interner::{Interner, Symbol},
        lower::{LoweringContext, lower_root_with_rope},
        text::{AnnotationBlock, annotation_block, line_text, node_text, point_label},
        types::{Atomic, CoreType, InferenceVariableId, SurfaceType, TypeScheme},
    },
    tree_sitter::Parser,
};
