// `Workspace` is a possible later editor-facing abstraction.
// For now, `Package` remains the unit of analysis and should own package contents directly.
use {
    crate::{
        document::{Document, DocumentEditError},
        package::Package,
        text::TextRange,
        tree::new_parser,
    },
    std::{
        collections::HashMap,
        path::{Path, PathBuf},
    },
    thiserror::Error,
    tree_sitter::Parser,
};

pub struct Workspace {
    parser: Parser,
    packages: HashMap<PathBuf, Package>,
    /// Scripts not attached to any package.
    scripts: HashMap<PathBuf, Document>,
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
        let parser = new_parser().map_err(|_| WorkspaceError::ParserInitializationFailed)?;

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
        if let Err(error) = document.edit_range(&mut self.parser, range, replacement_text) {
            self.insert_document_at(bucket, path.to_path_buf(), document)?;
            return Err(match error {
                DocumentEditError::InvalidRange => WorkspaceError::InvalidEditRange {
                    path: path.to_path_buf(),
                },
                DocumentEditError::ParseFailed => WorkspaceError::ParseFailed(path.to_path_buf()),
            });
        }

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
                package.insert_document(path, document);
                Ok(())
            }
            DocumentBucket::PackageScript(package_path) => {
                let package = self
                    .packages
                    .get_mut(&package_path)
                    .ok_or(WorkspaceError::PackageNotFound(package_path))?;
                package.insert_script(path, document);
                Ok(())
            }
        }
    }

    fn take_document(&mut self, path: &Path) -> Option<(DocumentBucket, Document)> {
        if let Some(document) = self.scripts.remove(path) {
            return Some((DocumentBucket::WorkspaceScript, document));
        }

        for (package_path, package) in &mut self.packages {
            if let Some(document) = package.remove_document(path) {
                return Some((
                    DocumentBucket::PackageDocument(package_path.clone()),
                    document,
                ));
            }
            if let Some(document) = package.remove_script(path) {
                return Some((
                    DocumentBucket::PackageScript(package_path.clone()),
                    document,
                ));
            }
        }

        None
    }

    fn parse_document(&mut self, source: &str, path: &Path) -> Result<Document, WorkspaceError> {
        Document::parse(&mut self.parser, source)
            .ok_or_else(|| WorkspaceError::ParseFailed(path.to_path_buf()))
    }
}

#[derive(Debug, Clone)]
enum DocumentBucket {
    WorkspaceScript,
    PackageDocument(PathBuf),
    PackageScript(PathBuf),
}

#[cfg(test)]
mod tests {
    use {
        super::Workspace,
        crate::text::{TextPosition, TextRange},
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
