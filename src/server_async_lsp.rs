use {
    crate::{
        cli, completions,
        config::Config,
        diagnostics,
        format::{self, LineEnding},
        index::{self, IndexError},
        lsp_types::{
            CompletionOptions, CompletionParams, CompletionResponse, DidChangeTextDocumentParams,
            DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
            DocumentFormattingParams, DocumentRangeFormattingParams, DocumentSymbol,
            DocumentSymbolParams, DocumentSymbolResponse, InitializeParams, InitializeResult,
            MessageType, OneOf, Position, PublishDiagnosticsParams, Range, SaveOptions,
            ServerCapabilities, ServerInfo, ShowMessageParams, TextDocumentSyncCapability,
            TextDocumentSyncKind, TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TextEdit,
            WorkspaceSymbolParams, WorkspaceSymbolResponse,
        },
        tree,
    },
    async_lsp::{
        ClientSocket, ErrorCode, LanguageClient, LanguageServer, ResponseError,
        client_monitor::ClientProcessMonitorLayer, concurrency::ConcurrencyLayer,
        panic::CatchUnwindLayer, router::Router, server::LifecycleLayer, tracing::TracingLayer,
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
    tree_sitter::{InputEdit, Point, Tree},
};

// #[tokio::main] # TODO: understand if this makes a difference???
#[tokio::main(flavor = "current_thread")]
pub async fn run(experimental: bool) {
    let (server, _) = async_lsp::MainLoop::new_server(|client| {
        let config = match Config::from_path(Path::new(".")) {
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
            .service(ServerState::new_router(client, config, experimental))
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
    experimental: bool,
    base_path: PathBuf,
    document_map: HashMap<PathBuf, Document>,
    document_symbols: HashMap<PathBuf, Vec<DocumentSymbol>>,
    workspace_symbols: HashMap<PathBuf, Vec<DocumentSymbol>>,
}

#[derive(Debug)]
pub struct Document {
    pub rope: Rope,
    pub tree: Tree,
}

impl ServerState {
    fn new_router(client: ClientSocket, config: Config, experimental: bool) -> Router<Self> {
        Router::from_language_server(Self {
            client,
            config,
            experimental,
            base_path: std::env::current_dir().unwrap().join("R"),
            workspace_symbols: HashMap::new(),
            document_symbols: HashMap::new(),
            document_map: HashMap::new(),
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
        tracing::info!("initialize");

        match index::index_full(&self.base_path) {
            Ok(symbols) => self.workspace_symbols.extend(symbols),
            Err(IndexError) => self
                .client
                .show_message(ShowMessageParams {
                    typ: MessageType::ERROR,
                    message: "failed to index files".into(),
                })
                .unwrap(),
        }

        Box::pin(async move {
            Ok(InitializeResult {
                capabilities: ServerCapabilities {
                    completion_provider: Some(CompletionOptions {
                        trigger_characters: Some(vec!["$".into(), "@".into()]),
                        ..Default::default()
                    }),
                    document_range_formatting_provider: Some(OneOf::Left(true)),
                    document_formatting_provider: Some(OneOf::Left(true)),
                    document_symbol_provider: Some(OneOf::Left(true)),
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
            })
        })
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

        tracing::debug!(?uri, "did open");

        let rope = Rope::from_str(text);
        let tree = tree::parse(text, None);

        let diagnostics = diagnostics::analyze_full(
            tree.root_node(),
            &rope,
            diagnostics::Config::from_config(self.config, self.experimental),
        );

        if !path.starts_with(&self.base_path) {
            let symbols = index::index(text);
            self.document_symbols.insert(path.clone(), symbols);
        };

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

    // inspired by:
    // https://github.com/marceline-cramer/saturn-v/blob/93d1c8fd022f5b4905928d6e9154385c5b6822ab/lsp/src/lib.rs
    fn did_change(
        &mut self,
        params: DidChangeTextDocumentParams,
    ) -> ControlFlow<async_lsp::Result<()>> {
        // DEBUG
        // let random_duration = 200 + rand::random::<u64>() % 401;
        // tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let uri = params.text_document.uri;
        let path = uri.to_file_path().unwrap();
        let content_changes = params.content_changes;

        tracing::debug!(?path, "did change");

        let start = Instant::now();

        let Some(document) = self.document_map.get_mut(&path) else {
            tracing::error!(?uri, "document not found");
            return ControlFlow::Continue(());
        };

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
                start_position: Point {
                    row: start_line,
                    column: start_col,
                },
                old_end_position: Point {
                    row: end_line,
                    column: end_col,
                },
                new_end_position: Point {
                    row: new_end_line,
                    column: new_end_col,
                },
            });
        }

        document.tree = tree::parse_rope(rope, tree);

        // DEBUG
        // eprintln!("<--DOCUMENT-->\n{}<--END-->", rope);

        let diagnostics = diagnostics::analyze_fast(
            document.tree.root_node(),
            &document.rope,
            diagnostics::Config::from_config(self.config, self.experimental),
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

    fn did_close(
        &mut self,
        params: DidCloseTextDocumentParams,
    ) -> ControlFlow<async_lsp::Result<()>> {
        let uri = params.text_document.uri;
        let path = uri.to_file_path().unwrap();

        self.document_map.remove(&path);

        ControlFlow::Continue(())
    }

    fn did_save(
        &mut self,
        params: DidSaveTextDocumentParams,
    ) -> ControlFlow<async_lsp::Result<()>> {
        let uri = params.text_document.uri;
        let path = uri.to_file_path().unwrap();

        tracing::debug!(?uri, "did save");

        if let Some(document) = self.document_map.get(&path) {
            let symbols = index::index(&document.rope.to_string());
            if path.starts_with(&self.base_path) {
                self.workspace_symbols.insert(path, symbols);
            } else {
                self.document_symbols.insert(path, symbols);
            }

            let diagnostics = diagnostics::analyze_full(
                document.tree.root_node(),
                &document.rope,
                diagnostics::Config::from_config(self.config, self.experimental),
            );

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
        } else {
            tracing::error!("document not found");
        };

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

        tracing::debug!(?uri, "completion");

        let Some(document) = self.document_map.get(&path) else {
            tracing::error!(?uri, "document not found");
            return Box::pin(async move { Err(ResponseError::new(ErrorCode::INTERNAL_ERROR, "")) });
        };

        let completions = completions::get(
            position,
            &document.rope,
            &document.tree,
            &self.workspace_symbols,
        );

        Box::pin(async move { Ok(completions) })
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

        tracing::debug!(?uri, "format");

        let Some(document) = self.document_map.get(&path) else {
            tracing::info!(?uri, "document not found");
            return Box::pin(async move { Err(ResponseError::new(ErrorCode::INTERNAL_ERROR, "")) });
        };

        let (rope, tree) = (&document.rope, &document.tree);
        let new_text = match format::format(tree.root_node(), rope, format::Config {
            indent: &" ".repeat(self.config.spaces),
            line_ending: LineEnding::Auto,
        }) {
            Ok(text) => text,
            Err(error) => {
                tracing::error!(?error, "failed to format");
                return Box::pin(async { Ok(None) });
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

        Box::pin(async move { Ok(Some(edits)) })
    }

    fn range_formatting(
        &mut self,
        params: DocumentRangeFormattingParams,
    ) -> BoxFuture<'static, Result<Option<Vec<TextEdit>>, ResponseError>> {
        let uri = params.text_document.uri;
        let range = params.range;
        let path = uri.to_file_path().unwrap();

        tracing::debug!(?uri, "format");

        let Some(document) = self.document_map.get(&path) else {
            tracing::info!(?uri, "document not found");
            return Box::pin(async move { Err(ResponseError::new(ErrorCode::INTERNAL_ERROR, "")) });
        };

        let (rope, tree) = (&document.rope, &document.tree);
        let Some(node) = tree.root_node().descendant_for_point_range(
            Point::new(range.start.line as usize, range.start.character as usize),
            Point::new(range.end.line as usize, range.end.character as usize),
        ) else {
            tracing::info!(?range, "no node for range");
            return Box::pin(async { Ok(None) });
        };

        let new_text = match format::format(node, rope, format::Config {
            indent: &" ".repeat(self.config.spaces),
            line_ending: LineEnding::Auto,
        }) {
            Ok(text) => text,
            Err(error) => {
                tracing::error!(?error, "failed to format");
                return Box::pin(async { Ok(None) });
            }
        };

        let start = node.start_position();
        let end = node.end_position();

        let edits = vec![TextEdit {
            new_text,
            range: Range::new(
                Position::new(start.row as u32, start.column as u32),
                Position::new(end.row as u32, end.column as u32),
            ),
        }];

        Box::pin(async move { Ok(Some(edits)) })
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
            tracing::error!(?uri, "symbols not found");
            return Box::pin(async move { Err(ResponseError::new(ErrorCode::INTERNAL_ERROR, "")) });
        };
        let symbols = symbols.clone();

        Box::pin(async move { Ok(Some(DocumentSymbolResponse::Nested(symbols))) })
    }

    fn symbol(
        &mut self,
        params: WorkspaceSymbolParams,
    ) -> BoxFuture<'static, Result<Option<WorkspaceSymbolResponse>, ResponseError>> {
        let query = params.query;

        let symbols = index::get_workspace_symbols(&query, &self.workspace_symbols);

        Box::pin(async { Ok(Some(WorkspaceSymbolResponse::Nested(symbols))) })
    }
}
