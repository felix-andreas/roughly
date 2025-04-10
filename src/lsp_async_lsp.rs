#[cfg(feature = "async-lsp")]
use crate::lsp_types::Url as Uri;
#[cfg(feature = "tower-lsp")]
use uri_ext::UriExt;
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
            DocumentFormattingParams, DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse,
            InitializeParams, InitializeResult, OneOf, Position, PublishDiagnosticsParams, Range,
            SaveOptions, ServerCapabilities, ServerInfo, TextDocumentSyncCapability,
            TextDocumentSyncKind, TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TextEdit,
            WorkspaceSymbolParams, WorkspaceSymbolResponse,
        },
        tree,
    },
    async_lsp::{
        ClientSocket, ErrorCode, LanguageClient, LanguageServer, ResponseError,
        client_monitor::ClientProcessMonitorLayer,
        concurrency::ConcurrencyLayer,
        lsp_types::{MessageType, ShowMessageParams},
        panic::CatchUnwindLayer,
        router::Router,
        server::LifecycleLayer,
        tracing::TracingLayer,
    },
    dashmap::DashMap,
    futures::future::BoxFuture,
    ropey::Rope,
    std::{
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
    // TODO: propbably don't need dashmap here with async-lsp ...
    base_path: PathBuf,
    document_map: DashMap<PathBuf, Document>,
    document_symbols: DashMap<PathBuf, Vec<DocumentSymbol>>,
    workspace_symbols: DashMap<PathBuf, Vec<DocumentSymbol>>,
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
            workspace_symbols: DashMap::new(),
            document_symbols: DashMap::new(),
            document_map: DashMap::new(),
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

    fn did_change(
        &mut self,
        params: DidChangeTextDocumentParams,
    ) -> ControlFlow<async_lsp::Result<()>> {
        let uri = params.text_document.uri;
        let path = uri.to_file_path().unwrap();
        let content_changes = params.content_changes;

        tracing::debug!(?path, "did change");

        let start = Instant::now();

        // let random_duration = 200 + rand::random::<u64>() % 401;
        // tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        // let random_duration = 200 + rand::random::<u64>() % 401;
        // std::thread::sleep(std::time::Duration::from_millis(500));

        self.document_map.alter(&path, |_, mut document| {
            for change in content_changes {
                let Some(range) = change.range else {
                    tracing::warn!("unexpected case #2141 - check");
                    continue;
                };
                // DEBUG
                // eprintln!(
                //     "{}:{}, {}:{} - {}",
                //     range.start.line,
                //     range.start.character,
                //     range.end.line,
                //     range.end.character,
                //     change.text
                // );

                let (rope, tree) = (&mut document.rope, &mut document.tree);

                let start =
                    rope.line_to_char(range.start.line as usize) + range.start.character as usize;

                let end = rope.line_to_char(range.end.line as usize) + range.end.character as usize;

                let old_end_byte = rope.try_char_to_byte(end).unwrap();

                rope.remove(start..end);
                rope.insert(start, &change.text);

                let new_end_char = start + change.text.len();
                let new_end_byte = rope.try_char_to_byte(new_end_char).unwrap();

                let new_end_line = rope.char_to_line(start + change.text.len());

                tree.edit(&InputEdit {
                    start_byte: rope.try_char_to_byte(start).unwrap(),
                    old_end_byte,
                    new_end_byte,
                    start_position: Point {
                        row: range.start.line as usize,
                        column: range.start.character as usize,
                    },
                    old_end_position: Point {
                        row: range.end.line as usize,
                        column: range.end.character as usize,
                    },
                    new_end_position: Point {
                        row: new_end_line,
                        column: new_end_char - rope.line_to_char(new_end_line),
                    },
                });

                // todo: use Parser::parse_with_options
                // let mut parser = tree_sitter::Parser::new();
                // let language = tree_sitter_r::LANGUAGE;
                // parser
                //     .set_language(&language.into())
                //     .expect("Error loading R parser");

                // parser.parse_with_options(
                //     &mut |i, point| rope.byte_slice(i..).bytes(),
                //     Some(&document.tree),
                //     None,
                // );
                document.tree = tree::parse(document.rope.to_string(), Some(&document.tree));
            }

            // DEBUG
            // eprintln!("<--DOCUMENT-->\n{}<--END-->", document.rope);
            // eprintln!("{}", utils::format_node(document.tree.root_node()));
            // if let Ok(code) = format::format(document.tree.root_node(), &document.rope) {
            //     eprintln!("<--DOCUMENT-->\n{}<--END-->", code);
            // }
            document
        });

        if let Some(document) = self.document_map.get(&path) {
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
        } else {
            tracing::error!(?uri, "document not found");
        };

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

        let result = Ok(completions::get(
            position,
            &document.rope,
            &self.workspace_symbols,
        ));

        Box::pin(async move { result })
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
        let new = match format::format(tree.root_node(), rope, format::Config {
            indent: &" ".repeat(self.config.spaces),
            line_ending: LineEnding::Auto,
        }) {
            Ok(new) => new,
            Err(error) => {
                tracing::error!(?error, "failed to format");
                return Box::pin(async { Ok(None) });
            }
        };

        // TODO: only format if necessary and send text edits...
        let result = Ok(Some(vec![TextEdit {
            range: Range::new(
                Position::new(0, 0),
                Position::new(
                    (rope.len_lines() - 1) as u32,
                    (rope.len_chars() - rope.line_to_char(rope.len_lines() - 1)) as u32,
                ),
            ),
            new_text: new,
        }]));
        Box::pin(async move { result })
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

        let result = Ok(Some(DocumentSymbolResponse::Flat(
            index::get_document_symbols(&symbols, &uri),
        )));
        // Ok(Some(DocumentSymbolResponse::Nested({
        //     let Some(document) = self.document_map.get(&params.text_document.uri) else {
        //         tracing::error!("failed to aquirce document :/");
        //         return Ok(None);
        //     };
        //     index::get_document_symbols_ng(&document.tree, &document.rope)
        // })))
        Box::pin(async move { result })
    }

    fn symbol(
        &mut self,
        params: WorkspaceSymbolParams,
    ) -> BoxFuture<'static, Result<Option<WorkspaceSymbolResponse>, ResponseError>> {
        let query = params.query;

        let symbols = self
            .workspace_symbols
            .iter()
            .flat_map(|ref_multi| {
                let (path, symbols) = ref_multi.pair();
                let uri = Uri::from_file_path(path).unwrap();
                symbols
                    .iter()
                    .filter(|symbol| index::filter_symbol(&query, symbol))
                    .map(move |symbol| index::to_symbol_information(symbol, &uri))
                    .collect::<Vec<_>>()
            })
            .take(32) // limit amount
            .collect::<Vec<_>>();

        let result = Ok(Some(WorkspaceSymbolResponse::Flat(symbols)));
        Box::pin(async { result })
    }
}
