use {
    crate::{
        cli,
        config::{Config, ExperimentalFeatures},
        diagnostics, format,
        index::{self, IndexError, Item},
        lsp_types::{
            CompletionItem, CompletionItemKind, CompletionItemLabelDetails, CompletionOptions,
            CompletionParams, CompletionResponse, DidChangeTextDocumentParams,
            DidChangeWatchedFilesParams, DidChangeWatchedFilesRegistrationOptions,
            DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
            DocumentFormattingParams, DocumentRangeFormattingParams, DocumentSymbol,
            DocumentSymbolParams, DocumentSymbolResponse, FileChangeType, FileSystemWatcher,
            GlobPattern, Hover, HoverContents, HoverParams, HoverProviderCapability, InlayHint,
            InlayHintKind, InlayHintLabel, InlayHintParams,
            InitializeParams, InitializeResult, InitializedParams, Location, MarkupContent,
            MarkupKind, MessageType, OneOf, Position, PublishDiagnosticsParams, Range,
            ParameterInformation, ParameterLabel, ReferenceParams, Registration,
            RegistrationParams, RelativePattern, RenameParams, SaveOptions, ServerCapabilities,
            ServerInfo, ShowMessageParams, SignatureHelp, SignatureHelpOptions,
            SignatureHelpParams, SignatureInformation,
            TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
            TextDocumentSyncSaveOptions, TextEdit, Url, WorkspaceEdit, WorkspaceSymbolParams,
            WorkspaceSymbolResponse,
            notification::{DidChangeWatchedFiles, Notification},
        },
        symbols, utils,
    },
    analysis::{self, Analysis, DocumentChange, TextPosition, TextRange, ide},
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
    let (server, _) = async_lsp::MainLoop::new_server(|client| {
        let config = Config::from_path(Path::new(CONFIG_FILE_NAME), experimental_features)
            .unwrap_or_else(|error| {
                cli::error(&error.to_string());
                panic!("failed to load config: {error}");
            });

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
            analysis_state: Analysis::new(workspace_root.clone(), config.lint, config.check),
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

    // Goto-definition for S4 class/generic/method names, which are string literals invisible to the
    // identifier-based analysis. Resolves the name under the cursor to its `setClass`/`setGeneric`/
    // `setMethod` definitions via the workspace index.
    fn s4_definition(&mut self, path: &Path, position: Position) -> Option<GotoDefinitionResponse> {
        let point = Point::new(position.line as usize, position.character as usize);
        let reference = {
            let document = self.document(path)?;
            index::s4_reference_at(document.tree().root_node(), document.rope(), point)?
        };

        let mut locations = Vec::new();
        for (file_path, items) in self.package_items_map() {
            let uri = Url::from_file_path(&file_path).unwrap();
            for range in index::s4_definition_ranges(&reference, &items) {
                locations.push(Location {
                    uri: uri.clone(),
                    range,
                });
            }
        }

        match locations.len() {
            0 => None,
            1 => Some(GotoDefinitionResponse::Scalar(
                locations.pop().expect("single S4 definition location"),
            )),
            _ => Some(GotoDefinitionResponse::Array(locations)),
        }
    }

    fn package_items_map(&mut self) -> HashMap<PathBuf, Vec<Item>> {
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
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                inlay_hint_provider: Some(OneOf::Left(self.analysis_state.typing_enabled())),
                signature_help_provider: self.analysis_state.typing_enabled().then(|| {
                    SignatureHelpOptions {
                        trigger_characters: Some(vec!["(".to_owned(), ",".to_owned()]),
                        retrigger_characters: None,
                        work_done_progress_options: Default::default(),
                    }
                }),
                references_provider: Some(OneOf::Left(true)),
                rename_provider: Some(OneOf::Left(true)),
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
                return;
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

        let document_id = self
            .analysis_state
            .document_id_for_path(&path)
            .unwrap_or_else(|| {
                panic!(
                    "analysis document not found after did_open sync {}",
                    path.display()
                )
            });
        analysis::run_fast(&mut self.analysis_state);
        let diagnostics =
            diagnostics::convert_diagnostics(self.analysis_state.document_diagnostics(document_id));

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
                self.analysis_state
                    .add_document_from_disk(path.clone())
                    .unwrap_or_else(|error| {
                        panic!(
                            "failed to reload analysis document from disk on close {}: {error:?}",
                            path.display()
                        )
                    });
            } else {
                self.analysis_state
                    .delete_document(&path)
                    .unwrap_or_else(|error| {
                        panic!(
                            "failed to delete analysis document on close {}: {error:?}",
                            path.display()
                        )
                    });
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

        let changes = content_changes
            .into_iter()
            .map(|change| {
                let range = change.range.unwrap_or_else(|| {
                    panic!(
                        "incremental did_change for {} must include a range",
                        path.display()
                    )
                });
                DocumentChange {
                    range: TextRange {
                        start: TextPosition {
                            line_index: range.start.line as usize,
                            character_index: range.start.character as usize,
                        },
                        end: TextPosition {
                            line_index: range.end.line as usize,
                            character_index: range.end.character as usize,
                        },
                    },
                    text: change.text,
                }
            })
            .collect::<Vec<_>>();
        self.analysis_state
            .edit_document(&path, &changes)
            .unwrap_or_else(|error| {
                panic!(
                    "failed to edit analysis document {} incrementally: {error:?}",
                    path.display()
                )
            });

        let document_id = self
            .analysis_state
            .document_id_for_path(&path)
            .unwrap_or_else(|| {
                panic!(
                    "analysis document not found after did_change sync {}",
                    path.display()
                )
            });
        analysis::run_fast(&mut self.analysis_state);
        let diagnostics =
            diagnostics::convert_diagnostics(self.analysis_state.document_diagnostics(document_id));

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

        let document_id = self
            .analysis_state
            .document_id_for_path(&path)
            .unwrap_or_else(|| {
                panic!(
                    "analysis document not found after did_save sync {}",
                    path.display()
                )
            });
        // Package-visible changes can move diagnostics in dependent files, so every
        // document whose typecheck output changed gets republished, not only the saved one.
        let mut affected_document_ids = analysis::run_full(&mut self.analysis_state);
        if !affected_document_ids.contains(&document_id) {
            affected_document_ids.push(document_id);
        }

        for affected_document_id in affected_document_ids {
            let Some(affected_path) = self
                .analysis_state
                .path_for_document_id(affected_document_id)
                .map(Path::to_path_buf)
            else {
                continue;
            };
            let affected_uri = if affected_path == path {
                uri.clone()
            } else {
                match Url::from_file_path(&affected_path) {
                    Ok(affected_uri) => affected_uri,
                    Err(()) => continue,
                }
            };
            let diagnostics = diagnostics::convert_diagnostics(
                self.analysis_state
                    .document_diagnostics(affected_document_id),
            );
            if let Err(error) = self
                .client
                .publish_diagnostics(PublishDiagnosticsParams::new(affected_uri, diagnostics, None))
            {
                tracing::error!(?error, "failed to publish diagnostics");
            }
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
                        self.analysis_state.set_configs(config.lint, config.check);
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
                            self.analysis_state.delete_document(&path).expect(&format!(
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
        let path = params
            .text_document_position
            .text_document
            .uri
            .to_file_path()
            .unwrap();
        let position = params.text_document_position.position;

        tracing::debug!(?path, "completion");

        if self.opened_document(&path).is_none() {
            tracing::error!(?path, "document not found");
            return box_future(Err(path_not_found_error(&path)));
        }

        let completions = ide::completion(
            &mut self.analysis_state,
            &path,
            TextPosition {
                line_index: position.line as usize,
                character_index: position.character as usize,
            },
        )
        .map(|items| {
            CompletionResponse::Array(
                items
                    .into_iter()
                    .map(|item| CompletionItem {
                        label: item.label,
                        label_details: Some(CompletionItemLabelDetails {
                            detail: None,
                            description: Some(match item.source {
                                analysis::CompletionItemSource::Keyword => "Keyword".into(),
                                analysis::CompletionItemSource::Local => "Local".into(),
                                analysis::CompletionItemSource::Global => "Global".into(),
                            }),
                        }),
                        kind: Some(match item.kind {
                            analysis::CompletionItemKind::Keyword => CompletionItemKind::KEYWORD,
                            analysis::CompletionItemKind::Variable => CompletionItemKind::VARIABLE,
                            analysis::CompletionItemKind::Function => CompletionItemKind::FUNCTION,
                        }),
                        ..Default::default()
                    })
                    .collect(),
            )
        });

        box_future(Ok(completions))
    }

    //
    // DEFINITION
    //

    fn definition(
        &mut self,
        params: GotoDefinitionParams,
    ) -> BoxFuture<'static, Result<Option<GotoDefinitionResponse>, ResponseError>> {
        let uri = params.text_document_position_params.text_document.uri;
        let path = uri.to_file_path().unwrap();
        let position = params.text_document_position_params.position;

        tracing::debug!(?path, ?position, "goto definition");

        if self.opened_document(&path).is_none() {
            tracing::info!(?path, "document not found");
            return box_future(Err(path_not_found_error(&path)));
        }

        let response = ide::definition(
            &mut self.analysis_state,
            &path,
            TextPosition {
                line_index: position.line as usize,
                character_index: position.character as usize,
            },
        )
        .map(|locations| {
            let mut locations = locations
                .into_iter()
                .map(convert_location)
                .collect::<Vec<_>>();
            match locations.len() {
                1 => GotoDefinitionResponse::Scalar(
                    locations.pop().expect("single definition location"),
                ),
                _ => GotoDefinitionResponse::Array(locations),
            }
        })
        // S4 names are string literals, so identifier-based resolution finds nothing; fall back to
        // the S4 index.
        .or_else(|| self.s4_definition(&path, position));

        box_future(Ok(response))
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

        let value = ide::render_hover_markdown(&hover_info, self.experimental_features.debug);

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
    // INLAY HINTS
    //

    fn inlay_hint(
        &mut self,
        params: InlayHintParams,
    ) -> BoxFuture<'static, Result<Option<Vec<InlayHint>>, ResponseError>> {
        let uri = params.text_document.uri;
        let path = uri.to_file_path().unwrap();

        tracing::debug!(?path, "inlay hints");

        if self.opened_document(&path).is_none() {
            tracing::info!(?path, "document not found");
            return box_future(Err(path_not_found_error(&path)));
        }

        let hints = ide::inlay_hints(&mut self.analysis_state, &path)
            .into_iter()
            .map(|hint| InlayHint {
                position: Position::new(
                    hint.position.line_index as u32,
                    hint.position.character_index as u32,
                ),
                label: InlayHintLabel::String(hint.label),
                kind: Some(InlayHintKind::TYPE),
                text_edits: None,
                tooltip: None,
                padding_left: Some(false),
                padding_right: Some(false),
                data: None,
            })
            .collect();

        box_future(Ok(Some(hints)))
    }

    //
    // SIGNATURE HELP
    //

    fn signature_help(
        &mut self,
        params: SignatureHelpParams,
    ) -> BoxFuture<'static, Result<Option<SignatureHelp>, ResponseError>> {
        let uri = params.text_document_position_params.text_document.uri;
        let path = uri.to_file_path().unwrap();
        let position = params.text_document_position_params.position;

        tracing::debug!(?path, ?position, "signature help");

        if self.opened_document(&path).is_none() {
            tracing::info!(?path, "document not found");
            return box_future(Err(path_not_found_error(&path)));
        }

        let Some(help) = ide::signature_help(
            &mut self.analysis_state,
            &path,
            TextPosition {
                line_index: position.line as usize,
                character_index: position.character as usize,
            },
        ) else {
            return box_future(Ok(None));
        };

        let active_parameter = help.active_parameter.map(|index| index as u32);
        let parameters = help
            .parameters
            .into_iter()
            .map(|label| ParameterInformation {
                label: ParameterLabel::Simple(label),
                documentation: None,
            })
            .collect();

        let signature_help = SignatureHelp {
            signatures: vec![SignatureInformation {
                label: help.label,
                documentation: None,
                parameters: Some(parameters),
                active_parameter,
            }],
            active_signature: Some(0),
            active_parameter,
        };

        box_future(Ok(Some(signature_help)))
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
        let uri = params.text_document_position.text_document.uri;
        let path = uri.to_file_path().unwrap();
        let position = params.text_document_position.position;
        let include_declaration = params.context.include_declaration;

        tracing::debug!(?path, ?position, ?include_declaration, "find references");

        if self.opened_document(&path).is_none() {
            tracing::info!(?path, "document not found");
            return box_future(Err(path_not_found_error(&path)));
        }

        let references = ide::references(
            &mut self.analysis_state,
            &path,
            TextPosition {
                line_index: position.line as usize,
                character_index: position.character as usize,
            },
            include_declaration,
        )
        .map(|locations| locations.into_iter().map(convert_location).collect());

        box_future(Ok(references))
    }

    //
    // RENAME
    //

    fn rename(
        &mut self,
        params: RenameParams,
    ) -> BoxFuture<'static, Result<Option<WorkspaceEdit>, ResponseError>> {
        let uri = params.text_document_position.text_document.uri;
        let path = uri.to_file_path().unwrap();
        let position = params.text_document_position.position;
        let new_name = params.new_name;

        tracing::debug!(?path, ?position, ?new_name, "rename");

        if self.opened_document(&path).is_none() {
            tracing::info!(?path, "document not found");
            return box_future(Err(path_not_found_error(&path)));
        }

        let workspace_edit = ide::rename(
            &mut self.analysis_state,
            &path,
            TextPosition {
                line_index: position.line as usize,
                character_index: position.character as usize,
            },
            &new_name,
        )
        .map(|rename_result| {
            let changes = rename_result
                .edits
                .into_iter()
                .map(|(path, edits)| {
                    let uri =
                        Url::from_file_path(path).expect("rename edit path should convert to URI");
                    let edits = edits
                        .into_iter()
                        .map(|edit| TextEdit {
                            range: Range::new(
                                Position::new(
                                    edit.range.start.line_index as u32,
                                    edit.range.start.character_index as u32,
                                ),
                                Position::new(
                                    edit.range.end.line_index as u32,
                                    edit.range.end.character_index as u32,
                                ),
                            ),
                            new_text: edit.replacement_text,
                        })
                        .collect();
                    (uri, edits)
                })
                .collect();

            WorkspaceEdit {
                changes: Some(changes),
                ..Default::default()
            }
        });

        box_future(Ok(workspace_edit))
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
        // Document symbols are requested on every keystroke and only need top-level symbols, so
        // they intentionally read the parsed tree directly instead of running the analysis
        // pipeline.
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

fn convert_location(location: analysis::ide::Location) -> Location {
    Location {
        uri: Url::from_file_path(&location.path).expect("location path should convert to URI"),
        range: Range::new(
            Position::new(
                location.range.start.line_index as u32,
                location.range.start.character_index as u32,
            ),
            Position::new(
                location.range.end.line_index as u32,
                location.range.end.character_index as u32,
            ),
        ),
    }
}
