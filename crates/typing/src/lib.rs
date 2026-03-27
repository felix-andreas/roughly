pub mod check;
pub mod diagnostic;
pub mod hir;
pub mod interner;
pub mod lower;
pub mod naming;
pub mod text;
pub mod type_syntax;
pub mod typecheck;
pub mod types;

pub use {
    crate::{
        check::{AnalysisState, CheckResult, check},
        diagnostic::{Diagnostic, DiagnosticCode, Severity},
        hir::{
            Argument, Definition, DefinitionId, DefinitionItem, DefinitionKind, Expression,
            ExpressionId, ExpressionKind, Module, Parameter,
        },
        interner::{Interner, Symbol},
        lower::{LoweringContext, lower_root_with_rope},
        text::{AnnotationBlock, annotation_block, line_text, node_text, point_label},
        type_syntax::{
            TypeParseError, TypeSyntax, parse_surface_type, parse_type_syntax, render_surface_type,
            render_type_syntax,
        },
        types::{Atomic, CoreType, InferenceVariableId, SurfaceType, TypeScheme},
    },
    tree_sitter::Parser,
};
