pub mod analysis;
pub mod diagnostic;
pub mod document;
pub mod hir;
pub mod ide;
pub mod interner;
pub mod lint;
pub mod lower;
pub mod namespace;
pub mod naming;
pub mod s4;
pub mod stdlib;
pub mod stub;
pub mod text;
pub mod tree;
pub mod type_syntax;
pub mod typecheck;
pub mod types;

pub use crate::{
    analysis::{
        Analysis, AnalysisError, CheckConfig, LintConfig, LintLevel, lint, lower, resolve_package,
        run_full, strict_origin_diagnostics, typecheck,
    },
    diagnostic::{
        Diagnostic, DiagnosticCode, Diagnostics, DocumentDiagnostics, Lint, Severity,
        render_core_type, render_diagnostics, render_type_scheme,
    },
    document::{Document, DocumentChange, DocumentId, DocumentParseError},
    hir::{
        Argument, Definition, DefinitionId, DefinitionItem, DefinitionKind, Expression,
        ExpressionId, ExpressionKind, Module, ModuleId, Parameter,
    },
    ide::{
        CodeAction, CodeActionKind, CompletionItem, CompletionItemKind, CompletionItemSource,
        CompletionResult, DebugSection, HoverInfo, InlayHint, Location, RenameResult,
        SignatureHelp, TextEdit, inlay_hints, render_hover_markdown, signature_help,
    },
    interner::{Interner, Symbol},
    lint::NameStyle,
    lower::LoweringContext,
    text::{TextPosition, TextRange},
    tree::{field, kind},
    type_syntax::{
        TypeParseError, TypeSyntax, TypeToken, TypeTokenRole, parse_surface_type,
        parse_type_syntax, render_surface_type, render_type_syntax,
        semantic_tokens as type_semantic_tokens, type_name_token_range,
    },
    types::{Atomic, CoreType, InferenceVariableId, SurfaceType, TypeScheme},
};
