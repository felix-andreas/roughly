//! The diagnostic edge: item-relative findings become file diagnostics with
//! absolute positions, and structured type errors become wording.
//!
//! One type-display policy, one renderer: inference variables and rigid
//! binders display as `T`/`U`/`V` in first-occurrence order, and one renderer
//! instance must span everything that shares names (both sides of an
//! expected/found message), because a fresh renderer restarts the numbering.

use crate::check::{TypeError, TypeErrorKind};
use crate::types::{Atomic, Constraint, FunctionType, Name, Ty, TyKind, TypeScheme};
use crate::{Db, DocumentKind, Item, SourceFile, item_check, item_tree, parse};
use syntax::TextRange;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, salsa::SalsaValue)]
pub enum Severity {
    Error,
    Warning,
}

/// One rendered finding, file-absolute.
#[derive(Debug, Clone, PartialEq, Eq, salsa::SalsaValue)]
pub struct Diagnostic {
    pub range: TextRange,
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
}

/// All diagnostics of one file: syntax errors, naming findings, and type
/// errors, in position order.
#[salsa::tracked(returns(clone))]
pub fn file_diagnostics(db: &dyn Db, file: SourceFile) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    let parsed = parse(db, file);
    for error in parsed.errors() {
        diagnostics.push(Diagnostic {
            range: error.range,
            severity: Severity::Error,
            code: "syntax-error",
            message: error.message.clone(),
        });
    }

    for item in item_tree(db, file) {
        let Some(offset) = item_offset(db, item) else {
            continue;
        };
        let Some(check) = item_check(db, item) else {
            continue;
        };
        for error in &check.errors {
            let range = TextRange::new(error.range.start() + offset, error.range.end() + offset);
            diagnostics.push(render_type_error(db, range, error));
        }
        // Naming findings are item-relative too.
        let Some(module) = crate::item_hir(db, item) else {
            continue;
        };
        let Some(naming) = crate::item_naming(db, item) else {
            continue;
        };
        for unused in &naming.unused_assignments {
            let range = TextRange::new(unused.range.start() + offset, unused.range.end() + offset);
            diagnostics.push(Diagnostic {
                range,
                severity: Severity::Warning,
                code: "unused",
                message: format!("`{}` is assigned but its value is never used", unused.name),
            });
        }
        if *file.kind(db) == DocumentKind::Package {
            for (expression, name) in &naming.non_locals {
                if crate::package_scheme_exists(db, name) {
                    continue;
                }
                let expression_range = module.expression(*expression).range;
                let range = TextRange::new(
                    expression_range.start() + offset,
                    expression_range.end() + offset,
                );
                diagnostics.push(Diagnostic {
                    range,
                    severity: Severity::Warning,
                    code: "could-not-resolve",
                    message: format!("could not resolve `{name}`"),
                });
            }
        }
    }

    diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start(), diagnostic.range.end()));
    diagnostics
}

/// The absolute byte offset of an item's subtree inside its file.
fn item_offset(db: &dyn Db, item: Item<'_>) -> Option<syntax::TextSize> {
    crate::resolve_item_node(db, item).map(|node| node.text_range().start())
}

fn render_type_error(db: &dyn Db, range: TextRange, error: &TypeError<'_>) -> Diagnostic {
    let message = render_type_error_message(db, error);
    Diagnostic {
        range,
        severity: Severity::Error,
        code: "type-mismatch",
        message,
    }
}

fn render_type_error_message(db: &dyn Db, error: &TypeError<'_>) -> String {
    let mut renderer = TypeRenderer::default();
    match &error.kind {
        TypeErrorKind::Mismatch { expected, found } => format!(
            "expected `{}`, found `{}`",
            renderer.render(db, *expected),
            renderer.render(db, *found)
        ),
        TypeErrorKind::NotAFunction { found } => {
            format!("this is not a function: `{}`", renderer.render(db, *found))
        }
        TypeErrorKind::ArityMismatch { expected, found } => {
            format!("this call passes {found} argument(s), but the function accepts {expected}")
        }
        TypeErrorKind::UnknownArgument { name } => {
            format!("unknown argument `{name}`")
        }
        TypeErrorKind::AnnotationParameterMismatch { name } => {
            format!("the annotation and the definition disagree about parameter `{name}`")
        }
        TypeErrorKind::ConstraintViolation { constraint, found } => {
            let expected_description = match constraint {
                Constraint::Unconstrained => "a value",
                Constraint::Numeric => "a numeric value (`integer` or `double`)",
                Constraint::AtomicElement => {
                    "an atomic value (`logical`, `integer`, `double`, `complex`, `character`, or `raw`)"
                }
                Constraint::ScalarNumeric => "a scalar numeric value (`integer` or `double`)",
            };
            format!(
                "expected {expected_description}, found `{}`",
                renderer.render(db, *found)
            )
        }
        TypeErrorKind::NoMatchingOverload {
            name,
            candidates,
            first,
        } => {
            let mut message = format!(
                "no overload of `{name}` matches these arguments — I tried all {candidates} declared signatures"
            );
            if let Some(first) = first {
                message.push_str(&format!(
                    "; the first candidate fails with: {}",
                    render_type_error_message(db, first)
                ));
            }
            message
        }
        TypeErrorKind::InfiniteType => "this would create an infinite type".to_owned(),
    }
}

/// The user-facing type renderer: `T`/`U`/`V`… in first-occurrence order.
#[derive(Default)]
pub struct TypeRenderer<'db> {
    names: Vec<RenderedVar<'db>>,
}

#[derive(PartialEq, Eq)]
enum RenderedVar<'db> {
    Inference(crate::types::InferenceVar),
    Rigid(Name<'db>),
}

impl<'db> TypeRenderer<'db> {
    pub fn render(&mut self, db: &'db dyn Db, ty: Ty<'db>) -> String {
        match ty.kind(db) {
            TyKind::Any => "Any".to_owned(),
            TyKind::Unknown => "Unknown".to_owned(),
            TyKind::Null => "NULL".to_owned(),
            TyKind::Scalar(atomic) => atomic_name(*atomic).to_owned(),
            TyKind::Vector(element) => format!("{}[]", self.render(db, *element)),
            TyKind::NamedVector(element) => format!("{}[named]", self.render(db, *element)),
            TyKind::List(element) => format!("list[{}]", self.render(db, *element)),
            TyKind::NamedList(element) => format!("list[named: {}]", self.render(db, *element)),
            TyKind::Tuple(items) => {
                let items: Vec<String> = items.iter().map(|&item| self.render(db, item)).collect();
                format!("list{{{}}}", items.join(", "))
            }
            TyKind::Record(fields) => {
                let fields: Vec<String> = fields
                    .iter()
                    .map(|field| format!("{}: {}", field.name.text(db), self.render(db, field.ty)))
                    .collect();
                format!("list{{{}}}", fields.join(", "))
            }
            TyKind::Function(function) => self.render_function(db, function),
            TyKind::Union(members) => {
                let members: Vec<String> = members
                    .iter()
                    .map(|&member| self.render(db, member))
                    .collect();
                members.join(" | ")
            }
            TyKind::Named(name, arguments) => {
                if arguments.is_empty() {
                    name.text(db).to_owned()
                } else {
                    let arguments: Vec<String> = arguments
                        .iter()
                        .map(|&argument| self.render(db, argument))
                        .collect();
                    format!("{}<{}>", name.text(db), arguments.join(", "))
                }
            }
            TyKind::Var(var) => self.variable_name(RenderedVar::Inference(*var)),
            TyKind::Rigid(name) => self.variable_name(RenderedVar::Rigid(*name)),
        }
    }

    pub fn render_scheme(&mut self, db: &'db dyn Db, scheme: &TypeScheme<'db>) -> String {
        if scheme.binders.is_empty() {
            return self.render(db, scheme.body);
        }
        let binders: Vec<String> = scheme
            .binders
            .iter()
            .map(|(name, constraint)| {
                let rendered = self.variable_name(RenderedVar::Rigid(*name));
                match constraint {
                    Constraint::Unconstrained => rendered,
                    Constraint::Numeric => format!("{rendered}: numeric"),
                    Constraint::AtomicElement => format!("{rendered}: atomic"),
                    Constraint::ScalarNumeric => format!("{rendered}: scalar numeric"),
                }
            })
            .collect();
        format!("<{}> {}", binders.join(", "), self.render(db, scheme.body))
    }

    fn render_function(&mut self, db: &'db dyn Db, function: &FunctionType<'db>) -> String {
        let mut parameters = Vec::new();
        for ty in &function.positional {
            parameters.push(self.render(db, *ty));
        }
        for field in &function.named {
            let name = if field.optional {
                format!("[{}]", field.name.text(db))
            } else {
                field.name.text(db).to_owned()
            };
            parameters.push(format!("{name}: {}", self.render(db, field.ty)));
        }
        if let Some(rest) = &function.variadic {
            parameters.push(format!("...: {}", self.render(db, rest.element)));
        }
        let ret = self.render(db, function.ret);
        format!("fn({}) -> {}", parameters.join(", "), ret)
    }

    fn variable_name(&mut self, var: RenderedVar<'db>) -> String {
        let index = match self.names.iter().position(|existing| *existing == var) {
            Some(index) => index,
            None => {
                self.names.push(var);
                self.names.len() - 1
            }
        };
        let letter = (b'T' + (index as u8 % 7)) as char;
        let suffix = index / 7;
        if suffix == 0 {
            letter.to_string()
        } else {
            format!("{letter}{suffix}")
        }
    }
}

fn atomic_name(atomic: Atomic) -> &'static str {
    match atomic {
        Atomic::Logical => "logical",
        Atomic::Integer => "integer",
        Atomic::Double => "double",
        Atomic::Complex => "complex",
        Atomic::Character => "character",
        Atomic::Raw => "raw",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DocumentKind, ProjectFiles, RootDatabase, SourceFile};

    fn render_all(db: &RootDatabase, file: SourceFile) -> Vec<String> {
        file_diagnostics(db, file)
            .into_iter()
            .map(|d| {
                format!(
                    "{}..{} {}[{}] {}",
                    u32::from(d.range.start()),
                    u32::from(d.range.end()),
                    match d.severity {
                        Severity::Error => "error",
                        Severity::Warning => "warning",
                    },
                    d.code,
                    d.message
                )
            })
            .collect()
    }

    #[test]
    fn file_diagnostics_end_to_end() {
        let db = RootDatabase::default();
        let util = SourceFile::new(
            &db,
            "add <- function(x, y) x + y\n".to_owned(),
            DocumentKind::Package,
        );
        let main = SourceFile::new(
            &db,
            "bad <- function() add(\"a\", 2L)\nmissing_fn <- function() nowhere()\n".to_owned(),
            DocumentKind::Package,
        );
        ProjectFiles::new(&db, vec![util, main]);
        let rendered = render_all(&db, main);
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("type-mismatch") && line.contains("character")),
            "expected a mismatch mentioning character: {rendered:?}"
        );
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("could-not-resolve") && line.contains("nowhere")),
            "expected could-not-resolve for nowhere: {rendered:?}"
        );
        // The second item's finding is offset into the file (absolute range).
        let missing = rendered
            .iter()
            .find(|line| line.contains("nowhere"))
            .expect("nowhere finding");
        let start: u32 = missing.split("..").next().unwrap().parse().unwrap();
        assert!(start > 30, "range must be file-absolute, got {missing}");
    }

    #[test]
    fn unused_and_syntax_diagnostics_render() {
        let db = RootDatabase::default();
        let file = SourceFile::new(
            &db,
            "f <- function() {\n  dead <- 1\n  dead <- 2\n  dead\n}\ng <- function( {\n".to_owned(),
            DocumentKind::Script,
        );
        let rendered = render_all(&db, file);
        assert!(
            rendered.iter().any(|line| line.contains("unused")),
            "expected a dead-store warning: {rendered:?}"
        );
        assert!(
            rendered.iter().any(|line| line.contains("syntax-error")),
            "expected a syntax error: {rendered:?}"
        );
    }

    #[test]
    fn renderer_shares_names_across_both_sides() {
        let db = RootDatabase::default();
        let mut renderer = TypeRenderer::default();
        let t = crate::types::Ty::new(
            &db,
            crate::types::TyKind::Rigid(crate::types::Name::new(&db, "A".to_owned())),
        );
        let u = crate::types::Ty::new(
            &db,
            crate::types::TyKind::Rigid(crate::types::Name::new(&db, "B".to_owned())),
        );
        // First occurrence order: A -> T, B -> U; A again stays T.
        assert_eq!(renderer.render(&db, t), "T");
        assert_eq!(renderer.render(&db, u), "U");
        assert_eq!(renderer.render(&db, t), "T");
    }
}
