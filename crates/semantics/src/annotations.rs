//! Lowering `#:` annotation syntax onto interned types.
//!
//! The parser already produced first-class `TYPE_*` nodes with real spans;
//! this module turns them into `Ty`/`TypeScheme` values. Explicit `<T>`
//! binders lower to rigid (skolem) types that refuse to bind while the
//! annotated definition's body is checked and generalize back out afterwards.
//! Lowering is total: malformed pieces lower to `Unknown` (their syntax errors
//! already mark them), so annotations never cascade.

use crate::Db;
use crate::types::{
    Atomic, Constraint, FunctionType, Name, RecordField, RestParameter, Ty, TyKind, TypeScheme,
    any, null, scalar, union_of, unknown,
};
use syntax::ast::AstNode as _;
use syntax::{SyntaxKind, SyntaxNode, TextRange};

/// One lowered annotation region.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Annotation<'db> {
    /// The declared type of the annotated definition: a compact type, or the
    /// assembly of `@forall` / `@param` / `@return` directives.
    pub declared: Option<TypeScheme<'db>>,
    /// Named parameter types by name (from `fn(name: T)` or `@param`), for
    /// name-aware matching against the definition's formals.
    pub parameter_names: Vec<(String, Ty<'db>, bool)>,
    /// `@type` / `@alias` definitions carried by this annotation.
    pub definitions: Vec<NamedDefinition<'db>>,
    /// A `#: @strict` / `#: @strict off` toggle.
    pub strict: Option<bool>,
    /// `@trust TYPE` — the declared type applies unchecked.
    pub trusted: bool,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedDefinition<'db> {
    pub alias: bool,
    pub name: Name<'db>,
    pub parameters: Vec<Name<'db>>,
    pub body: Ty<'db>,
}

/// Lower one `ANNOTATION` node.
pub fn lower_annotation<'db>(db: &'db dyn Db, node: &SyntaxNode) -> Annotation<'db> {
    debug_assert_eq!(node.kind(), SyntaxKind::ANNOTATION);
    let mut lowering = Lowering {
        db,
        binders: Vec::new(),
    };
    let mut annotation = Annotation {
        range: node.text_range(),
        ..Annotation::default()
    };

    // Expanded-form accumulation.
    let mut forall: Vec<(Name<'db>, Constraint)> = Vec::new();
    let mut params: Vec<(String, Ty<'db>, bool)> = Vec::new();
    let mut rest: Option<Ty<'db>> = None;
    let mut ret: Option<Ty<'db>> = None;
    let mut saw_expanded = false;

    for child in node.children() {
        match child.kind() {
            SyntaxKind::ANNOTATION_DIRECTIVE => {
                let directive = directive_name(&child);
                match directive.as_str() {
                    "type" | "alias" => {
                        if let Some(definition) =
                            lowering.lower_named_definition(&child, directive == "alias")
                        {
                            annotation.definitions.push(definition);
                        }
                    }
                    "forall" => {
                        saw_expanded = true;
                        for binder in child
                            .children()
                            .filter(|c| c.kind() == SyntaxKind::TYPE_BINDER)
                        {
                            if let Some((name, constraint)) = lowering.lower_binder(&binder) {
                                lowering.binders.push(name);
                                forall.push((name, constraint));
                            }
                        }
                    }
                    "param" => {
                        saw_expanded = true;
                        let name = child
                            .children()
                            .find(|c| c.kind() == SyntaxKind::NAME)
                            .and_then(syntax::ast::Name::cast)
                            .and_then(|name| name.text());
                        let optional = child
                            .children_with_tokens()
                            .filter_map(|element| element.into_token())
                            .any(|token| token.kind() == SyntaxKind::L_BRACKET);
                        let ty = child
                            .children()
                            .find(|c| is_type_kind(c.kind()))
                            .map(|ty| lowering.lower_type(&ty))
                            .unwrap_or_else(|| unknown(db));
                        if let Some(name) = name {
                            if name == "..." {
                                rest = Some(ty);
                            } else {
                                params.push((name, ty, optional));
                            }
                        }
                    }
                    "return" | "returns" => {
                        saw_expanded = true;
                        ret = child
                            .children()
                            .find(|c| is_type_kind(c.kind()))
                            .map(|ty| lowering.lower_type(&ty));
                    }
                    "strict" => {
                        let off = child
                            .children_with_tokens()
                            .filter_map(|element| element.into_token())
                            .any(|token| {
                                token.kind() == SyntaxKind::IDENT && token.text() == "off"
                            });
                        annotation.strict = Some(!off);
                    }
                    "trust" => {
                        annotation.trusted = true;
                        if let Some(ty) = child.children().find(|c| is_type_kind(c.kind())) {
                            let lowered = lowering.lower_type(&ty);
                            annotation.declared = Some(TypeScheme {
                                binders: Vec::new(),
                                body: lowered,
                            });
                        }
                    }
                    // `@new`, `@if-unknown`, unknown directives: no typing
                    // payload at this layer.
                    _ => {}
                }
            }
            SyntaxKind::TYPE_BINDER_LIST => {
                for binder in child
                    .children()
                    .filter(|c| c.kind() == SyntaxKind::TYPE_BINDER)
                {
                    if let Some((name, constraint)) = lowering.lower_binder(&binder) {
                        lowering.binders.push(name);
                        forall.push((name, constraint));
                    }
                }
            }
            kind if is_type_kind(kind) => {
                // Compact form: the annotation IS the declared type.
                let body = lowering.lower_type(&child);
                if let TyKind::Function(function) = body.kind(db) {
                    annotation.parameter_names = function
                        .named
                        .iter()
                        .map(|field| (field.name.text(db).to_owned(), field.ty, field.optional))
                        .collect();
                }
                annotation.declared = Some(TypeScheme {
                    binders: forall.clone(),
                    body,
                });
            }
            _ => {}
        }
    }

    // Assemble the expanded form into a function scheme when directives
    // declared one and no compact type did.
    if saw_expanded && annotation.declared.is_none() && (!params.is_empty() || ret.is_some()) {
        let named: Vec<RecordField<'db>> = params
            .iter()
            .map(|(name, ty, optional)| RecordField {
                name: Name::new(db, name.clone()),
                ty: *ty,
                optional: *optional,
            })
            .collect();
        annotation.parameter_names = params.clone();
        let function = FunctionType {
            positional: Vec::new(),
            named,
            variadic: rest.map(|element| RestParameter {
                element,
                preceding_named: params.len(),
            }),
            ret: ret.unwrap_or_else(|| unknown(db)),
        };
        annotation.declared = Some(TypeScheme {
            binders: forall,
            body: Ty::new(db, TyKind::Function(function)),
        });
    }

    annotation
}

pub fn is_type_kind(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::TYPE_REF
            | SyntaxKind::TYPE_APPLY
            | SyntaxKind::TYPE_VECTOR
            | SyntaxKind::TYPE_UNION
            | SyntaxKind::TYPE_FUNCTION
            | SyntaxKind::TYPE_RECORD
            | SyntaxKind::TYPE_TUPLE
            | SyntaxKind::TYPE_LIST
            | SyntaxKind::TYPE_PAREN
            | SyntaxKind::TYPE_BINDER_LIST
    )
}

/// The joined directive name: adjacent IDENT/keyword/`-` tokens right after
/// the `@` (`@if-unknown` lexes as `if` `-` `unknown`).
fn directive_name(node: &SyntaxNode) -> String {
    let mut name = String::new();
    let mut cursor: Option<rowan::TextSize> = None;
    for element in node.children_with_tokens() {
        let Some(token) = element.into_token() else {
            break;
        };
        match token.kind() {
            SyntaxKind::AT => {
                cursor = Some(token.text_range().end());
                continue;
            }
            SyntaxKind::IDENT | SyntaxKind::MINUS => {}
            kind if kind.is_keyword() => {}
            _ => break,
        }
        if cursor != Some(token.text_range().start()) {
            break;
        }
        name.push_str(token.text());
        cursor = Some(token.text_range().end());
    }
    name
}

struct Lowering<'db> {
    db: &'db dyn Db,
    /// In-scope rigid binder names.
    binders: Vec<Name<'db>>,
}

impl<'db> Lowering<'db> {
    fn lower_binder(&mut self, node: &SyntaxNode) -> Option<(Name<'db>, Constraint)> {
        let mut names = node.children().filter(|c| c.kind() == SyntaxKind::NAME);
        let binder = names.next()?;
        let binder_name = syntax::ast::Name::cast(binder)?.text()?;
        let constraint = names
            .next()
            .and_then(syntax::ast::Name::cast)
            .and_then(|name| name.text())
            .map(|name| match name.as_str() {
                "numeric" => Constraint::Numeric,
                "atomic" => Constraint::AtomicElement,
                _ => Constraint::Unconstrained,
            })
            .unwrap_or(Constraint::Unconstrained);
        Some((Name::new(self.db, binder_name), constraint))
    }

    fn lower_named_definition(
        &mut self,
        node: &SyntaxNode,
        alias: bool,
    ) -> Option<NamedDefinition<'db>> {
        let name = node
            .children()
            .find(|c| c.kind() == SyntaxKind::NAME)
            .and_then(syntax::ast::Name::cast)
            .and_then(|name| name.text())?;
        let saved = self.binders.len();
        let mut parameters = Vec::new();
        if let Some(list) = node
            .children()
            .find(|c| c.kind() == SyntaxKind::TYPE_BINDER_LIST)
        {
            for binder in list
                .children()
                .filter(|c| c.kind() == SyntaxKind::TYPE_BINDER)
            {
                if let Some((binder_name, _)) = self.lower_binder(&binder) {
                    self.binders.push(binder_name);
                    parameters.push(binder_name);
                }
            }
        }
        let body = node
            .children()
            .find(|c| is_type_kind(c.kind()) && c.kind() != SyntaxKind::TYPE_BINDER_LIST)
            .map(|ty| self.lower_type(&ty))
            .unwrap_or_else(|| unknown(self.db));
        self.binders.truncate(saved);
        Some(NamedDefinition {
            alias,
            name: Name::new(self.db, name),
            parameters,
            body,
        })
    }

    fn lower_type(&mut self, node: &SyntaxNode) -> Ty<'db> {
        match node.kind() {
            SyntaxKind::TYPE_REF => self.lower_type_ref(node),
            SyntaxKind::TYPE_PAREN => node
                .children()
                .find(|c| is_type_kind(c.kind()))
                .map(|inner| self.lower_type(&inner))
                .unwrap_or_else(|| unknown(self.db)),
            SyntaxKind::TYPE_UNION => {
                let members: Vec<Ty<'db>> = node
                    .children()
                    .filter(|c| is_type_kind(c.kind()))
                    .map(|member| self.lower_type(&member))
                    .collect();
                union_of(self.db, members)
            }
            SyntaxKind::TYPE_VECTOR => {
                let element = node
                    .children()
                    .find(|c| is_type_kind(c.kind()))
                    .map(|inner| self.lower_type(&inner))
                    .unwrap_or_else(|| unknown(self.db));
                let named = node
                    .children_with_tokens()
                    .filter_map(|element| element.into_token())
                    .any(|token| token.kind() == SyntaxKind::IDENT && token.text() == "named");
                if named {
                    Ty::new(self.db, TyKind::NamedVector(element))
                } else {
                    Ty::new(self.db, TyKind::Vector(element))
                }
            }
            SyntaxKind::TYPE_LIST => {
                let element = node
                    .children()
                    .find(|c| is_type_kind(c.kind()))
                    .map(|inner| self.lower_type(&inner))
                    .unwrap_or_else(|| unknown(self.db));
                let named = node
                    .children_with_tokens()
                    .filter_map(|element| element.into_token())
                    .any(|token| token.kind() == SyntaxKind::IDENT && token.text() == "named");
                if named {
                    Ty::new(self.db, TyKind::NamedList(element))
                } else {
                    Ty::new(self.db, TyKind::List(element))
                }
            }
            SyntaxKind::TYPE_TUPLE => {
                let items = node
                    .children()
                    .filter(|c| is_type_kind(c.kind()))
                    .map(|item| self.lower_type(&item))
                    .collect();
                Ty::new(self.db, TyKind::Tuple(items))
            }
            SyntaxKind::TYPE_RECORD => {
                let fields = node
                    .children()
                    .filter(|c| c.kind() == SyntaxKind::TYPE_FIELD)
                    .filter_map(|field| {
                        let name = field
                            .children()
                            .find(|c| c.kind() == SyntaxKind::NAME)
                            .and_then(syntax::ast::Name::cast)
                            .and_then(|name| name.text())?;
                        let ty = field
                            .children()
                            .find(|c| is_type_kind(c.kind()))
                            .map(|ty| self.lower_type(&ty))
                            .unwrap_or_else(|| unknown(self.db));
                        Some(RecordField {
                            name: Name::new(self.db, name),
                            ty,
                            optional: false,
                        })
                    })
                    .collect();
                Ty::new(self.db, TyKind::Record(fields))
            }
            SyntaxKind::TYPE_APPLY => {
                let name = node
                    .children()
                    .find(|c| c.kind() == SyntaxKind::NAME)
                    .and_then(syntax::ast::Name::cast)
                    .and_then(|name| name.text())
                    .unwrap_or_default();
                let arguments = node
                    .children()
                    .find(|c| c.kind() == SyntaxKind::TYPE_ARG_LIST)
                    .map(|list| {
                        list.children()
                            .filter(|c| is_type_kind(c.kind()))
                            .map(|argument| self.lower_type(&argument))
                            .collect()
                    })
                    .unwrap_or_default();
                Ty::new(self.db, TyKind::Named(Name::new(self.db, name), arguments))
            }
            SyntaxKind::TYPE_FUNCTION => self.lower_function_type(node),
            _ => unknown(self.db),
        }
    }

    fn lower_type_ref(&mut self, node: &SyntaxNode) -> Ty<'db> {
        let Some(token) = node.first_token() else {
            return unknown(self.db);
        };
        let text = token.text();
        match text {
            "Any" | "any" => return any(self.db),
            "Unknown" | "unknown" => return unknown(self.db),
            "NULL" | "null" => return null(self.db),
            "logical" => return scalar(self.db, Atomic::Logical),
            "integer" => return scalar(self.db, Atomic::Integer),
            "double" => return scalar(self.db, Atomic::Double),
            "complex" => return scalar(self.db, Atomic::Complex),
            "character" => return scalar(self.db, Atomic::Character),
            "raw" => return scalar(self.db, Atomic::Raw),
            _ => {}
        }
        let name = Name::new(self.db, text.to_owned());
        if self.binders.contains(&name) {
            return Ty::new(self.db, TyKind::Rigid(name));
        }
        Ty::new(self.db, TyKind::Named(name, Vec::new()))
    }

    fn lower_function_type(&mut self, node: &SyntaxNode) -> Ty<'db> {
        let mut positional = Vec::new();
        let mut named = Vec::new();
        let mut variadic = None;
        if let Some(list) = node
            .children()
            .find(|c| c.kind() == SyntaxKind::TYPE_PARAMETER_LIST)
        {
            for parameter in list
                .children()
                .filter(|c| c.kind() == SyntaxKind::TYPE_PARAMETER)
            {
                let has_dots = parameter
                    .children_with_tokens()
                    .filter_map(|element| element.into_token())
                    .any(|token| token.kind() == SyntaxKind::DOTS);
                let optional = parameter
                    .children_with_tokens()
                    .filter_map(|element| element.into_token())
                    .any(|token| token.kind() == SyntaxKind::L_BRACKET);
                let name = parameter
                    .children()
                    .find(|c| c.kind() == SyntaxKind::NAME)
                    .and_then(syntax::ast::Name::cast)
                    .and_then(|name| name.text());
                let ty = parameter
                    .children()
                    .find(|c| is_type_kind(c.kind()))
                    .map(|ty| self.lower_type(&ty));
                if has_dots {
                    variadic = Some(RestParameter {
                        element: ty.unwrap_or_else(|| any(self.db)),
                        preceding_named: named.len(),
                    });
                } else if let Some(name) = name {
                    named.push(RecordField {
                        name: Name::new(self.db, name),
                        ty: ty.unwrap_or_else(|| unknown(self.db)),
                        optional,
                    });
                } else {
                    positional.push(ty.unwrap_or_else(|| unknown(self.db)));
                }
            }
        }
        let ret = node
            .children()
            .filter(|c| is_type_kind(c.kind()))
            .last()
            .filter(|_| {
                node.children_with_tokens()
                    .filter_map(|element| element.into_token())
                    .any(|token| token.kind() == SyntaxKind::MINUS_GREATER)
            })
            .map(|ty| self.lower_type(&ty))
            .unwrap_or_else(|| unknown(self.db));
        Ty::new(
            self.db,
            TyKind::Function(FunctionType {
                positional,
                named,
                variadic,
                ret,
            }),
        )
    }
}
