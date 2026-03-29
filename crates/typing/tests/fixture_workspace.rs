use {
    fixtures::{Fixture, FixtureInputFile, FixtureKind},
    std::path::PathBuf,
    workspace::{DocumentKind, DocumentView, Workspace},
};

pub fn simple_fixture_source(fixture: &Fixture) -> Result<&str, String> {
    if let FixtureKind::Simple(case) = &fixture.kind {
        Ok(&case.input)
    } else {
        Err("unsupported fixture".to_owned())
    }
}

pub fn with_fixture_document<T>(
    source: &str,
    run: impl FnOnce(DocumentView<'_>) -> T,
) -> Result<T, String> {
    let input_files = [FixtureInputFile {
        path: PathBuf::from("/fixture.R"),
        contents: source.to_owned(),
    }];

    with_fixture_documents(&input_files, |workspace| {
        let document = workspace
            .document(PathBuf::from("/fixture.R"))
            .ok_or_else(|| "workspace".to_owned())?;
        Ok(run(document))
    })
}

pub fn with_fixture_documents<T>(
    input_files: &[FixtureInputFile],
    run: impl FnOnce(&Workspace) -> Result<T, String>,
) -> Result<T, String> {
    let mut workspace = Workspace::new().map_err(|_| "workspace".to_owned())?;

    for input_file in input_files {
        workspace
            .replace_document(
                input_file.path.clone(),
                &input_file.contents,
                DocumentKind::Standalone,
            )
            .map_err(|_| "workspace".to_owned())?;
    }

    run(&workspace)
}
