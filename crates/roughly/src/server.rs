use {
    crate::{
        cli,
        config::{Config, ExperimentalFeatures},
        diagnostics, format,
        index::{self, IndexError, Item},
        position::{self, PositionEncoding},
        lsp_types::{
            CompletionItem, CompletionItemKind, CompletionItemLabelDetails, CompletionList,
            CompletionOptions, CompletionParams, CompletionResponse, Diagnostic,
            DiagnosticOptions, DiagnosticServerCapabilities, DidChangeTextDocumentParams,
            DidChangeWatchedFilesParams, DidChangeWatchedFilesRegistrationOptions,
            DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
            DocumentDiagnosticParams, DocumentDiagnosticReport, DocumentDiagnosticReportResult,
            DocumentFormattingParams, DocumentRangeFormattingParams, DocumentSymbol,
            DocumentSymbolParams, DocumentSymbolResponse, FileChangeType, FileSystemWatcher,
            FullDocumentDiagnosticReport, GlobPattern, Hover, HoverContents, HoverParams,
            HoverProviderCapability, InlayHint,
            InlayHintKind, InlayHintLabel, InlayHintParams,
            InitializeParams, InitializeResult, InitializedParams, Location, MarkupContent,
            MarkupKind, MessageType, OneOf, Position, PublishDiagnosticsParams, Range,
            ParameterInformation, ParameterLabel, ReferenceParams, Registration,
            RegistrationParams, RelatedFullDocumentDiagnosticReport,
            RelatedUnchangedDocumentDiagnosticReport, RelativePattern, RenameParams, SaveOptions,
            ServerCapabilities,
            ServerInfo, ShowMessageParams, SignatureHelp, SignatureHelpOptions,
            SignatureHelpParams, SignatureInformation,
            TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
            TextDocumentSyncSaveOptions, TextEdit, UnchangedDocumentDiagnosticReport, Url,
            WorkspaceEdit, WorkspaceSymbolParams,
            WorkspaceSymbolResponse,
            notification::{DidChangeWatchedFiles, Notification},
        },
        symbols,
    },
    analysis::{self, Analysis, DocumentChange, TextPosition, TextRange, ide, naming::DocumentKind},
    engine::{
        Engine,
        ide_view::{EngineIde, PathTable},
        queries::{Config as EngineConfig, FileDiagnostics, FileId, Key, ParsedDocument, RoughlyQueries},
    },
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
        collections::{HashMap, HashSet, hash_map::DefaultHasher},
        hash::{Hash, Hasher},
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
        let config = Config::from_path(Path::new(CONFIG_FILE_NAME))
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
    // The memoized query engine, fed in lockstep with `analysis_state` during the cutover (Phase 3): the
    // engine mirrors the document text as inputs so reads can flip to it one subsystem at a time, with
    // `analysis_state` staying the frozen oracle until the final slice removes it. `paths`/`file_ids` are
    // the host-owned path↔FileId bijection the engine keys on (the engine never sees paths).
    engine: Engine<RoughlyQueries>,
    paths: PathTable,
    file_ids: HashMap<PathBuf, FileId>,
    next_file_id: FileId,
    position_encoding: PositionEncoding,
    // When the client advertises pull-diagnostics support it owns the request cadence, so the
    // server stops pushing `publish_diagnostics` and answers `textDocument/diagnostic` instead.
    client_supports_pull_diagnostics: bool,
    // When a pull client also supports `workspace/diagnostic/refresh`, a package-visible save that
    // moves diagnostics in OTHER documents asks the client to re-pull. Push is suppressed for pull
    // clients, so without this their non-visible dependents would go stale after such a save.
    client_supports_diagnostic_refresh: bool,
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
            engine: Engine::new(RoughlyQueries::new()),
            paths: PathTable::new(workspace_root.clone()),
            file_ids: HashMap::new(),
            next_file_id: 0,
            position_encoding: PositionEncoding::Utf16,
            client_supports_pull_diagnostics: false,
            client_supports_diagnostic_refresh: false,
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

    fn to_internal_position(&self, path: &Path, position: Position) -> Option<TextPosition> {
        let rope = self.document(path)?.rope();
        Some(position::lsp_position_to_internal(
            rope,
            self.position_encoding,
            position,
        ))
    }

    fn to_internal_range(&self, path: &Path, range: Range) -> Option<TextRange> {
        let rope = self.document(path)?.rope();
        Some(position::lsp_range_to_internal(
            rope,
            self.position_encoding,
            range,
        ))
    }

    // The target of a definition, reference, or rename edit may live in a different document than
    // the request, so the outgoing range is encoded against that document's rope.
    fn to_lsp_range_in(&self, path: &Path, range: TextRange) -> Range {
        match self.document(path) {
            Some(document) => {
                position::internal_range_to_lsp(document.rope(), self.position_encoding, range)
            }
            None => Range::new(
                Position::new(
                    range.start.line_index as u32,
                    range.start.character_index as u32,
                ),
                Position::new(
                    range.end.line_index as u32,
                    range.end.character_index as u32,
                ),
            ),
        }
    }

    fn to_lsp_position_in(&self, path: &Path, position: TextPosition) -> Position {
        match self.document(path) {
            Some(document) => position::internal_position_to_lsp(
                document.rope(),
                self.position_encoding,
                position,
            ),
            None => Position::new(position.line_index as u32, position.character_index as u32),
        }
    }

    // Diagnostics are now served by the engine (Phase 3b). The caller still identifies the document by its
    // analysis `DocumentId` (the frozen oracle still selects the affected set in did_save); this maps it to
    // the engine `FileId` via the path and renders the engine's `FileDiagnostics`, gating the typing/strict/
    // unused classes by config exactly as production's `document_diagnostics` does. The type errors stay raw
    // and are rendered here against the engine's interner + fallback range (single source for the error set).
    fn convert_document_diagnostics(
        &self,
        document_id: analysis::DocumentId,
    ) -> Vec<crate::lsp_types::Diagnostic> {
        let path = self
            .analysis_state
            .path_for_document_id(document_id)
            .expect("diagnostics document present in analysis state")
            .to_path_buf();
        let file = self
            .file_ids
            .get(&path)
            .copied()
            .expect("diagnostics document present in engine file ids");

        let file_diagnostics = self.engine.fetch::<FileDiagnostics>(Key::Diagnostics(file));
        let fallback = *self.engine.fetch::<tree_sitter::Range>(Key::FallbackRange);
        let config = self.engine_config();

        let mut rendered = Vec::new();
        // Local naming, package naming, lowering (syntax), and lint are emitted unconditionally, exactly as
        // production's `document_diagnostics` does (they are not config-gated).
        rendered.extend(file_diagnostics.naming.iter().cloned());
        rendered.extend(file_diagnostics.package_naming.iter().cloned());
        rendered.extend(file_diagnostics.lowering.iter().cloned());
        rendered.extend(file_diagnostics.lint.iter().cloned());
        if config.unused {
            rendered.extend(file_diagnostics.unused.iter().cloned());
        }
        if config.typing {
            self.engine.group().with_interner(|interner| {
                for error in &file_diagnostics.type_errors {
                    rendered.push(analysis::Diagnostic::from_inference_error(
                        error, fallback, interner,
                    ));
                }
            });
        }
        if config.strict {
            rendered.extend(file_diagnostics.strict_diagnostics.iter().cloned());
        }

        let parsed = self.engine.fetch::<ParsedDocument>(Key::Parse(file));
        diagnostics::convert_diagnostics(rendered, parsed.0.rope(), self.position_encoding)
    }

    fn convert_location(&self, location: analysis::ide::Location) -> Location {
        Location {
            uri: Url::from_file_path(&location.path).expect("location path should convert to URI"),
            range: self.to_lsp_range_in(&location.path, location.range),
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

    fn engine_config(&self) -> EngineConfig {
        EngineConfig {
            typing: self.config.check.typing,
            strict: self.config.check.strict,
            unused: self.config.check.unused,
            lint: self.config.lint.clone(),
        }
    }

    fn file_id_for(&mut self, path: &Path) -> FileId {
        if let Some(file) = self.file_ids.get(path) {
            return *file;
        }
        let file = self.next_file_id;
        self.next_file_id += 1;
        self.file_ids.insert(path.to_path_buf(), file);
        file
    }

    // Mirror the current `analysis_state` document set into the engine inputs (Phase 3 transition).
    // `analysis_state` owns the text and the incremental edit logic; this re-derives every engine input
    // from it after each mutation so the engine stays a faithful shadow until reads cut over. `ProjectFiles`
    // is ordered package-documents-first in `package_document_ids` (package-path-key) order so the engine's
    // last-writer-wins symbol index selects the same winner production does; scripts follow (they export
    // nothing, so their order does not affect winner selection).
    fn sync_engine_from_analysis(&mut self) {
        let package_ids = self.analysis_state.package_document_ids();
        let package_set: HashSet<analysis::DocumentId> = package_ids.iter().copied().collect();
        let mut ordered_paths = Vec::new();
        for document_id in package_ids.iter().copied().chain(
            self.analysis_state
                .all_document_ids()
                .into_iter()
                .filter(|document_id| !package_set.contains(document_id)),
        ) {
            let Some(path) = self
                .analysis_state
                .path_for_document_id(document_id)
                .map(Path::to_path_buf)
            else {
                continue;
            };
            let Some(document) = self.analysis_state.document_by_id(document_id) else {
                continue;
            };
            ordered_paths.push((path, document.rope().to_string()));
        }

        let r_path = self.workspace_r_path();
        let mut project = Vec::new();
        let mut live = HashSet::new();
        for (path, text) in ordered_paths {
            let file = self.file_id_for(&path);
            let kind = if path.starts_with(&r_path) {
                DocumentKind::Package
            } else {
                DocumentKind::Script
            };
            self.engine.set_input(Key::SourceText(file), text);
            self.engine.set_input(Key::DocumentKind(file), kind);
            self.paths.insert(file, path);
            project.push(file);
            live.insert(file);
        }
        // Retract files that left the analysis set (a delete or a closed non-package document).
        let stale = self
            .file_ids
            .iter()
            .filter(|(_, file)| !live.contains(*file))
            .map(|(path, file)| (path.clone(), *file))
            .collect::<Vec<_>>();
        for (path, file) in stale {
            self.engine.remove_input(&Key::SourceText(file));
            self.engine.remove_input(&Key::DocumentKind(file));
            self.paths.remove(file);
            self.file_ids.remove(&path);
        }

        self.engine.set_input(Key::ProjectFiles, project);
        self.engine.set_input(Key::Config, self.engine_config());
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
        params: InitializeParams,
    ) -> BoxFuture<'static, Result<InitializeResult, ResponseError>> {
        tracing::info!(?self.experimental_features, "initialize");

        let client_encodings = params
            .capabilities
            .general
            .as_ref()
            .and_then(|general| general.position_encodings.as_deref());
        self.position_encoding = PositionEncoding::negotiate(client_encodings);
        tracing::info!(?self.position_encoding, "negotiated position encoding");

        self.client_supports_pull_diagnostics = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.diagnostic.as_ref())
            .is_some();
        self.client_supports_diagnostic_refresh = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.diagnostic.as_ref())
            .and_then(|diagnostic| diagnostic.refresh_support)
            .unwrap_or(false);
        tracing::info!(
            pull_diagnostics = self.client_supports_pull_diagnostics,
            diagnostic_refresh = self.client_supports_diagnostic_refresh,
            "negotiated diagnostics delivery"
        );

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

        self.sync_engine_from_analysis();

        box_future(Ok(InitializeResult {
            capabilities: ServerCapabilities {
                position_encoding: Some(self.position_encoding.kind()),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec!["$".into(), "@".into(), ":".into()]),
                    ..Default::default()
                }),
                definition_provider: Some(OneOf::Left(true)),
                diagnostic_provider: Some(DiagnosticServerCapabilities::Options(DiagnosticOptions {
                    identifier: Some("roughly".into()),
                    inter_file_dependencies: true,
                    workspace_diagnostics: false,
                    work_done_progress_options: Default::default(),
                })),
                document_formatting_provider: Some(OneOf::Left(true)),
                document_range_formatting_provider: Some(OneOf::Left(
                    self.experimental_features.range_formatting,
                )),
                document_symbol_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                inlay_hint_provider: Some(OneOf::Left(true)),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_owned(), ",".to_owned()]),
                    retrigger_characters: None,
                    work_done_progress_options: Default::default(),
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
        self.sync_engine_from_analysis();

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
        if !self.client_supports_pull_diagnostics {
            let diagnostics = self.convert_document_diagnostics(document_id);
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
        self.sync_engine_from_analysis();

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

        // Each incremental change's range refers to the document state after the previous changes
        // in the batch are applied, so positions are converted and applied against the live rope
        // one change at a time rather than all up front.
        for change in content_changes {
            let range = change.range.unwrap_or_else(|| {
                panic!(
                    "incremental did_change for {} must include a range",
                    path.display()
                )
            });
            let internal_range = self.to_internal_range(&path, range).unwrap_or_else(|| {
                panic!(
                    "analysis document not found while converting did_change range for {}",
                    path.display()
                )
            });
            let document_change = DocumentChange {
                range: internal_range,
                text: change.text,
            };
            self.analysis_state
                .edit_document(&path, std::slice::from_ref(&document_change))
                .unwrap_or_else(|error| {
                    panic!(
                        "failed to edit analysis document {} incrementally: {error:?}",
                        path.display()
                    )
                });
        }
        self.sync_engine_from_analysis();

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
        if !self.client_supports_pull_diagnostics {
            let diagnostics = self.convert_document_diagnostics(document_id);
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

        // Pull clients re-request the saved (visible) document on their own cadence, so the server
        // does not push. But push is suppressed for them, so a save that moved diagnostics in OTHER
        // (non-visible) documents would leave those dependents stale. Precisely detecting which
        // dependents moved is not reliably available here — `run_full` reports an affected set only
        // when type checking is enabled, yet cross-file naming diagnostics (e.g. "could not resolve")
        // move regardless — so conservatively ask the client to re-pull on every save. Saves are
        // infrequent and the client only re-pulls the documents it has open.
        if self.client_supports_pull_diagnostics {
            if self.client_supports_diagnostic_refresh {
                let mut client = self.client.clone();
                tokio::spawn(async move {
                    if let Err(error) = client.workspace_diagnostic_refresh(()).await {
                        tracing::error!(?error, "failed to request workspace diagnostic refresh");
                    }
                });
            }
            return ControlFlow::Continue(());
        }

        // Package-visible changes can move diagnostics in dependent files, so every document whose
        // typecheck output changed gets republished, not only the saved one.
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
            let diagnostics = self.convert_document_diagnostics(affected_document_id);
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
                match Config::from_path(&config_path) {
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
        self.sync_engine_from_analysis();

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
    // PULL DIAGNOSTICS
    //

    fn document_diagnostic(
        &mut self,
        params: DocumentDiagnosticParams,
    ) -> BoxFuture<'static, Result<DocumentDiagnosticReportResult, ResponseError>> {
        let uri = params.text_document.uri;
        let path = uri.to_file_path().unwrap();

        tracing::debug!(?path, "document diagnostic");

        // Unlike the sync notifications, a pull can legitimately target a document the server does
        // not track; answer with an empty full report rather than panicking.
        let Some(document_id) = self.analysis_state.document_id_for_path(&path) else {
            tracing::debug!(?path, "pull diagnostic for untracked document");
            return box_future(Ok(empty_full_diagnostic_report()));
        };

        // The full pipeline (typecheck included) is required so the pulled report equals what the
        // push path would send and reflects package-visible edits in dependent files.
        analysis::run_full(&mut self.analysis_state);
        let items = self.convert_document_diagnostics(document_id);
        let result_id = diagnostics_result_id(&items);

        // The result id is a content hash of the report, so an unchanged answer is correct even
        // under inter-file dependencies where the document's own edit version did not move.
        if params.previous_result_id.as_deref() == Some(result_id.as_str()) {
            return box_future(Ok(DocumentDiagnosticReportResult::Report(
                DocumentDiagnosticReport::Unchanged(RelatedUnchangedDocumentDiagnosticReport {
                    related_documents: None,
                    unchanged_document_diagnostic_report: UnchangedDocumentDiagnosticReport {
                        result_id,
                    },
                }),
            )));
        }

        box_future(Ok(DocumentDiagnosticReportResult::Report(
            DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                related_documents: None,
                full_document_diagnostic_report: FullDocumentDiagnosticReport {
                    result_id: Some(result_id),
                    items,
                },
            }),
        )))
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

        let internal_position = self
            .to_internal_position(&path, position)
            .expect("opened document rope available for completion");
        let completions = EngineIde::new(&self.engine, &self.paths)
            .completion(&path, internal_position)
            .map(
            |result| {
                CompletionResponse::List(CompletionList {
                    is_incomplete: result.is_incomplete,
                    items: result
                        .items
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
                                analysis::CompletionItemKind::Keyword => {
                                    CompletionItemKind::KEYWORD
                                }
                                analysis::CompletionItemKind::Variable => {
                                    CompletionItemKind::VARIABLE
                                }
                                analysis::CompletionItemKind::Function => {
                                    CompletionItemKind::FUNCTION
                                }
                            }),
                            ..Default::default()
                        })
                        .collect(),
                })
            },
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

        tracing::debug!(?path, ?position, "goto definition");

        if self.opened_document(&path).is_none() {
            tracing::info!(?path, "document not found");
            return box_future(Err(path_not_found_error(&path)));
        }

        let internal_position = self
            .to_internal_position(&path, position)
            .expect("opened document rope available for definition");
        let response =
            EngineIde::new(&self.engine, &self.paths).definition(&path, internal_position).map(|locations| {
                let mut locations = locations
                    .into_iter()
                    .map(|location| self.convert_location(location))
                    .collect::<Vec<_>>();
                match locations.len() {
                    1 => GotoDefinitionResponse::Scalar(
                        locations.pop().expect("single definition location"),
                    ),
                    _ => GotoDefinitionResponse::Array(locations),
                }
            });

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

        let internal_position = self
            .to_internal_position(&path, position)
            .expect("opened document rope available for hover");
        let Some(hover_info) = EngineIde::new(&self.engine, &self.paths).hover(&path, internal_position)
        else {
            tracing::debug!(?position, "hover target not found");
            return box_future(Ok(None));
        };

        let value = ide::render_hover_markdown(&hover_info, self.experimental_features.debug);

        let hover = Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: Some(self.to_lsp_range_in(&path, hover_info.range)),
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

        let viewport = self
            .to_internal_range(&path, params.range)
            .expect("opened document rope available for inlay hints");
        let raw_hints = EngineIde::new(&self.engine, &self.paths).inlay_hints(&path, Some(viewport));
        let hints = raw_hints
            .into_iter()
            .map(|hint| InlayHint {
                position: self.to_lsp_position_in(&path, hint.position),
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

        let internal_position = self
            .to_internal_position(&path, position)
            .expect("opened document rope available for signature help");
        let Some(help) = EngineIde::new(&self.engine, &self.paths).signature_help(&path, internal_position)
        else {
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

        let rope = document.rope();
        let last_line = rope.len_lines().saturating_sub(1);
        let document_end = position::internal_position_to_lsp(
            rope,
            self.position_encoding,
            TextPosition {
                line_index: last_line,
                character_index: rope.line(last_line).len_bytes(),
            },
        );
        let edits = vec![TextEdit {
            new_text,
            range: Range::new(Position::new(0, 0), document_end),
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

        let internal_range = self
            .to_internal_range(&path, range)
            .expect("opened document rope available for range formatting");
        let Some(node) = document.tree().root_node().descendant_for_point_range(
            Point::new(
                internal_range.start.line_index,
                internal_range.start.character_index,
            ),
            Point::new(
                internal_range.end.line_index,
                internal_range.end.character_index,
            ),
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
            range: position::tree_sitter_range_to_lsp(
                document.rope(),
                self.position_encoding,
                node.range(),
            ),
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

        let internal_position = self
            .to_internal_position(&path, position)
            .expect("opened document rope available for references");
        let references =
            EngineIde::new(&self.engine, &self.paths).references(&path, internal_position, include_declaration)
                .map(|locations| {
                    locations
                        .into_iter()
                        .map(|location| self.convert_location(location))
                        .collect()
                });

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

        let internal_position = self
            .to_internal_position(&path, position)
            .expect("opened document rope available for rename");
        let workspace_edit =
            EngineIde::new(&self.engine, &self.paths).rename(&path, internal_position, &new_name).map(
                |rename_result| {
                    let changes = rename_result
                        .edits
                        .into_iter()
                        .map(|(edit_path, edits)| {
                            let uri = Url::from_file_path(&edit_path)
                                .expect("rename edit path should convert to URI");
                            let edits = edits
                                .into_iter()
                                .map(|edit| TextEdit {
                                    range: self.to_lsp_range_in(&edit_path, edit.range),
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
                },
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

        let Some(document) = self.document(&path) else {
            tracing::error!(?path, "symbols not found");
            return box_future(Err(path_not_found_error(&path)));
        };
        // Document symbols are requested on every keystroke and only need top-level symbols, so
        // they intentionally read the parsed tree directly instead of running the analysis
        // pipeline.
        let items = index::index(document.tree().root_node(), document.rope(), false, false);
        let symbols: Vec<DocumentSymbol> =
            symbols::document(&items, &|range| self.to_lsp_range_in(&path, range));

        box_future(Ok(Some(DocumentSymbolResponse::Nested(symbols))))
    }

    fn symbol(
        &mut self,
        params: WorkspaceSymbolParams,
    ) -> BoxFuture<'static, Result<Option<WorkspaceSymbolResponse>, ResponseError>> {
        let query = params.query;

        tracing::debug!(?query);

        let workspace_items = self.package_items_map();
        let symbols = symbols::workspace(&query, &workspace_items, &|path, range| {
            self.to_lsp_range_in(path, range)
        });

        box_future(Ok(Some(WorkspaceSymbolResponse::Nested(symbols))))
    }
}

#[inline(always)]
fn box_future<T: Send + 'static>(content: T) -> BoxFuture<'static, T> {
    Box::pin(async { content })
}

fn empty_full_diagnostic_report() -> DocumentDiagnosticReportResult {
    DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Full(
        RelatedFullDocumentDiagnosticReport {
            related_documents: None,
            full_document_diagnostic_report: FullDocumentDiagnosticReport {
                result_id: None,
                items: Vec::new(),
            },
        },
    ))
}

// A content hash of the report, used as the pull `resultId`: a later pull carrying this id can be
// answered with `Unchanged` whenever the recomputed diagnostics hash to the same value.
fn diagnostics_result_id(items: &[Diagnostic]) -> String {
    let serialized =
        serde_json::to_vec(items).expect("lsp diagnostics are serializable for result-id hashing");
    let mut hasher = DefaultHasher::new();
    serialized.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn path_not_found_error(path: &Path) -> ResponseError {
    ResponseError::new(
        ErrorCode::REQUEST_FAILED,
        format!("path not found '{}'", path.display()),
    )
}

