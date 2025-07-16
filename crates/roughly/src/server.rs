use {
    crate::{
        cli, completion,
        config::{Config, ExperimentalFeatures},
        definition, diagnostics, format,
        index::{self, IndexError, Item},
        lsp_types::{
            CompletionOptions, CompletionParams, CompletionResponse, DidChangeTextDocumentParams,
            DidChangeWatchedFilesParams, DidChangeWatchedFilesRegistrationOptions,
            DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
            DocumentFormattingParams, DocumentRangeFormattingParams, DocumentSymbol,
            DocumentSymbolParams, DocumentSymbolResponse, FileChangeType, FileSystemWatcher,
            GlobPattern, InitializeParams, InitializeResult, InitializedParams, Location,
            MessageType, OneOf, Position, PublishDiagnosticsParams, Range, ReferenceParams,
            Registration, RegistrationParams, RelativePattern, RenameParams, SaveOptions,
            ServerCapabilities, ServerInfo, ShowMessageParams, TextDocumentSyncCapability,
            TextDocumentSyncKind, TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TextEdit,
            Url, WorkspaceEdit, WorkspaceSymbolParams, WorkspaceSymbolResponse,
            notification::{DidChangeWatchedFiles, Notification},
        },
        references, rename, tree, utils,
    },
    async_lsp::{
        ClientSocket, ErrorCode, LanguageClient, LanguageServer, ResponseError,
        client_monitor::ClientProcessMonitorLayer,
        concurrency::ConcurrencyLayer,
        lsp_types::{GotoDefinitionParams, GotoDefinitionResponse},
        panic::CatchUnwindLayer,
        router::Router,
        server::LifecycleLayer,
        tracing::TracingLayer,
    },
    futures::future::BoxFuture,
    ropey::Rope,
    std::{
        collections::HashMap,
        ops::ControlFlow,
        path::{Path, PathBuf},
        time::Instant,
    },
    tower::ServiceBuilder,
    tree_sitter::{InputEdit, Parser, Point, Tree},
};

// #[tokio::main] # TODO: understand if this makes a difference???
#[tokio::main(flavor = "current_thread")]
pub async fn run(experimental_features: ExperimentalFeatures) {
    let (server, _) = async_lsp::MainLoop::new_server(|client| {
        let config = match Config::from_path(Path::new("."), experimental_features) {
            Ok(config) => config,
            Err(err) => {
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
    base_path: PathBuf,
    document_map: HashMap<PathBuf, Document>,
    /// stores symbolds for all other files
    document_symbols: HashMap<PathBuf, Vec<Item>>,
    /// stores index for all files in R/ folder
    workspace_symbols: HashMap<PathBuf, Vec<Item>>,
    parser: Parser,
}

#[derive(Debug)]
pub struct Document {
    pub rope: Rope,
    pub tree: Tree,
}

impl ServerState {
    fn new_router(
        client: ClientSocket,
        config: Config,
        experimental_features: ExperimentalFeatures,
    ) -> Router<Self> {
        Router::from_language_server(Self {
            client,
            config,
            experimental_features,
            base_path: std::env::current_dir().unwrap().join("R"),
            workspace_symbols: HashMap::new(),
            document_symbols: HashMap::new(),
            document_map: HashMap::new(),
            parser: tree::new_parser(),
        })
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

        match index::index_dir(&self.base_path, &mut self.parser) {
            Ok(symbols) => self.workspace_symbols.extend(symbols),
            Err(IndexError) => self
                .client
                .show_message(ShowMessageParams {
                    typ: MessageType::ERROR,
                    message: "failed to index files".into(),
                })
                .unwrap(),
        }

        box_future(Ok(InitializeResult {
            capabilities: ServerCapabilities {
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec!["$".into(), "@".into(), ":".into()]),
                    ..Default::default()
                }),
                definition_provider: Some(OneOf::Left(self.experimental_features.goto_definition)),
                document_formatting_provider: Some(OneOf::Left(true)),
                document_range_formatting_provider: Some(OneOf::Left(
                    self.experimental_features.range_formatting,
                )),
                document_symbol_provider: Some(OneOf::Left(true)),
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
        let params = RegistrationParams {
            registrations: vec![Registration {
                id: DidChangeWatchedFiles::METHOD.into(),
                method: DidChangeWatchedFiles::METHOD.into(),
                register_options: Some(
                    serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
                        watchers: vec![FileSystemWatcher {
                            glob_pattern: GlobPattern::Relative(RelativePattern {
                                base_uri: OneOf::Right(
                                    Url::from_file_path(&self.base_path).unwrap(),
                                ),
                                pattern: "*.[rR]".into(),
                            }),
                            kind: None,
                        }],
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

        let rope = Rope::from_str(text);
        let tree = tree::parse(&mut self.parser, text, None);

        let diagnostics = diagnostics::analyze_full(tree.root_node(), &rope, self.config.lint);

        let symbols = index::index(tree.root_node(), &rope, false, false);
        if path.starts_with(&self.base_path) {
            // note: we need to insert into workspace in case a new file is created
            self.workspace_symbols.insert(path.clone(), symbols);
        } else {
            self.document_symbols.insert(path.clone(), symbols);
        }

        self.document_map.insert(path, Document { rope, tree });

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

        self.document_map.remove(&path);

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

        let Some(document) = self.document_map.get_mut(&path) else {
            tracing::error!(?path, "document not found");
            return ControlFlow::Continue(());
        };

        // UPDATE ROPE AND TREE
        // based on: https://github.com/marceline-cramer/saturn-v/blob/93d1c8fd02/lsp/src/lib.rs
        let (rope, tree) = (&mut document.rope, &mut document.tree);
        for change in content_changes {
            let range = change.range.unwrap();

            let start_line = range.start.line as usize;
            let start_col = range.start.character as usize;
            let end_line = range.end.line as usize;
            let end_col = range.end.character as usize;

            let start_char = rope.line_to_char(start_line) + start_col;
            let end_char = rope.line_to_char(end_line) + end_col;

            let start_byte = rope.char_to_byte(start_char);
            let old_end_byte = rope.char_to_byte(end_char);
            let new_end_byte = start_byte + change.text.len();

            rope.remove(start_char..end_char);
            rope.insert(start_char, &change.text);

            let new_end_line = rope.byte_to_line(new_end_byte);
            let new_end_col = rope.byte_to_char(new_end_byte) - rope.line_to_char(new_end_line);

            tree.edit(&InputEdit {
                start_byte,
                old_end_byte,
                new_end_byte,
                start_position: Point::new(start_line, start_col),
                old_end_position: Point::new(end_line, end_col),
                new_end_position: Point::new(new_end_line, new_end_col),
            });
        }

        *tree = tree::parse_rope(&mut self.parser, rope, Some(tree));

        // UPDATE DIAGNOSTICS
        let diagnostics = diagnostics::analyze_fast(tree.root_node(), rope, self.config.lint);

        // UPDATE SYMBOLS
        // note: We must re-index on every change (not just on save)
        // because textDocument/documentSymbol is triggered before textDocument/didSave.
        let symbols = index::index(tree.root_node(), rope, false, false);
        if path.starts_with(&self.base_path) {
            self.workspace_symbols.insert(path, symbols);
        } else {
            self.document_symbols.insert(path, symbols);
        }

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

        let Some(document) = self.document_map.get_mut(&path) else {
            tracing::error!(?path, "document not found");
            return ControlFlow::Continue(());
        };

        let (rope, root_node) = (&document.rope, document.tree.root_node());

        let diagnostics = diagnostics::analyze_full(root_node, rope, self.config.lint);

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
        for change in params.changes {
            let uri = change.uri;
            let typ = change.typ;
            let path = uri.to_file_path().unwrap();

            tracing::info!(?path, ?typ, "watched file changed");

            if path.starts_with(&self.base_path) {
                match change.typ {
                    FileChangeType::CREATED | FileChangeType::CHANGED => {
                        // note: potential race condition if the user already has the file open and begins editing immediately.
                        let symbols = index::index_file(&path, &mut self.parser);
                        self.workspace_symbols.insert(path.clone(), symbols);
                    }
                    FileChangeType::DELETED => {
                        self.workspace_symbols.remove(&path);
                    }
                    _ => unreachable!(),
                }
            }
        }

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

        let Some(document) = self.document_map.get(&path) else {
            tracing::error!(?path, "document not found");
            return box_future(Err(path_not_found_error(&path)));
        };

        let completions = completion::get(
            position,
            &document.rope,
            &document.tree,
            &self.workspace_symbols,
        );

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

        tracing::debug!(?path, "goto definition");

        let Some(document) = self.document_map.get(&path) else {
            tracing::info!(?path, "document not found");
            return box_future(Err(path_not_found_error(&path)));
        };

        let definitions = definition::goto(
            &uri,
            position.line as usize,
            position.character as usize,
            &document.rope,
            &document.tree,
            &self.workspace_symbols,
        );

        box_future(Ok(definitions))
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

        let Some(document) = self.document_map.get(&path) else {
            tracing::info!(?path, "document not found");
            return box_future(Err(path_not_found_error(&path)));
        };

        let (rope, tree) = (&document.rope, &document.tree);
        let new_text = match format::format(tree.root_node(), rope, self.config.format) {
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
                    (rope.len_lines() - 1) as u32,
                    (rope.len_chars() - rope.line_to_char(rope.len_lines() - 1)) as u32,
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

        let Some(document) = self.document_map.get(&path) else {
            tracing::info!(?path, "document not found");
            return box_future(Err(path_not_found_error(&path)));
        };

        let (rope, tree) = (&document.rope, &document.tree);
        let Some(node) = tree.root_node().descendant_for_point_range(
            Point::new(range.start.line as usize, range.start.character as usize),
            Point::new(range.end.line as usize, range.end.character as usize),
        ) else {
            tracing::info!(?range, "no node for range");
            return box_future(Ok(None));
        };

        let new_text = match format::format(node, rope, self.config.format) {
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

        let Some(document) = self.document_map.get(&path) else {
            tracing::info!(?path, "document not found");
            return box_future(Err(path_not_found_error(&path)));
        };

        let references = references::find_references(
            &uri,
            position.line as usize,
            position.character as usize,
            include_declaration,
            &document.rope,
            &document.tree,
            &self.workspace_symbols,
        );

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

        let Some(document) = self.document_map.get(&path) else {
            tracing::info!(?path, "document not found");
            return box_future(Err(path_not_found_error(&path)));
        };

        let workspace_edit = rename::rename(
            &uri,
            position.line as usize,
            position.character as usize,
            &new_name,
            &document.rope,
            &document.tree,
        );

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

        let symbols_map = if path.starts_with(&self.base_path) {
            &self.workspace_symbols
        } else {
            &self.document_symbols
        };

        let Some(symbols) = symbols_map.get(&path) else {
            tracing::error!(?path, "symbols not found");
            return box_future(Err(path_not_found_error(&path)));
        };
        let symbols: Vec<DocumentSymbol> = symbols.iter().map(|item| item.to_document_symbol()).collect();

        box_future(Ok(Some(DocumentSymbolResponse::Nested(symbols))))
    }

    fn symbol(
        &mut self,
        params: WorkspaceSymbolParams,
    ) -> BoxFuture<'static, Result<Option<WorkspaceSymbolResponse>, ResponseError>> {
        let query = params.query;

        tracing::debug!(?query);

        let symbols = index::get_workspace_symbols(&query, &self.workspace_symbols);

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
