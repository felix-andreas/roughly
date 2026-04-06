use {
    crate::{
        cli, completion,
        config::{Config, ExperimentalFeatures},
        diagnostics,
        format, hover,
        index::{self, IndexError, Item},
        lsp_types::{
            CompletionOptions, CompletionParams, CompletionResponse, DidChangeTextDocumentParams,
            DidChangeWatchedFilesParams, DidChangeWatchedFilesRegistrationOptions,
            DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
            DocumentFormattingParams, DocumentRangeFormattingParams, DocumentSymbol,
            DocumentSymbolParams, DocumentSymbolResponse, FileChangeType, FileSystemWatcher,
            GlobPattern, Hover, HoverContents, HoverParams, HoverProviderCapability,
            InitializeParams, InitializeResult, InitializedParams, Location, MarkupContent,
            MarkupKind, MessageType, OneOf, Position, PublishDiagnosticsParams, Range,
            ReferenceParams, Registration, RegistrationParams, RelativePattern, RenameParams,
            SaveOptions, ServerCapabilities, ServerInfo, ShowMessageParams,
            TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
            TextDocumentSyncSaveOptions, TextEdit, Url, WorkspaceEdit, WorkspaceSymbolParams,
            WorkspaceSymbolResponse,
            notification::{DidChangeWatchedFiles, Notification},
        },
        symbols, utils,
    },
    analysis::{Analysis, TextPosition, TextRange, ide},
    async_lsp::{
        ClientSocket, ErrorCode, LanguageClient, LanguageServer, ResponseError,
        client_monitor::ClientProcessMonitorLayer,
        concurrency::ConcurrencyLayer,
        lsp_types::{DidChangeConfigurationParams, GotoDefinitionParams, GotoDefinitionResponse},
        panic::CatchUnwindLayer,
        router::Router,
        server::LifecycleLayer,
        tracing::TracingLayer,
    },
    futures::future::BoxFuture,
    std::{
        collections::{HashMap, HashSet},
        ops::ControlFlow,
        path::{Path, PathBuf},
        time::Instant,
    },
    tower::ServiceBuilder,
    tree_sitter::Point,
};

const CONFIG_FILE_NAME: &str = "roughly.toml";

// #[tokio::main] # TODO: understand if this makes a difference???
#[tokio::main(flavor = "current_thread")]
pub async fn run(experimental_features: ExperimentalFeatures) {
    let (server, _) = async_lsp::MainLoop::new_server(|mut client| {
        let config = match Config::from_path(Path::new(CONFIG_FILE_NAME), experimental_features) {
            Ok(config) => config,
            Err(err) => {
                let _ = client.show_message(ShowMessageParams {
                    typ: MessageType::ERROR,
                    message: format!("failed to load config: {err}"),
                });
                cli::error(&err.to_string());
                panic!("fixme");
            }
        };

        ServiceBuilder::new()
            .layer(TracingLayer::default())
            .layer(LifecycleLayer::default())
            .layer(CatchUnwindLayer::default())
            .layer(ConcurrencyLayer::default())
            .layer(ClientProcessMonitorLayer::new(client.clone()))
            .service(ServerState::new_router(
                client,
                config,
                experimental_features,
            ))
    });

    // Prefer truly asynchronous piped stdin/stdout without blocking tasks.
    #[cfg(unix)]
    let (stdin, stdout) = (
        async_lsp::stdio::PipeStdin::lock_tokio().unwrap(),
        async_lsp::stdio::PipeStdout::lock_tokio().unwrap(),
    );
    // Fallback to spawn blocking read/write otherwise.
    #[cfg(not(unix))]
    let (stdin, stdout) = (
        tokio_util::compat::TokioAsyncReadCompatExt::compat(tokio::io::stdin()),
        tokio_util::compat::TokioAsyncWriteCompatExt::compat_write(tokio::io::stdout()),
    );

    tracing::info!("starting server using async-lsp ...");
    server.run_buffered(stdin, stdout).await.unwrap();
}

struct ServerState {
    client: ClientSocket,
    config: Config,
    experimental_features: ExperimentalFeatures,
    workspace_root: PathBuf,
    open_documents: HashSet<PathBuf>,
    analysis_state: Analysis,
}

impl ServerState {
    fn new_router(
        client: ClientSocket,
        config: Config,
        experimental_features: ExperimentalFeatures,
    ) -> Router<Self> {
        let workspace_root = std::env::current_dir().unwrap();

        Router::from_language_server(Self {
            client,
            config,
            experimental_features,
            workspace_root: workspace_root.clone(),
            open_documents: HashSet::new(),
            analysis_state: Analysis::new(workspace_root.clone()),
        })
    }

    fn workspace_r_path(&self) -> PathBuf {
        self.workspace_root.join("R")
    }

    fn document(&self, path: &Path) -> Option<&analysis::Document> {
        self.analysis_state.document(path)
    }

    fn opened_document(&self, path: &Path) -> Option<&analysis::Document> {
        self.open_documents
            .contains(path)
            .then(|| self.document(path))
            .flatten()
    }

    fn package_items_map(&mut self) -> HashMap<PathBuf, Vec<Item>> {
        self.sync_dirty_documents();

        self.analysis_state
            .package_document_ids()
            .into_iter()
            .filter_map(|document_id| {
                let path = self
                    .analysis_state
                    .path_for_document_id(document_id)?
                    .to_path_buf();
                let document = self.analysis_state.document_by_id(document_id)?;
                Some((
                    path,
                    index::index(document.tree().root_node(), document.rope(), false, false),
                ))
            })
            .collect()
    }

    fn sync_dirty_documents(&mut self) {
        let sync_errors = self.analysis_state.sync_dirty_documents();
        assert!(
            sync_errors.is_empty(),
            "failed to synchronize analysis documents from disk: {sync_errors:?}"
        );
    }

    fn continue_with_error(&mut self, message: String) -> ControlFlow<async_lsp::Result<()>> {
        self.client
            .show_message(ShowMessageParams {
                typ: MessageType::ERROR,
                message,
            })
            .unwrap();
        ControlFlow::Continue(())
    }
}

impl LanguageServer for ServerState {
    type Error = ResponseError;
    type NotifyResult = ControlFlow<async_lsp::Result<()>>;

    fn initialize(
        &mut self,
        _: InitializeParams,
    ) -> BoxFuture<'static, Result<InitializeResult, ResponseError>> {
        tracing::info!(?self.experimental_features, "initialize");

        let workspace_r_path = self.workspace_r_path();

        if workspace_r_path.is_dir() {
            match index::source_file_paths(&workspace_r_path) {
                Ok(paths) => {
                    for path in paths {
                        self.analysis_state
                            .add_document_from_disk(path.clone())
                            .expect(&format!(
                                "failed to preload analysis document {}",
                                path.display()
                            ));
                    }
                }
                Err(IndexError) => {
                    panic!(
                        "failed to list package source files in {}",
                        workspace_r_path.display()
                    );
                }
            }
        }

        box_future(Ok(InitializeResult {
            capabilities: ServerCapabilities {
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec!["$".into(), "@".into(), ":".into()]),
                    ..Default::default()
                }),
                definition_provider: Some(OneOf::Left(true)),
                document_formatting_provider: Some(OneOf::Left(true)),
                document_range_formatting_provider: Some(OneOf::Left(
                    self.experimental_features.range_formatting,
                )),
                document_symbol_provider: Some(OneOf::Left(true)),
                hover_provider: self
                    .experimental_features
                    .hovering
                    .then_some(HoverProviderCapability::Simple(true)),
                references_provider: Some(OneOf::Left(self.experimental_features.goto_references)),
                rename_provider: Some(OneOf::Left(self.experimental_features.rename)),
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::INCREMENTAL),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(false),
                        })),
                        ..Default::default()
                    },
                )),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: env!("CARGO_PKG_NAME").into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
        }))
    }

    fn initialized(&mut self, _: InitializedParams) -> ControlFlow<async_lsp::Result<()>> {
        // TODO: consider to negotiate client capabilities
        // see: https://github.com/oxalica/nil/blob/870a4b1b5f/crates/nil/src/capabilities.rs
        let workspace_r_path = self.workspace_r_path();

        let params = RegistrationParams {
            registrations: vec![Registration {
                id: DidChangeWatchedFiles::METHOD.into(),
                method: DidChangeWatchedFiles::METHOD.into(),
                register_options: Some(
                    serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
                        watchers: vec![
                            FileSystemWatcher {
                                glob_pattern: GlobPattern::Relative(RelativePattern {
                                    base_uri: OneOf::Right(
                                        Url::from_file_path(&workspace_r_path).unwrap(),
                                    ),
                                    pattern: "*.[rR]".into(),
                                }),
                                kind: None,
                            },
                            FileSystemWatcher {
                                glob_pattern: GlobPattern::Relative(RelativePattern {
                                    base_uri: OneOf::Right(
                                        Url::from_file_path(&self.workspace_root).unwrap(),
                                    ),
                                    pattern: CONFIG_FILE_NAME.into(),
                                }),
                                kind: None,
                            },
                        ],
                    })
                    .unwrap(),
                ),
            }],
        };

        let mut client = self.client.clone();
        tokio::spawn(async move {
            if let Err(err) = client.register_capability(params).await {
                client
                    .show_message(ShowMessageParams {
                        typ: MessageType::ERROR,
                        message: format!("failed to watch R files: {err:#}"),
                    })
                    .unwrap();
            }
            tracing::info!("registered file watching for R files");
        });

        ControlFlow::Continue(())
    }

    //
    // TEXT SYNC
    //

    fn did_open(
        &mut self,
        params: DidOpenTextDocumentParams,
    ) -> ControlFlow<async_lsp::Result<()>> {
        let uri = params.text_document.uri;
        let path = uri.to_file_path().unwrap();
        let text = &params.text_document.text;

        tracing::debug!(?path, "did open");

        self.analysis_state
            .add_document_from_source(path.clone(), text)
            .expect(&format!(
                "failed to sync analysis document from source {}",
                path.display()
            ));
        self.open_documents.insert(path.clone());

        let diagnostics = {
            diagnostics::saved_document_diagnostics(
                &mut self.analysis_state,
                &path,
                self.config.lint,
                false,
            )
        };

        if let Err(error) = self
            .client
            .publish_diagnostics(PublishDiagnosticsParams::new(
                uri,
                diagnostics,
                Some(params.text_document.version),
            ))
        {
            tracing::error!(?error, "failed to publish diagnostics");
        }

        ControlFlow::Continue(())
    }

    fn did_close(
        &mut self,
        params: DidCloseTextDocumentParams,
    ) -> ControlFlow<async_lsp::Result<()>> {
        let uri = params.text_document.uri;
        let path = uri.to_file_path().unwrap();

        tracing::debug!(?path, "did close");

        self.open_documents.remove(&path);
        if path.starts_with(self.workspace_r_path()) {
            if path.exists() {
                self.analysis_state.mark_document_dirty(&path);
            } else {
                self.analysis_state.mark_document_deleted(&path);
            }
        }

        ControlFlow::Continue(())
    }

    fn did_change(
        &mut self,
        params: DidChangeTextDocumentParams,
    ) -> ControlFlow<async_lsp::Result<()>> {
        let uri = params.text_document.uri;
        let path = uri.to_file_path().unwrap();
        let content_changes = params.content_changes;

        tracing::debug!(?path, "did change");

        let start = Instant::now();

        if !self.open_documents.contains(&path) {
            return self.continue_with_error(format!(
                "received did_change for non-open document {}",
                path.display()
            ));
        }

        self.document(&path).expect(&format!(
            "analysis document not found for {}",
            path.display()
        ));

        for change in &content_changes {
            if let Some(range) = change.range.as_ref() {
                let edit_range = TextRange {
                    start: TextPosition {
                        line_index: range.start.line as usize,
                        character_index: range.start.character as usize,
                    },
                    end: TextPosition {
                        line_index: range.end.line as usize,
                        character_index: range.end.character as usize,
                    },
                };
                if let Err(error) = self
                    .analysis_state
                    .edit_document(&path, |document, parser| {
                        document.edit_range(parser, edit_range, &change.text)
                    })
                {
                    panic!(
                        "failed to edit analysis document {} incrementally: {error:?}",
                        path.display()
                    );
                }
                continue;
            }

            assert_eq!(
                content_changes.len(),
                1,
                "full-document did_change for {} should contain exactly one content change",
                path.display()
            );
            self.analysis_state
                .add_document_from_source(path.clone(), &change.text)
                .expect(&format!(
                    "failed to replace analysis document from source {}",
                    path.display()
                ));
            break;
        }

        let diagnostics = diagnostics::current_document_diagnostics(
            &mut self.analysis_state,
            &path,
            self.config.lint,
        );

        if let Err(error) = self
            .client
            .publish_diagnostics(PublishDiagnosticsParams::new(
                uri.clone(),
                diagnostics,
                Some(params.text_document.version),
            ))
        {
            tracing::error!(?error, "failed to publish diagnostics");
        }

        tracing::debug!(elapsed = start.elapsed().as_millis());

        ControlFlow::Continue(())
    }

    fn did_save(
        &mut self,
        params: DidSaveTextDocumentParams,
    ) -> ControlFlow<async_lsp::Result<()>> {
        let uri = params.text_document.uri;
        let path = uri.to_file_path().unwrap();

        tracing::debug!(?path, "did save");

        if !self.open_documents.contains(&path) {
            return self.continue_with_error(format!(
                "received did_save for non-open document {}",
                path.display()
            ));
        }

        let mut diagnostics = {
            diagnostics::saved_document_diagnostics(
                &mut self.analysis_state,
                &path,
                self.config.lint,
                false,
            )
        };

        if self.config.lint.experimental_typing && path.starts_with(self.workspace_r_path()) {
            self.sync_dirty_documents();
            diagnostics = diagnostics::saved_document_diagnostics(
                &mut self.analysis_state,
                &path,
                self.config.lint,
                true,
            );
        }

        if diagnostics.is_empty() {
            tracing::debug!(?uri, "save produced no diagnostics");
        }

        if let Err(error) = self
            .client
            .publish_diagnostics(PublishDiagnosticsParams::new(
                uri.clone(),
                diagnostics,
                None,
            ))
        {
            tracing::error!(?error, "failed to publish diagnostics");
        }

        ControlFlow::Continue(())
    }

    fn did_change_watched_files(
        &mut self,
        params: DidChangeWatchedFilesParams,
    ) -> ControlFlow<async_lsp::Result<()>> {
        let config_path = self.workspace_root.join(CONFIG_FILE_NAME);
        let workspace_r_path = self.workspace_r_path();

        for change in params.changes {
            let uri = change.uri;
            let typ = change.typ;
            let path = uri.to_file_path().unwrap();

            tracing::info!(?path, ?typ, "watched file changed");

            if path == config_path {
                match Config::from_path(&config_path, self.experimental_features) {
                    Ok(config) => {
                        self.config = config;
                    }
                    Err(error) => {
                        self.client
                            .show_message(ShowMessageParams {
                                typ: MessageType::ERROR,
                                message: format!("failed to reload config: {error}"),
                            })
                            .unwrap();
                    }
                }
                continue;
            }

            if path.starts_with(&workspace_r_path) {
                match change.typ {
                    FileChangeType::CREATED | FileChangeType::CHANGED => {
                        if !self.open_documents.contains(&path) {
                            self.analysis_state
                                .add_document_from_disk(path.clone())
                                .expect(&format!(
                                    "failed to update analysis document from disk {}",
                                    path.display()
                                ));
                        }
                    }
                    FileChangeType::DELETED => {
                        if !self.open_documents.contains(&path) {
                            self.analysis_state
                                .delete_document(&path)
                                .or_else(|error| match error {
                                    analysis::AnalysisError::DocumentNotFound(_) => Ok(()),
                                    error => Err(error),
                                })
                                .expect(&format!(
                                    "failed to delete analysis document {}",
                                    path.display()
                                ));
                        }
                    }
                    _ => unreachable!(),
                }
            }
        }

        ControlFlow::Continue(())
    }

    fn did_change_configuration(
        &mut self,
        _params: DidChangeConfigurationParams,
    ) -> ControlFlow<async_lsp::Result<()>> {
        // Stub implementation to satisfy Zed's requirements; does not apply any configuration changes.
        ControlFlow::Continue(())
    }

    //
    // COMPLETION
    //

    fn completion(
        &mut self,
        params: CompletionParams,
    ) -> BoxFuture<'static, Result<Option<CompletionResponse>, ResponseError>> {
        let uri = params.text_document_position.text_document.uri;
        let path = uri.to_file_path().unwrap();
        let position = params.text_document_position.position;

        tracing::debug!(?path, "completion");

        let Some(document) = self.opened_document(&path).cloned() else {
            tracing::error!(?path, "document not found");
            return box_future(Err(path_not_found_error(&path)));
        };
        let workspace_items = self.package_items_map();

        let completions =
            completion::get(position, document.rope(), document.tree(), &workspace_items);

        box_future(Ok(completions))
    }

    //
    // DEFINITION
    //

    fn definition(
        &mut self,
        params: GotoDefinitionParams,
    ) -> BoxFuture<'static, Result<Option<GotoDefinitionResponse>, ResponseError>> {
        /*
        let _ = params;
        let uri = params.text_document_position_params.text_document.uri;
        let path = uri.to_file_path().unwrap();
        let position = params.text_document_position_params.position;

        tracing::debug!(?path, "goto definition");

        if self.opened_document(&path).is_none() {
            tracing::info!(?path, "document not found");
            return box_future(Err(path_not_found_error(&path)));
        }
        let workspace_items = self.package_items_map();

        let definitions = definition::goto(
            &uri,
            position.line as usize,
            position.character as usize,
            document.rope(),
            document.tree(),
            &workspace_items,
        );

        return box_future(Ok(definitions));
        */
        let _ = params;
        box_future(Err(unsupported_feature_error("goto definition")))
    }

    //
    // HOVER
    //

    fn hover(
        &mut self,
        params: HoverParams,
    ) -> BoxFuture<'static, Result<Option<Hover>, ResponseError>> {
        let uri = params.text_document_position_params.text_document.uri;
        let path = uri.to_file_path().unwrap();
        let position = params.text_document_position_params.position;

        tracing::debug!(?path, ?position, "hover");

        if self.opened_document(&path).is_none() {
            tracing::info!(?path, "document not found");
            return box_future(Err(path_not_found_error(&path)));
        }

        self.sync_dirty_documents();

        let Some(hover_info) = ide::hover(
            &mut self.analysis_state,
            &path,
            TextPosition {
                line_index: position.line as usize,
                character_index: position.character as usize,
            },
        ) else {
            tracing::debug!(?position, "hover target not found");
            return box_future(Ok(None));
        };

        let value = hover::markdown(&hover_info, self.experimental_features);

        let hover = Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: Some(Range::new(
                Position::new(
                    hover_info.range.start.line_index as u32,
                    hover_info.range.start.character_index as u32,
                ),
                Position::new(
                    hover_info.range.end.line_index as u32,
                    hover_info.range.end.character_index as u32,
                ),
            )),
        };

        box_future(Ok(Some(hover)))
    }

    //
    // FORMATTING
    //

    fn formatting(
        &mut self,
        params: DocumentFormattingParams,
    ) -> BoxFuture<'static, Result<Option<Vec<TextEdit>>, ResponseError>> {
        let uri = params.text_document.uri;
        let path = uri.to_file_path().unwrap();

        tracing::debug!(?path, "format");

        let Some(document) = self.opened_document(&path) else {
            tracing::info!(?path, "document not found");
            return box_future(Err(path_not_found_error(&path)));
        };

        let new_text = match format::format(
            document.tree().root_node(),
            document.rope(),
            self.config.format,
        ) {
            Ok(text) => text,
            Err(error) => {
                tracing::error!(?error, "failed to format");
                return box_future(Ok(None));
            }
        };

        let edits = vec![TextEdit {
            new_text,
            range: Range::new(
                Position::new(0, 0),
                Position::new(
                    (document.rope().len_lines() - 1) as u32,
                    (document.rope().len_chars()
                        - document
                            .rope()
                            .line_to_char(document.rope().len_lines() - 1))
                        as u32,
                ),
            ),
        }];

        box_future(Ok(Some(edits)))
    }

    fn range_formatting(
        &mut self,
        params: DocumentRangeFormattingParams,
    ) -> BoxFuture<'static, Result<Option<Vec<TextEdit>>, ResponseError>> {
        let uri = params.text_document.uri;
        let range = params.range;
        let path = uri.to_file_path().unwrap();

        tracing::debug!(?path, "format");

        let Some(document) = self.opened_document(&path) else {
            tracing::info!(?path, "document not found");
            return box_future(Err(path_not_found_error(&path)));
        };

        let Some(node) = document.tree().root_node().descendant_for_point_range(
            Point::new(range.start.line as usize, range.start.character as usize),
            Point::new(range.end.line as usize, range.end.character as usize),
        ) else {
            tracing::info!(?range, "no node for range");
            return box_future(Ok(None));
        };

        let new_text = match format::format(node, document.rope(), self.config.format) {
            Ok(text) => text,
            Err(error) => {
                tracing::error!(?error, "failed to format");
                return box_future(Ok(None));
            }
        };

        let edits = vec![TextEdit {
            new_text,
            range: utils::node_range(node),
        }];

        box_future(Ok(Some(edits)))
    }

    //
    // REFERENCES
    //

    fn references(
        &mut self,
        params: ReferenceParams,
    ) -> BoxFuture<'static, Result<Option<Vec<Location>>, ResponseError>> {
        /*
        let _ = params;
        let uri = params.text_document_position.text_document.uri;
        let path = uri.to_file_path().unwrap();
        let position = params.text_document_position.position;
        let include_declaration = params.context.include_declaration;

        tracing::debug!(?path, ?position, ?include_declaration, "find references");

        let Some(document) = self.opened_document(&path) else {
            tracing::info!(?path, "document not found");
            return box_future(Err(path_not_found_error(&path)));
        };
        let workspace_items = self.package_items_map();

        let references = references::find_references(
            &uri,
            position.line as usize,
            position.character as usize,
            include_declaration,
            document.rope(),
            document.tree(),
            &workspace_items,
        );

        return box_future(Ok(references));
        */
        let _ = params;
        box_future(Err(unsupported_feature_error("references")))
    }

    //
    // RENAME
    //

    fn rename(
        &mut self,
        params: RenameParams,
    ) -> BoxFuture<'static, Result<Option<WorkspaceEdit>, ResponseError>> {
        /*
        let _ = params;
        let uri = params.text_document_position.text_document.uri;
        let path = uri.to_file_path().unwrap();
        let position = params.text_document_position.position;
        let new_name = params.new_name;

        tracing::debug!(?path, ?position, ?new_name, "rename");

        let Some(document) = self.opened_document(&path) else {
            tracing::info!(?path, "document not found");
            return box_future(Err(path_not_found_error(&path)));
        };

        let workspace_edit = rename::rename(
            &uri,
            position.line as usize,
            position.character as usize,
            &new_name,
            document.rope(),
            document.tree(),
        );

        return box_future(Ok(workspace_edit));
        */
        let _ = params;
        box_future(Err(unsupported_feature_error("rename")))
    }

    //
    // SYMBOLS
    //

    fn document_symbol(
        &mut self,
        params: DocumentSymbolParams,
    ) -> BoxFuture<'static, Result<Option<DocumentSymbolResponse>, ResponseError>> {
        let uri = params.text_document.uri;
        let path = uri.to_file_path().unwrap();

        let Some(document) = self.document(&path) else {
            tracing::error!(?path, "symbols not found");
            return box_future(Err(path_not_found_error(&path)));
        };
        let items = index::index(document.tree().root_node(), document.rope(), false, false);
        let symbols: Vec<DocumentSymbol> = symbols::document(&items);

        box_future(Ok(Some(DocumentSymbolResponse::Nested(symbols))))
    }

    fn symbol(
        &mut self,
        params: WorkspaceSymbolParams,
    ) -> BoxFuture<'static, Result<Option<WorkspaceSymbolResponse>, ResponseError>> {
        let query = params.query;

        tracing::debug!(?query);

        let workspace_items = self.package_items_map();
        let symbols = symbols::workspace(&query, &workspace_items);

        box_future(Ok(Some(WorkspaceSymbolResponse::Nested(symbols))))
    }
}

#[inline(always)]
fn box_future<T: Send + 'static>(content: T) -> BoxFuture<'static, T> {
    Box::pin(async { content })
}

fn path_not_found_error(path: &Path) -> ResponseError {
    ResponseError::new(
        ErrorCode::REQUEST_FAILED,
        format!("path not found '{}'", path.display()),
    )
}

fn unsupported_feature_error(feature_name: &str) -> ResponseError {
    ResponseError::new(
        ErrorCode::REQUEST_FAILED,
        format!("{feature_name} is temporarily disabled during analysis integration"),
    )
}
