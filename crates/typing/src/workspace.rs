use {
    ropey::Rope,
    std::{
        collections::HashMap,
        path::{Path, PathBuf},
    },
    thiserror::Error,
    tree_sitter::{InputEdit, Parser, Point, Tree},
};

pub struct Workspace {
    parser: Parser,
    packages: HashMap<PathBuf, Package>,
    /// Scripts not attached to any package.
    scripts: HashMap<PathBuf, Document>,
}

#[derive(Clone, Default)]
pub struct Package {
    documents: HashMap<PathBuf, Document>,
    /// Scripts attached to a package. They can resolve against the package namespace, but they do
    /// not contribute back to that namespace.
    scripts: HashMap<PathBuf, Document>,
}

#[derive(Debug, Clone)]
pub struct Document {
    rope: Rope,
    tree: Tree,
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
    #[error("package `{0}` was not found")]
    PackageNotFound(PathBuf),
    #[error("package `{0}` already exists")]
    PackageAlreadyExists(PathBuf),
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
            packages: HashMap::new(),
            scripts: HashMap::new(),
        })
    }

    pub fn insert_package(&mut self, path: PathBuf) -> Result<(), WorkspaceError> {
        if self.packages.contains_key(&path) {
            return Err(WorkspaceError::PackageAlreadyExists(path));
        }

        self.packages.insert(path, Package::default());
        Ok(())
    }

    pub fn insert_workspace_script(
        &mut self,
        path: PathBuf,
        source: &str,
    ) -> Result<(), WorkspaceError> {
        self.insert_document(DocumentBucket::WorkspaceScript, path, source)
    }

    pub fn insert_package_document(
        &mut self,
        package_path: &Path,
        path: PathBuf,
        source: &str,
    ) -> Result<(), WorkspaceError> {
        self.insert_document(
            DocumentBucket::PackageDocument(package_path.to_path_buf()),
            path,
            source,
        )
    }

    pub fn insert_package_script(
        &mut self,
        package_path: &Path,
        path: PathBuf,
        source: &str,
    ) -> Result<(), WorkspaceError> {
        self.insert_document(
            DocumentBucket::PackageScript(package_path.to_path_buf()),
            path,
            source,
        )
    }

    pub fn package(&self, path: &Path) -> Option<&Package> {
        self.packages.get(path)
    }

    pub fn package_mut(&mut self, path: &Path) -> Option<&mut Package> {
        self.packages.get_mut(path)
    }

    pub fn document(&self, path: &Path) -> Option<&Document> {
        if let Some(document) = self.scripts.get(path) {
            return Some(document);
        }

        for package in self.packages.values() {
            if let Some(document) = package.document(path) {
                return Some(document);
            }
        }

        None
    }

    pub fn edit_document_range(
        &mut self,
        path: &Path,
        range: TextRange,
        replacement_text: &str,
    ) -> Result<(), WorkspaceError> {
        let (bucket, mut document) = self
            .take_document(path)
            .ok_or_else(|| WorkspaceError::DocumentNotFound(path.to_path_buf()))?;

        let start_character = line_character_to_character_index(&document.rope, range.start)
            .ok_or_else(|| WorkspaceError::InvalidEditRange {
                path: path.to_path_buf(),
            })?;
        let end_character = line_character_to_character_index(&document.rope, range.end)
            .ok_or_else(|| WorkspaceError::InvalidEditRange {
                path: path.to_path_buf(),
            })?;

        if start_character > end_character {
            self.insert_document_at(bucket, path.to_path_buf(), document)?;
            return Err(WorkspaceError::InvalidEditRange {
                path: path.to_path_buf(),
            });
        }

        let start_byte = document
            .rope
            .try_char_to_byte(start_character)
            .map_err(|_| WorkspaceError::InvalidEditRange {
                path: path.to_path_buf(),
            })?;
        let old_end_byte = document.rope.try_char_to_byte(end_character).map_err(|_| {
            WorkspaceError::InvalidEditRange {
                path: path.to_path_buf(),
            }
        })?;

        document.rope.remove(start_character..end_character);
        document.rope.insert(start_character, replacement_text);

        let new_end_byte = start_byte + replacement_text.len();
        let new_end_line = document.rope.try_byte_to_line(new_end_byte).map_err(|_| {
            WorkspaceError::InvalidEditRange {
                path: path.to_path_buf(),
            }
        })?;
        let line_start_character = document.rope.try_line_to_char(new_end_line).map_err(|_| {
            WorkspaceError::InvalidEditRange {
                path: path.to_path_buf(),
            }
        })?;
        let new_end_character = document.rope.try_byte_to_char(new_end_byte).map_err(|_| {
            WorkspaceError::InvalidEditRange {
                path: path.to_path_buf(),
            }
        })?;

        document.tree.edit(&InputEdit {
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

        let previous_tree = document.tree.clone();
        let reparsed_tree =
            parse_rope(&mut self.parser, &document.rope, Some(&previous_tree), path)?;
        document.tree = reparsed_tree;
        self.insert_document_at(bucket, path.to_path_buf(), document)
    }

    pub fn delete_document(&mut self, path: &Path) -> Result<(), WorkspaceError> {
        self.take_document(path)
            .map(|_| ())
            .ok_or_else(|| WorkspaceError::DocumentNotFound(path.to_path_buf()))
    }

    pub fn rename_document(
        &mut self,
        source_path: &Path,
        destination_path: PathBuf,
    ) -> Result<(), WorkspaceError> {
        if self.document(&destination_path).is_some() {
            return Err(WorkspaceError::DocumentAlreadyExists(destination_path));
        }

        let (bucket, document) = self
            .take_document(source_path)
            .ok_or_else(|| WorkspaceError::DocumentNotFound(source_path.to_path_buf()))?;

        self.insert_document_at(bucket, destination_path, document)
    }

    fn insert_document(
        &mut self,
        bucket: DocumentBucket,
        path: PathBuf,
        source: &str,
    ) -> Result<(), WorkspaceError> {
        if self.document(&path).is_some() {
            return Err(WorkspaceError::DocumentAlreadyExists(path));
        }

        let document = self.parse_document(source, &path)?;
        self.insert_document_at(bucket, path, document)
    }

    fn insert_document_at(
        &mut self,
        bucket: DocumentBucket,
        path: PathBuf,
        document: Document,
    ) -> Result<(), WorkspaceError> {
        match bucket {
            DocumentBucket::WorkspaceScript => {
                self.scripts.insert(path, document);
                Ok(())
            }
            DocumentBucket::PackageDocument(package_path) => {
                let package = self
                    .packages
                    .get_mut(&package_path)
                    .ok_or(WorkspaceError::PackageNotFound(package_path))?;
                package.documents.insert(path, document);
                Ok(())
            }
            DocumentBucket::PackageScript(package_path) => {
                let package = self
                    .packages
                    .get_mut(&package_path)
                    .ok_or(WorkspaceError::PackageNotFound(package_path))?;
                package.scripts.insert(path, document);
                Ok(())
            }
        }
    }

    fn take_document(&mut self, path: &Path) -> Option<(DocumentBucket, Document)> {
        if let Some(document) = self.scripts.remove(path) {
            return Some((DocumentBucket::WorkspaceScript, document));
        }

        for (package_path, package) in &mut self.packages {
            if let Some(document) = package.documents.remove(path) {
                return Some((
                    DocumentBucket::PackageDocument(package_path.clone()),
                    document,
                ));
            }
            if let Some(document) = package.scripts.remove(path) {
                return Some((
                    DocumentBucket::PackageScript(package_path.clone()),
                    document,
                ));
            }
        }

        None
    }

    fn parse_document(&mut self, source: &str, path: &Path) -> Result<Document, WorkspaceError> {
        let rope = Rope::from_str(source);
        let tree = parse_rope(&mut self.parser, &rope, None, path)?;
        Ok(Document { rope, tree })
    }
}

impl Package {
    pub fn document(&self, path: &Path) -> Option<&Document> {
        self.documents.get(path).or_else(|| self.scripts.get(path))
    }

    pub fn documents(&self) -> impl Iterator<Item = (&PathBuf, &Document)> {
        self.documents.iter()
    }

    pub fn scripts(&self) -> impl Iterator<Item = (&PathBuf, &Document)> {
        self.scripts.iter()
    }
}

impl Document {
    pub fn new(rope: Rope, tree: Tree) -> Self {
        Self { rope, tree }
    }

    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    pub fn tree(&self) -> &Tree {
        &self.tree
    }
}

#[derive(Debug, Clone)]
enum DocumentBucket {
    WorkspaceScript,
    PackageDocument(PathBuf),
    PackageScript(PathBuf),
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
        super::{TextPosition, TextRange, Workspace},
        indoc::indoc,
        std::path::PathBuf,
    };

    #[test]
    fn edits_documents_incrementally() {
        let mut workspace = Workspace::new().expect("workspace should initialize");
        let path = PathBuf::from("/workspace/file.R");

        workspace
            .insert_workspace_script(
                path.clone(),
                indoc! {"
                alpha <- 1
                beta <- 2
                gamma <- alpha + beta
            "},
            )
            .expect("document should insert");

        let unchanged_node_identifier = workspace
            .document(&path)
            .and_then(|document| document.tree().root_node().child(0))
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
            document.rope().to_string(),
            indoc! {"
                alpha <- 1
                beta <- 200
                gamma <- alpha + beta
            "}
        );

        let updated_node_identifier = document
            .tree()
            .root_node()
            .child(0)
            .map(|node| node.id())
            .expect("first node should exist after edit");
        assert_eq!(updated_node_identifier, unchanged_node_identifier);
    }

    #[test]
    fn renames_document_within_its_bucket() {
        let mut workspace = Workspace::new().expect("workspace should initialize");
        let package_path = PathBuf::from("/repo/package");
        let source_path = PathBuf::from("/repo/package/tests/file.R");
        let destination_path = PathBuf::from("/repo/package/tests/renamed.R");

        workspace
            .insert_package(package_path.clone())
            .expect("package should insert");
        workspace
            .insert_package_script(&package_path, source_path.clone(), "value <- 1\n")
            .expect("document should insert");

        workspace
            .rename_document(&source_path, destination_path.clone())
            .expect("rename should succeed");

        assert!(workspace.document(&source_path).is_none());
        assert!(workspace.document(&destination_path).is_some());
        let package = workspace
            .package(&package_path)
            .expect("package should still exist");
        assert!(package.scripts().any(|(path, _)| path == &destination_path));
    }

    #[test]
    fn deletes_documents() {
        let mut workspace = Workspace::new().expect("workspace should initialize");
        let path = PathBuf::from("/workspace/file.R");

        workspace
            .insert_workspace_script(path.clone(), "value <- 1\n")
            .expect("document should insert");
        workspace
            .delete_document(&path)
            .expect("delete should succeed");

        assert!(workspace.document(&path).is_none());
    }
}
