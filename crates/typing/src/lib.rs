pub mod annotations;
pub mod check;
pub mod diagnostics;
pub mod infer;
pub mod interner;
pub mod lower;
pub mod parse;
pub mod types;

pub use crate::{
    annotations::{TypeParseError, parse_surface_type, render_surface_type},
    check::{CheckResult, check},
    diagnostics::{Diagnostic, DiagnosticCode, Severity},
    interner::{Interner, Symbol},
    lower::{
        Argument, Expression, ExpressionId, ExpressionKind, LoweringContext, Module, Parameter,
    },
    parse::{new_parser, parse},
    types::{Atomic, CoreType, InferenceVariableId, SurfaceType, TypeScheme},
};
