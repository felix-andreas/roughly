// TODO: this file is a mess, clean it up

use {
    crate::{
        cli, completions,
        config::Config,
        diagnostics,
        format::{self, LineEnding},
        index::{self, IndexError},
        tree,
    },
    dashmap::DashMap,
    ropey::Rope,
    std::path::Path,
    tower_lsp_server::{
        Client, LanguageServer, LspService, Server,
        jsonrpc::{Error, Result},
        lsp_types::*,
    },
    tree_sitter::{InputEdit, Point, Tree},
};
#[cfg(feature = "async-lsp")]
pub use {async_lsp::lsp_types, lsp_async_lsp::*};

#[cfg(feature = "tower-lsp")]
mod lsp_tower_lsp;

// TODO: test if this fixes sync issues
// #[tokio::main(flavor = "current_thread")]
#[tokio::main]
pub async fn run(experimental: bool) {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let config = match Config::from_path(Path::new(".")) {
        Ok(config) => config,
        Err(err) => {
            cli::error(&err.to_string());
            return;
        }
    };

    let (service, socket) = LspService::new(|client| Backend {
        client,
        config,
        experimental,
        symbols_map: DashMap::new(),
        document_map: DashMap::new(),
    });

    tracing::info!("starting language server ... listing for stdin");
    tracing::info!("for more info run 'roughly --help'");
    Server::new(stdin, stdout, socket).serve(service).await
}

#[derive(Debug)]
struct Backend {
    client: Client,
    config: Config,
    experimental: bool,
    symbols_map: DashMap<Uri, Vec<DocumentSymbol>>,
    document_map: DashMap<Uri, Document>,
}

#[derive(Debug)]
pub struct Document {
    pub rope: Rope,
    pub tree: Tree,
}

impl LanguageServer for Backend {
    //
    // LIFE CYLE METHODS
    //

    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        tracing::info!("server :: initialize");
        if let Err(IndexError) = index::index_full(&self.symbols_map) {
            self.client
                .show_message(MessageType::ERROR, "failed to index files")
                .await;
        }

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
    }

    async fn initialized(&self, _: InitializedParams) {
        tracing::info!("initialized")
    }

    async fn shutdown(&self) -> Result<()> {
        tracing::debug!("bye bye");
        Ok(())
    }

    //
    // TEXT SYNC
    //

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        tracing::debug!("did open {}", params.text_document.uri.path());
        let rope = Rope::from_str(&params.text_document.text);
        let tree = tree::parse(&params.text_document.text, None);

        let diagnostics = diagnostics::analyze_full(
            tree.root_node(),
            &rope,
            diagnostics::Config::from_config(self.config, self.experimental),
        );

        self.document_map
            .insert(params.text_document.uri.clone(), Document { rope, tree });

        self.client
            .publish_diagnostics(
                params.text_document.uri.clone(),
                diagnostics,
                Some(params.text_document.version),
            )
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        tracing::debug!("did change {}", params.text_document.uri.path());
        let start = std::time::Instant::now();

        // let random_duration = 200 + rand::random::<u64>() % 401;
        // tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        // let random_duration = 200 + rand::random::<u64>() % 401;
        // std::thread::sleep(std::time::Duration::from_millis(500));

        self.document_map
            .alter(&params.text_document.uri, |_, mut document| {
                for change in params.content_changes {
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

                    let start = rope.line_to_char(range.start.line as usize)
                        + range.start.character as usize;

                    let end =
                        rope.line_to_char(range.end.line as usize) + range.end.character as usize;

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

        if let Some(document) = self.document_map.get(&params.text_document.uri) {
            let diagnostics = diagnostics::analyze_fast(
                document.tree.root_node(),
                &document.rope,
                diagnostics::Config::from_config(self.config, self.experimental),
            );

            self.client
                .publish_diagnostics(
                    params.text_document.uri.clone(),
                    diagnostics,
                    Some(params.text_document.version),
                )
                .await;
        } else {
            tracing::info!("did change: failed to acquire symbols map");
        };

        let elapsed = start.elapsed();
        tracing::debug!("did change in {} ms", elapsed.as_millis());
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        self.document_map.remove(&params.text_document.uri);
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        tracing::debug!("did save {}", params.text_document.uri.path());

        if let Some(document) = self.document_map.get(&params.text_document.uri) {
            index::index_update(
                &self.symbols_map,
                &params.text_document.uri,
                &document.rope.to_string(),
            );

            let diagnostics = diagnostics::analyze_full(
                document.tree.root_node(),
                &document.rope,
                diagnostics::Config::from_config(self.config, self.experimental),
            );

            self.client
                .publish_diagnostics(params.text_document.uri.clone(), diagnostics, None)
                .await;
        } else {
            tracing::info!("did change: failed to acquire symbols map");
        };
    }

    //
    // COMPLETION
    //

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        tracing::debug!("Request completion items for: {uri:?}");
        let position = params.text_document_position.position;
        Ok(completions::get(
            uri,
            position,
            &self.document_map,
            &self.symbols_map,
        ))
    }

    //
    // FORMATTING
    //

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        tracing::debug!("format file {}", params.text_document.uri.path());

        let Some(document) = self.document_map.get(&params.text_document.uri) else {
            tracing::info!("formatting: failed to acquire symbols map");
            // todo: understand when this happens
            return Err(Error::internal_error());
        };
        let (rope, tree) = (&document.rope, &document.tree);
        let new = match format::format(tree.root_node(), rope, format::Config {
            indent: &" ".repeat(self.config.spaces),
            line_ending: LineEnding::Auto,
        }) {
            Ok(new) => new,
            Err(error) => {
                tracing::error!("formatting: {}", error);
                return Ok(None);
            }
        };

        // TODO: only format if necessary and send text edits...
        Ok(Some(vec![TextEdit {
            range: Range::new(
                Position::new(0, 0),
                Position::new(
                    (rope.len_lines() - 1) as u32,
                    (rope.len_chars() - rope.line_to_char(rope.len_lines() - 1)) as u32,
                ),
            ),
            new_text: new,
        }]))
    }

    //
    // SYMBOLS
    //

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        Ok(Some(DocumentSymbolResponse::Flat(
            index::get_document_symbols(&params.text_document.uri, &self.symbols_map),
        )))
        // Ok(Some(DocumentSymbolResponse::Nested({
        //     let Some(document) = self.document_map.get(&params.text_document.uri) else {
        //         tracing::error!("failed to aquirce document :/");
        //         return Ok(None);
        //     };
        //     index::get_document_symbols_ng(&document.tree, &document.rope)
        // })))
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        Ok(Some(index::get_workspace_symbols(
            &params.query,
            &self.symbols_map,
            32,
        )))
    }
}
