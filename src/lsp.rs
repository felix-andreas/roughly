use {
    crate::{cli, config::Config, diagnostics, format, index, tree},
    dashmap::DashMap,
    ropey::Rope,
    std::path::Path,
    tower_lsp::{
        Client, LanguageServer, LspService, Server,
        jsonrpc::{Error, Result},
        lsp_types::*,
    },
    tree_sitter::{InputEdit, Point, Tree},
};

pub async fn run() {
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
        config,
        client,
        symbols_map: DashMap::new(),
        document_map: DashMap::new(),
    });

    log::info!("starting language server ... listing for stdin");
    log::info!("for more info run 'roughly --help'");
    Server::new(stdin, stdout, socket).serve(service).await
}

#[derive(Debug)]
struct Backend {
    config: Config,
    client: Client,
    symbols_map: DashMap<Url, Vec<DocumentSymbol>>,
    document_map: DashMap<Url, Document>,
}

#[derive(Debug)]
struct Document {
    rope: Rope,
    tree: Tree,
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    //
    // LIFE CYLE METHODS
    //

    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        log::info!("server :: initialize");
        if let Err(()) = index::index_full(&self.symbols_map) {
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
                            include_text: Some(true),
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
        log::info!("initialized")
    }

    async fn shutdown(&self) -> Result<()> {
        log::debug!("bye bye");
        Ok(())
    }
    //
    // TEXT SYNC
    //

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        log::debug!("did open {}", params.text_document.uri.path());
        let rope = Rope::from_str(&params.text_document.text);
        let tree = tree::parse(&params.text_document.text, None);

        let diagnostics = diagnostics::diagnostics(
            tree.root_node(),
            &rope,
            diagnostics::Config::from_config(self.config),
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
        log::debug!("did change {}", params.text_document.uri.path());
        self.document_map
            .alter(&params.text_document.uri, |_, mut document| {
                for change in params.content_changes {
                    let Some(range) = change.range else {
                        log::warn!("unexpected case #2141 - check");
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

                    document.tree =
                        	// todo: use Parser::parse_with_options
                            tree::parse(&format!("{}", document.rope), Some(&document.tree));
                }

                // DEBUG
                // eprintln!("<--DOCUMENT-->\n{}<--END-->", document.rope.to_string());
                // eprintln!("{}", utils::format_node(document.tree.root_node()));
                // if let Ok(code) = format::format(document.tree.root_node(), &document.rope) {
                //     eprintln!("<--DOCUMENT-->\n{}<--END-->", code);
                // }
                document
            });

        if let Some(document) = self.document_map.get(&params.text_document.uri) {
            let diagnostics = diagnostics::diagnostics(
                document.tree.root_node(),
                &document.rope,
                diagnostics::Config::from_config(self.config),
            );

            self.client
                .publish_diagnostics(
                    params.text_document.uri.clone(),
                    diagnostics,
                    Some(params.text_document.version),
                )
                .await;
        };
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        self.document_map.remove(&params.text_document.uri);
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        log::debug!("did save {}", params.text_document.uri.path());

        index::index_update(&self.symbols_map, &params.text_document.uri);
        if let Some(document) = self.document_map.get(&params.text_document.uri) {
            let diagnostics = diagnostics::diagnostics(
                document.tree.root_node(),
                &document.rope,
                diagnostics::Config::from_config(self.config),
            );

            self.client
                .publish_diagnostics(params.text_document.uri.clone(), diagnostics, None)
                .await;
        };
    }

    //
    // COMPLETION
    //

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        log::debug!("Request completion items for: {uri}");
        let position = params.text_document_position.position;
        // todo: proper error handling. make ropey, dashmap -> JSONRpc error
        let completions = || -> Option<Vec<CompletionItem>> {
            let rope = &self.document_map.get(&uri)?.rope;
            let line = rope.get_line(position.line as usize)?;
            let mut query = String::new();
            for (i, char) in line.chars().enumerate() {
                if char.is_alphabetic() || char == '.' || (!query.is_empty() && char.is_numeric()) {
                    query.push(char)
                } else {
                    query.clear();
                }
                if i == (position.character - 1) as usize {
                    break;
                }
            }
            log::debug!("completion query: {query}");

            // TODO: consider passing Some(&uri) to avoid showing local symbols twice ...
            let workspace_symbols =
                index::get_workspace_symbols(&query, &self.symbols_map, 1000, None);

            // optimization would be to get all symbols for enclosing function
            // TODO: write code to get local completion items
            // let document_symbols = if let Some(document) = self.document_map.get(&uri) {
            //     let point = Point {
            //         row: position.line as usize,
            //         column: position.character as usize,
            //     };
            //     document
            //         .tree
            //         .root_node()
            //         .descendant_for_point_range(point, point)
            //         .and_then(|node| {
            //             let mut candidate = None;
            //             while let Some(node) = node.parent() {
            //                 if node.kind() == "function_definition" {
            //                     candidate = Some(node);
            //                 }
            //             }
            //             candidate.and_then(|function| function.child(2))
            //         })
            //         .map(|node| index::symbols_for_block(&node, &document.rope))
            //         .unwrap_or_default()
            // } else {
            //     log::error!("failed to aquirce document :/");
            //     vec![]
            // };

            const RESERVED_WORDS: &[&str] = &[
                "if",
                "else",
                "repeat",
                "while",
                "function",
                "for",
                "in",
                "next",
                "break",
                "TRUE",
                "FALSE",
                "NULL",
                "Inf",
                "NaN",
                "NA",
                "NA_integer_",
                "NA_real_",
                "NA_complex_",
                "NA_character_",
            ];

            Some(
                RESERVED_WORDS
                    .iter()
                    .map(|reserved_word| CompletionItem {
                        label: reserved_word.to_string(),
                        kind: Some(CompletionItemKind::KEYWORD),
                        ..Default::default()
                    })
                    // todo: do proper traversing
                    // .chain(document_symbols.iter().fold(vec![], |mut symbols, symbol| {
                    //     symbols.push(CompletionItem {
                    //         label: symbol.name.clone(),
                    //         kind: Some(match symbol.kind {
                    //             SymbolKind::FUNCTION => CompletionItemKind::FUNCTION,
                    //             SymbolKind::CLASS => CompletionItemKind::CLASS,
                    //             SymbolKind::METHOD => CompletionItemKind::METHOD,
                    //             _ => CompletionItemKind::VARIABLE,
                    //         }),
                    //         detail: None,
                    //         ..Default::default()
                    //     });
                    //     symbols
                    // }))
                    .chain(workspace_symbols.into_iter().map(|symbol| CompletionItem {
                        label: symbol.name,
                        kind: Some(match symbol.kind {
                            SymbolKind::FUNCTION => CompletionItemKind::FUNCTION,
                            SymbolKind::CLASS => CompletionItemKind::CLASS,
                            SymbolKind::METHOD => CompletionItemKind::METHOD,
                            _ => CompletionItemKind::VARIABLE,
                        }),
                        detail: None,
                        ..Default::default()
                    }))
                    .collect(),
            )
        }();

        Ok(completions.map(CompletionResponse::Array))
    }

    //
    // FORMATTING
    //

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let Some(document) = self.document_map.get(&params.text_document.uri) else {
            log::info!("formatting: failed to acquire symbols map");
            // todo: understand when this happens
            return Err(Error::internal_error());
        };
        let (rope, tree) = (&document.rope, &document.tree);
        let new = match format::format(tree.root_node(), rope, format::Config {
            spaces: self.config.spaces,
        }) {
            Ok(new) => new,
            Err(error) => {
                log::error!("formatting: {}", error);
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
        //         log::error!("failed to aquirce document :/");
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
            None,
        )))
    }
}
