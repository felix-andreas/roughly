pub mod analysis;
pub mod diagnostic;
pub mod document;
pub mod hir;
pub mod ide;
pub mod interner;
pub mod lint;
pub mod lower;
pub mod naming;
pub mod text;
pub mod tree;
pub mod type_syntax;
pub mod typecheck;
pub mod types;

pub use crate::{
    analysis::{
        Analysis, AnalysisError, AnalysisPhase, PhaseDiagnostic, check, run_lowering, run_naming,
        run_typecheck,
    },
    diagnostic::{Diagnostic, DiagnosticCode, Diagnostics, DocumentDiagnostics, Severity, render_diagnostics},
    document::{Document, DocumentEditError, DocumentId, DocumentParseError},
    hir::{
        Argument, Definition, DefinitionId, DefinitionItem, DefinitionKind, Expression,
        ExpressionId, ExpressionKind, Module, ModuleId, Parameter,
    },
    ide::{HoverInfo, HoverPhase, HoverSection, render_hover_markdown},
    interner::{Interner, Symbol},
    lint::{Config as LintConfig, NameStyle},
    lower::{LoweringContext, lower},
    text::{TextPosition, TextRange},
    tree::{field, kind},
    type_syntax::{
        TypeParseError, TypeSyntax, parse_surface_type, parse_type_syntax, render_surface_type,
        render_type_syntax,
    },
    types::{Atomic, CoreType, InferenceVariableId, SurfaceType, TypeScheme},
};
