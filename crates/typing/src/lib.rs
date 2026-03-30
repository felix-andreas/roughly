pub mod diagnostic;
pub mod document;
pub mod hir;
pub mod interner;
pub mod lower;
pub mod naming;
pub mod package;
mod package_hir;
pub mod pipeline;
pub mod text;
pub mod tree;
pub mod type_syntax;
pub mod typecheck;
pub mod types;
pub mod workspace;

pub use crate::{
    diagnostic::{CheckResult, Diagnostic, DiagnosticCode, DocumentDiagnostics, Severity},
    document::Document,
    hir::{
        Argument, Definition, DefinitionId, DefinitionItem, DefinitionKind, Expression,
        ExpressionId, ExpressionKind, Module, Parameter,
    },
    interner::{Interner, Symbol},
    lower::{LoweringContext, lower},
    package::Package,
    pipeline::{
        AnalysisState, PackageLoweringAndNamingResult, check, run_lowering_and_naming,
        run_typecheck,
    },
    text::{TextPosition, TextRange},
    tree::{field, kind},
    type_syntax::{
        TypeParseError, TypeSyntax, parse_surface_type, parse_type_syntax, render_surface_type,
        render_type_syntax,
    },
    types::{Atomic, CoreType, InferenceVariableId, SurfaceType, TypeScheme},
    workspace::{Workspace, WorkspaceError},
};
