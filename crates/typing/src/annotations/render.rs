use crate::{
    annotations::parse::TypeSyntaxItem,
    interner::Interner,
    types::{Atomic, SurfaceType},
};

pub fn render_type_syntax_item(interner: &Interner, item: &TypeSyntaxItem) -> String {
    let mut renderer = SurfaceTypeRenderer::new(interner);

    match item {
        TypeSyntaxItem::SurfaceType(surface_type) => renderer.render(surface_type),
        TypeSyntaxItem::IfUnknown(surface_type) => {
            format!("@if-unknown {}", renderer.render(surface_type))
        }
        TypeSyntaxItem::Trust(surface_type) => {
            format!("@trust {}", renderer.render(surface_type))
        }
        TypeSyntaxItem::TypeDefinition { name, surface_type } => {
            let name = interner.resolve(*name).unwrap_or("<unknown>");
            format!("@type {name} {{{}}}", renderer.render(surface_type))
        }
        TypeSyntaxItem::TypeAlias { name, surface_type } => {
            let name = interner.resolve(*name).unwrap_or("<unknown>");
            format!("@alias {name} {{{}}}", renderer.render(surface_type))
        }
    }
}

pub fn render_surface_type(interner: &Interner, surface_type: &SurfaceType) -> String {
    let mut renderer = SurfaceTypeRenderer::new(interner);
    renderer.render(surface_type)
}

struct SurfaceTypeRenderer<'a> {
    interner: &'a Interner,
}

impl<'a> SurfaceTypeRenderer<'a> {
    fn new(interner: &'a Interner) -> Self {
        Self { interner }
    }

    fn render(&mut self, surface_type: &SurfaceType) -> String {
        match surface_type {
            SurfaceType::Any => "Any".to_owned(),
            SurfaceType::Unknown => "Unknown".to_owned(),
            SurfaceType::Null => "NULL".to_owned(),
            SurfaceType::Nullable(inner_type) => format!("{} | NULL", self.render(inner_type)),
            SurfaceType::Scalar(atomic) => render_atomic(*atomic).to_owned(),
            SurfaceType::Named(name) => self
                .interner
                .resolve(*name)
                .unwrap_or("<unknown>")
                .to_owned(),
            SurfaceType::Vector(inner_type) => format!("{}[]", self.render(inner_type)),
            SurfaceType::NamedVector(inner_type) => format!("{}[named]", self.render(inner_type)),
            SurfaceType::List(item_type) => format!("list[{}]", self.render(item_type)),
            SurfaceType::NamedList(item_type) => {
                format!("list[named: {}]", self.render(item_type))
            }
            SurfaceType::Record(fields) => {
                let rendered_fields = fields
                    .iter()
                    .map(|field| {
                        let name = self.interner.resolve(field.name).unwrap_or("<unknown>");
                        format!("{name}: {}", self.render(&field.value))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("list{{{rendered_fields}}}")
            }
            SurfaceType::Tuple(items) => {
                let rendered_items = items
                    .iter()
                    .map(|item| self.render(item))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("list{{{rendered_items}}}")
            }
            SurfaceType::Function(function_type) => {
                let rendered_parameters = function_type
                    .parameters
                    .iter()
                    .map(|parameter| self.render(parameter))
                    .collect::<Vec<_>>();
                let rendered_named_parameters = function_type
                    .named_parameters
                    .iter()
                    .map(|parameter| {
                        let name = self.interner.resolve(parameter.name).unwrap_or("<unknown>");
                        format!("{name}: {}", self.render(&parameter.value))
                    })
                    .collect::<Vec<_>>();
                let mut rendered_parts = rendered_parameters;
                rendered_parts.extend(rendered_named_parameters);
                format!(
                    "fn({}) -> {}",
                    rendered_parts.join(", "),
                    self.render(&function_type.return_type)
                )
            }
        }
    }
}

fn render_atomic(atomic: Atomic) -> &'static str {
    match atomic {
        Atomic::Logical => "logical",
        Atomic::Integer => "integer",
        Atomic::Double => "double",
        Atomic::Complex => "complex",
        Atomic::Character => "character",
        Atomic::Raw => "raw",
    }
}
