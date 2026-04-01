pub mod diagnostic;
pub mod document;
pub mod hir;
pub mod interner;
pub mod lower;
pub mod naming;
mod package_hir;
pub mod analysis;
pub mod text;
pub mod tree;
pub mod type_syntax;
pub mod typecheck;
pub mod types;

pub use crate::{
    diagnostic::{CheckResult, Diagnostic, DiagnosticCode, DocumentDiagnostics, Severity},
    document::{Document, DocumentEditError, DocumentId},
    hir::{
        Argument, Definition, DefinitionId, DefinitionItem, DefinitionKind, Expression,
        ExpressionId, ExpressionKind, Module, ModuleId, Parameter,
    },
    interner::{Interner, Symbol},
    lower::{LoweringContext, lower},
    analysis::{
        AnalysisError, AnalysisPhase, Analysis, LoweringResult, NamingRunResult,
        PhaseDiagnostic, check, run_lowering, run_lowering_and_naming, run_naming, run_typecheck,
    },
    text::{TextPosition, TextRange},
    tree::{field, kind},
    type_syntax::{
        TypeParseError, TypeSyntax, parse_surface_type, parse_type_syntax, render_surface_type,
        render_type_syntax,
    },
    types::{Atomic, CoreType, InferenceVariableId, SurfaceType, TypeScheme},
};
