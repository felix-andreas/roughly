use {
    ropey::Rope,
    std::{
        collections::{HashMap, HashSet},
        path::{Path, PathBuf},
    },
    thiserror::Error,
    tree_sitter::{InputEdit, Parser, Point, Tree},
};

pub struct Workspace {
    parser: Parser,
    next_document_identifier: u64,
    package_roots_by_name: HashMap<String, PathBuf>,
    package_names_by_root: HashMap<PathBuf, String>,
    packages_by_name: HashMap<String, Package>,
    documents_by_identifier: HashMap<DocumentIdentifier, ManagedDocument>,
    document_identifiers_by_path: HashMap<PathBuf, DocumentIdentifier>,
}

#[derive(Debug)]
pub struct Document {
    rope: Rope,
    tree: Tree,
}

#[derive(Debug, Clone, Copy)]
pub struct DocumentView<'a> {
    document: &'a Document,
    kind: &'a DocumentKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentKind {
    Package { package_name: String },
    Auxiliary { package_name: String },
    Standalone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextPosition {
    pub line_index: usize,
    pub character_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextRange {
    pub start: TextPosition,
    pub end: TextPosition,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum WorkspaceError {
    #[error("failed to initialize the R parser")]
    ParserInitializationFailed,
    #[error("package `{0}` already exists")]
    PackageAlreadyExists(String),
    #[error("package `{0}` was not found")]
    PackageNotFound(String),
    #[error("package root `{package_root}` is already assigned to package `{package_name}`")]
    PackageRootAlreadyAssigned {
        package_root: PathBuf,
        package_name: String,
    },
    #[error("document `{0}` was not found")]
    DocumentNotFound(PathBuf),
    #[error("document `{0}` already exists")]
    DocumentAlreadyExists(PathBuf),
    #[error("invalid edit range for document `{path}`")]
    InvalidEditRange { path: PathBuf },
    #[error("failed to parse document `{0}`")]
    ParseFailed(PathBuf),
}

impl Workspace {
    pub fn new() -> Result<Self, WorkspaceError> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_r::LANGUAGE.into())
            .map_err(|_| WorkspaceError::ParserInitializationFailed)?;

        Ok(Self {
            parser,
            next_document_identifier: 0,
            package_roots_by_name: HashMap::new(),
            package_names_by_root: HashMap::new(),
            packages_by_name: HashMap::new(),
            documents_by_identifier: HashMap::new(),
            document_identifiers_by_path: HashMap::new(),
        })
    }

    pub fn register_package(
        &mut self,
        package_name: impl Into<String>,
    ) -> Result<(), WorkspaceError> {
        let package_name = package_name.into();
        if self.packages_by_name.contains_key(&package_name) {
            return Err(WorkspaceError::PackageAlreadyExists(package_name));
        }

        self.packages_by_name
            .insert(package_name, Package::default());
        Ok(())
    }

    pub fn register_package_root(
        &mut self,
        package_name: &str,
        package_root: impl Into<PathBuf>,
    ) -> Result<(), WorkspaceError> {
        if !self.packages_by_name.contains_key(package_name) {
            return Err(WorkspaceError::PackageNotFound(package_name.to_owned()));
        }

        let package_root = package_root.into();

        if let Some(existing_package_name) = self.package_names_by_root.get(&package_root) {
            if existing_package_name != package_name {
                return Err(WorkspaceError::PackageRootAlreadyAssigned {
                    package_root,
                    package_name: existing_package_name.clone(),
                });
            }
        }

        if let Some(previous_root) = self
            .package_roots_by_name
            .insert(package_name.to_owned(), package_root.clone())
        {
            self.package_names_by_root.remove(&previous_root);
        }

        self.package_names_by_root
            .insert(package_root, package_name.to_owned());

        Ok(())
    }

    pub fn package_root(&self, package_name: &str) -> Option<&Path> {
        self.package_roots_by_name
            .get(package_name)
            .map(PathBuf::as_path)
    }

    pub fn package_name_for_root(&self, package_root: impl AsRef<Path>) -> Option<&str> {
        self.package_names_by_root
            .get(package_root.as_ref())
            .map(String::as_str)
    }

    pub fn package_name_for_path(&self, path: impl AsRef<Path>) -> Option<&str> {
        let path = path.as_ref();

        self.package_names_by_root
            .iter()
            .filter(|(package_root, _)| path.starts_with(package_root))
            .max_by_key(|(package_root, _)| package_root.components().count())
            .map(|(_, package_name)| package_name.as_str())
    }

    pub fn replace_document(
        &mut self,
        path: impl Into<PathBuf>,
        source: &str,
        kind: DocumentKind,
    ) -> Result<(), WorkspaceError> {
        self.ensure_document_kind_is_valid(&kind)?;
        self.replace_document_inner(path.into(), source, kind)
    }

    pub fn document(&self, path: impl AsRef<Path>) -> Option<DocumentView<'_>> {
        let document_identifier = self.document_identifiers_by_path.get(path.as_ref())?;

        self.documents_by_identifier
            .get(document_identifier)
            .map(|managed_document| DocumentView {
                document: &managed_document.document,
                kind: &managed_document.kind,
            })
    }

    pub fn edit_document_range(
        &mut self,
        path: impl AsRef<Path>,
        range: TextRange,
        replacement_text: &str,
    ) -> Result<(), WorkspaceError> {
        let path = path.as_ref().to_path_buf();
        let document_identifier = *self
            .document_identifiers_by_path
            .get(&path)
            .ok_or_else(|| WorkspaceError::DocumentNotFound(path.clone()))?;

        let managed_document = self
            .documents_by_identifier
            .get_mut(&document_identifier)
            .ok_or_else(|| WorkspaceError::DocumentNotFound(path.clone()))?;

        let start_character =
            line_character_to_character_index(&managed_document.document.rope, range.start)
                .ok_or_else(|| WorkspaceError::InvalidEditRange { path: path.clone() })?;
        let end_character =
            line_character_to_character_index(&managed_document.document.rope, range.end)
                .ok_or_else(|| WorkspaceError::InvalidEditRange { path: path.clone() })?;

        if start_character > end_character {
            return Err(WorkspaceError::InvalidEditRange { path });
        }

        let start_byte = managed_document
            .document
            .rope
            .try_char_to_byte(start_character)
            .map_err(|_| WorkspaceError::InvalidEditRange { path: path.clone() })?;
        let old_end_byte = managed_document
            .document
            .rope
            .try_char_to_byte(end_character)
            .map_err(|_| WorkspaceError::InvalidEditRange { path: path.clone() })?;

        managed_document
            .document
            .rope
            .remove(start_character..end_character);
        managed_document
            .document
            .rope
            .insert(start_character, replacement_text);

        let new_end_byte = start_byte + replacement_text.len();
        let new_end_line = managed_document
            .document
            .rope
            .try_byte_to_line(new_end_byte)
            .map_err(|_| WorkspaceError::InvalidEditRange { path: path.clone() })?;
        let line_start_character = managed_document
            .document
            .rope
            .try_line_to_char(new_end_line)
            .map_err(|_| WorkspaceError::InvalidEditRange { path: path.clone() })?;
        let new_end_character = managed_document
            .document
            .rope
            .try_byte_to_char(new_end_byte)
            .map_err(|_| WorkspaceError::InvalidEditRange { path: path.clone() })?;

        managed_document.document.tree.edit(&InputEdit {
            start_byte,
            old_end_byte,
            new_end_byte,
            start_position: point_from_text_position(range.start),
            old_end_position: point_from_text_position(range.end),
            new_end_position: Point::new(
                new_end_line,
                new_end_character.saturating_sub(line_start_character),
            ),
        });

        managed_document.document.tree = parse_rope(
            &mut self.parser,
            &managed_document.document.rope,
            Some(&managed_document.document.tree),
            &path,
        )?;

        Ok(())
    }

    pub fn delete_document(&mut self, path: impl AsRef<Path>) -> Result<(), WorkspaceError> {
        let path = path.as_ref().to_path_buf();
        let document_identifier = self
            .document_identifiers_by_path
            .remove(&path)
            .ok_or_else(|| WorkspaceError::DocumentNotFound(path.clone()))?;

        let managed_document = self
            .documents_by_identifier
            .remove(&document_identifier)
            .ok_or_else(|| WorkspaceError::DocumentNotFound(path.clone()))?;

        self.detach_document(document_identifier, &managed_document.kind)?;
        Ok(())
    }

    pub fn move_document(
        &mut self,
        source_path: impl AsRef<Path>,
        destination_path: impl Into<PathBuf>,
    ) -> Result<(), WorkspaceError> {
        let source_path = source_path.as_ref().to_path_buf();
        let destination_path = destination_path.into();

        if self
            .document_identifiers_by_path
            .contains_key(&destination_path)
        {
            return Err(WorkspaceError::DocumentAlreadyExists(destination_path));
        }

        let document_identifier = self
            .document_identifiers_by_path
            .remove(&source_path)
            .ok_or_else(|| WorkspaceError::DocumentNotFound(source_path.clone()))?;

        self.document_identifiers_by_path
            .insert(destination_path.clone(), document_identifier);

        let managed_document = self
            .documents_by_identifier
            .get_mut(&document_identifier)
            .ok_or_else(|| WorkspaceError::DocumentNotFound(source_path))?;
        managed_document.path = destination_path;

        Ok(())
    }

    fn ensure_document_kind_is_valid(&self, kind: &DocumentKind) -> Result<(), WorkspaceError> {
        match kind {
            DocumentKind::Package { package_name } | DocumentKind::Auxiliary { package_name } => {
                if self.packages_by_name.contains_key(package_name) {
                    Ok(())
                } else {
                    Err(WorkspaceError::PackageNotFound(package_name.clone()))
                }
            }
            DocumentKind::Standalone => Ok(()),
        }
    }

    fn replace_document_inner(
        &mut self,
        path: PathBuf,
        source: &str,
        kind: DocumentKind,
    ) -> Result<(), WorkspaceError> {
        if let Some(document_identifier) = self.document_identifiers_by_path.get(&path).copied() {
            let previous_tree = self
                .documents_by_identifier
                .get(&document_identifier)
                .map(|managed_document| managed_document.document.tree.clone())
                .ok_or_else(|| WorkspaceError::DocumentNotFound(path.clone()))?;

            let document = parse_document(&mut self.parser, source, Some(&previous_tree), &path)?;

            let previous_kind = self
                .documents_by_identifier
                .get(&document_identifier)
                .map(|managed_document| managed_document.kind.clone())
                .ok_or_else(|| WorkspaceError::DocumentNotFound(path.clone()))?;

            self.detach_document(document_identifier, &previous_kind)?;
            self.attach_document(document_identifier, &kind)?;

            let managed_document = self
                .documents_by_identifier
                .get_mut(&document_identifier)
                .ok_or_else(|| WorkspaceError::DocumentNotFound(path.clone()))?;
            managed_document.document = document;
            managed_document.kind = kind;

            return Ok(());
        }

        let document_identifier = self.next_document_identifier();
        let document = parse_document(&mut self.parser, source, None, &path)?;

        self.attach_document(document_identifier, &kind)?;
        self.document_identifiers_by_path
            .insert(path.clone(), document_identifier);
        self.documents_by_identifier.insert(
            document_identifier,
            ManagedDocument {
                path,
                kind,
                document,
            },
        );

        Ok(())
    }

    fn attach_document(
        &mut self,
        document_identifier: DocumentIdentifier,
        kind: &DocumentKind,
    ) -> Result<(), WorkspaceError> {
        match kind {
            DocumentKind::Package { package_name } => {
                let package = self
                    .packages_by_name
                    .get_mut(package_name)
                    .ok_or_else(|| WorkspaceError::PackageNotFound(package_name.clone()))?;
                package
                    .package_document_identifiers
                    .insert(document_identifier);
            }
            DocumentKind::Auxiliary { package_name } => {
                let package = self
                    .packages_by_name
                    .get_mut(package_name)
                    .ok_or_else(|| WorkspaceError::PackageNotFound(package_name.clone()))?;
                package
                    .auxiliary_document_identifiers
                    .insert(document_identifier);
            }
            DocumentKind::Standalone => {}
        }

        Ok(())
    }

    fn detach_document(
        &mut self,
        document_identifier: DocumentIdentifier,
        kind: &DocumentKind,
    ) -> Result<(), WorkspaceError> {
        match kind {
            DocumentKind::Package { package_name } => {
                let package = self
                    .packages_by_name
                    .get_mut(package_name)
                    .ok_or_else(|| WorkspaceError::PackageNotFound(package_name.clone()))?;
                package
                    .package_document_identifiers
                    .remove(&document_identifier);
            }
            DocumentKind::Auxiliary { package_name } => {
                let package = self
                    .packages_by_name
                    .get_mut(package_name)
                    .ok_or_else(|| WorkspaceError::PackageNotFound(package_name.clone()))?;
                package
                    .auxiliary_document_identifiers
                    .remove(&document_identifier);
            }
            DocumentKind::Standalone => {}
        }

        Ok(())
    }

    fn next_document_identifier(&mut self) -> DocumentIdentifier {
        let document_identifier = DocumentIdentifier(self.next_document_identifier);
        self.next_document_identifier += 1;
        document_identifier
    }
}

impl Document {
    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    pub fn tree(&self) -> &Tree {
        &self.tree
    }
}

impl<'a> DocumentView<'a> {
    pub fn document(self) -> &'a Document {
        self.document
    }

    pub fn kind(self) -> &'a DocumentKind {
        self.kind
    }

    pub fn package_name(self) -> Option<&'a str> {
        self.kind.package_name()
    }
}

impl DocumentKind {
    pub fn package_name(&self) -> Option<&str> {
        match self {
            Self::Package { package_name } | Self::Auxiliary { package_name } => {
                Some(package_name.as_str())
            }
            Self::Standalone => None,
        }
    }
}

#[derive(Default)]
struct Package {
    package_document_identifiers: HashSet<DocumentIdentifier>,
    auxiliary_document_identifiers: HashSet<DocumentIdentifier>,
}

#[derive(Debug)]
struct ManagedDocument {
    path: PathBuf,
    kind: DocumentKind,
    document: Document,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct DocumentIdentifier(u64);

fn parse_document(
    parser: &mut Parser,
    source: &str,
    previous_tree: Option<&Tree>,
    path: &Path,
) -> Result<Document, WorkspaceError> {
    let rope = Rope::from_str(source);
    let tree = parse_rope(parser, &rope, previous_tree, path)?;
    Ok(Document { rope, tree })
}

fn parse_rope(
    parser: &mut Parser,
    rope: &Rope,
    previous_tree: Option<&Tree>,
    path: &Path,
) -> Result<Tree, WorkspaceError> {
    let mut lookup = |byte, _position| {
        let (chunk, chunk_byte, _, _) = rope.chunk_at_byte(byte);
        let offset = byte - chunk_byte;
        &chunk.as_bytes()[offset..]
    };

    parser
        .parse_with_options(&mut lookup, previous_tree, None)
        .ok_or_else(|| WorkspaceError::ParseFailed(path.to_path_buf()))
}

fn line_character_to_character_index(rope: &Rope, position: TextPosition) -> Option<usize> {
    let line_start_character = rope.try_line_to_char(position.line_index).ok()?;
    let line_text = rope.get_line(position.line_index)?;
    let line_character_count = line_text.len_chars();

    (position.character_index <= line_character_count)
        .then_some(line_start_character + position.character_index)
}

fn point_from_text_position(position: TextPosition) -> Point {
    Point::new(position.line_index, position.character_index)
}

#[cfg(test)]
mod tests {
    use {
        super::{DocumentKind, TextPosition, TextRange, Workspace},
        indoc::indoc,
        std::path::PathBuf,
    };

    #[test]
    fn registers_packages_and_roots() {
        let mut workspace = Workspace::new().expect("workspace should initialize");

        workspace
            .register_package("alpha")
            .expect("package should register");
        workspace
            .register_package("beta")
            .expect("package should register");
        workspace
            .register_package_root("alpha", "/repo/packages/alpha")
            .expect("package root should register");

        assert_eq!(
            workspace.package_root("alpha"),
            Some(PathBuf::from("/repo/packages/alpha").as_path())
        );
        assert_eq!(
            workspace.package_name_for_root("/repo/packages/alpha"),
            Some("alpha")
        );
        assert_eq!(
            workspace.package_name_for_path("/repo/packages/alpha/R/file.R"),
            Some("alpha")
        );
        assert_eq!(
            workspace.register_package_root("beta", "/repo/packages/alpha"),
            Err(super::WorkspaceError::PackageRootAlreadyAssigned {
                package_root: PathBuf::from("/repo/packages/alpha"),
                package_name: "alpha".to_owned()
            })
        );
    }

    #[test]
    fn keeps_documents_in_exactly_one_bucket() {
        let mut workspace = Workspace::new().expect("workspace should initialize");
        let path = PathBuf::from("/repo/packages/alpha/R/file.R");

        workspace
            .register_package("alpha")
            .expect("package should register");

        workspace
            .replace_document(
                path.clone(),
                "value <- 1\n",
                DocumentKind::Package {
                    package_name: "alpha".to_owned(),
                },
            )
            .expect("package document should insert");
        let document = workspace.document(&path).expect("document should exist");
        assert_eq!(
            document.kind(),
            &DocumentKind::Package {
                package_name: "alpha".to_owned()
            }
        );
        assert_eq!(document.package_name(), Some("alpha"));

        workspace
            .replace_document(
                path.clone(),
                "value <- 2\n",
                DocumentKind::Auxiliary {
                    package_name: "alpha".to_owned(),
                },
            )
            .expect("auxiliary document should replace");
        let document = workspace.document(&path).expect("document should exist");
        assert_eq!(
            document.kind(),
            &DocumentKind::Auxiliary {
                package_name: "alpha".to_owned()
            }
        );
        assert_eq!(document.package_name(), Some("alpha"));

        workspace
            .replace_document(path.clone(), "value <- 3\n", DocumentKind::Standalone)
            .expect("standalone document should replace");
        let document = workspace.document(&path).expect("document should exist");
        assert_eq!(document.kind(), &DocumentKind::Standalone);
        assert_eq!(document.package_name(), None);
    }

    #[test]
    fn edits_documents_incrementally() {
        let mut workspace = Workspace::new().expect("workspace should initialize");
        let path = PathBuf::from("/workspace/file.R");

        workspace
            .replace_document(
                path.clone(),
                indoc! {"
                    alpha <- 1
                    beta <- 2
                    gamma <- alpha + beta
                "},
                DocumentKind::Standalone,
            )
            .expect("standalone document should insert");

        let unchanged_node_identifier = workspace
            .document(&path)
            .and_then(|document| document.document().tree().root_node().child(0))
            .map(|node| node.id())
            .expect("first node should exist");

        workspace
            .edit_document_range(
                &path,
                TextRange {
                    start: TextPosition {
                        line_index: 1,
                        character_index: 8,
                    },
                    end: TextPosition {
                        line_index: 1,
                        character_index: 9,
                    },
                },
                "200",
            )
            .expect("edit should succeed");

        let document = workspace.document(&path).expect("document should exist");
        assert_eq!(
            document.document().rope().to_string(),
            indoc! {"
                alpha <- 1
                beta <- 200
                gamma <- alpha + beta
            "}
        );

        let updated_node_identifier = document
            .document()
            .tree()
            .root_node()
            .child(0)
            .map(|node| node.id())
            .expect("first node should exist after edit");
        assert_eq!(updated_node_identifier, unchanged_node_identifier);
    }

    #[test]
    fn moves_and_deletes_documents_without_changing_bucket() {
        let mut workspace = Workspace::new().expect("workspace should initialize");
        let source_path = PathBuf::from("/repo/packages/alpha/R/file.R");
        let destination_path = PathBuf::from("/repo/packages/alpha/tests/file.R");

        workspace
            .register_package("alpha")
            .expect("package should register");
        workspace
            .replace_document(
                source_path.clone(),
                "value <- 1\n",
                DocumentKind::Auxiliary {
                    package_name: "alpha".to_owned(),
                },
            )
            .expect("auxiliary document should insert");

        workspace
            .move_document(&source_path, destination_path.clone())
            .expect("move should succeed");

        assert!(workspace.document(&source_path).is_none());
        let document = workspace
            .document(&destination_path)
            .expect("moved document should exist");
        assert_eq!(
            document.kind(),
            &DocumentKind::Auxiliary {
                package_name: "alpha".to_owned()
            }
        );
        assert_eq!(document.package_name(), Some("alpha"));

        workspace
            .delete_document(&destination_path)
            .expect("delete should succeed");

        assert!(workspace.document(&destination_path).is_none());
    }
}
