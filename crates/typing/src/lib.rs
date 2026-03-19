pub mod check;
pub mod diagnostics;
pub mod infer;
pub mod interner;
pub mod lower;
pub mod parse;
pub mod type_syntax;
pub mod types;

pub use crate::{
    check::{CheckResult, check},
    diagnostics::{Diagnostic, DiagnosticCode, Severity},
    interner::{Interner, Symbol},
    lower::{
        Argument, Expression, ExpressionId, ExpressionKind, LoweringContext, Module, Parameter,
    },
    parse::{new_parser, parse},
    type_syntax::{TypeParseError, parse_surface_type, render_surface_type},
    types::{Atomic, CoreType, InferenceVariableId, SurfaceType, TypeScheme},
};
