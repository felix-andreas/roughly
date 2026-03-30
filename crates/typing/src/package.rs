use {
    crate::document::Document,
    std::path::{Path, PathBuf},
    tree_sitter::Point,
};

#[derive(Clone, Default)]
pub struct Package {
    documents: std::collections::HashMap<PathBuf, Document>,
    /// Scripts attached to a package. They can resolve against the package namespace, but they do
    /// not contribute back to that namespace.
    scripts: std::collections::HashMap<PathBuf, Document>,
}

impl Package {
    pub fn document(&self, path: &Path) -> Option<&Document> {
        self.documents.get(path).or_else(|| self.scripts.get(path))
    }

    pub fn ordered_documents_and_scripts(&self) -> Vec<(PathBuf, &Document)> {
        let mut package_documents = self
            .documents()
            .map(|(path, document)| (path.clone(), document))
            .collect::<Vec<_>>();
        package_documents.sort_by(|(left_path, _), (right_path, _)| left_path.cmp(right_path));

        let mut package_scripts = self
            .scripts()
            .map(|(path, document)| (path.clone(), document))
            .collect::<Vec<_>>();
        package_scripts.sort_by(|(left_path, _), (right_path, _)| left_path.cmp(right_path));

        package_documents.extend(package_scripts);
        package_documents
    }

    pub fn first_ordered_document_or_script(&self) -> Option<(PathBuf, &Document)> {
        self.ordered_documents_and_scripts().into_iter().next()
    }

    pub fn fallback_range(&self) -> tree_sitter::Range {
        if let Some((_, document)) = self.first_ordered_document_or_script() {
            let line = document
                .rope()
                .get_line(0)
                .map(|line| line.to_string())
                .unwrap_or_default();
            return tree_sitter::Range {
                start_byte: 0,
                end_byte: line.len(),
                start_point: Point { row: 0, column: 0 },
                end_point: Point {
                    row: 0,
                    column: line.len(),
                },
            };
        }

        tree_sitter::Range {
            start_byte: 0,
            end_byte: 0,
            start_point: Point { row: 0, column: 0 },
            end_point: Point { row: 0, column: 0 },
        }
    }

    pub fn documents(&self) -> impl Iterator<Item = (&PathBuf, &Document)> {
        self.documents.iter()
    }

    pub fn scripts(&self) -> impl Iterator<Item = (&PathBuf, &Document)> {
        self.scripts.iter()
    }

    pub(crate) fn insert_document(&mut self, path: PathBuf, document: Document) {
        self.documents.insert(path, document);
    }

    pub(crate) fn insert_script(&mut self, path: PathBuf, document: Document) {
        self.scripts.insert(path, document);
    }

    pub(crate) fn remove_document(&mut self, path: &Path) -> Option<Document> {
        self.documents.remove(path)
    }

    pub(crate) fn remove_script(&mut self, path: &Path) -> Option<Document> {
        self.scripts.remove(path)
    }
}
