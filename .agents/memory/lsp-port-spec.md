# Legacy LSP server + CLI port specification (Phase 3 working document)

Delete this file when Phase 3 completes — it is a distilled map of
`crates/roughly-legacy/src/server.rs` (+ index/symbols/position/config/
diagnostics) for building the new `crates/roughly` without re-reading the
legacy source. The legacy code stays the ground truth; verify against it
when detail matters.

## 1. Mainloop

`run()` = `#[tokio::main(current_thread)]`; `install_panic_hook()` first
(hook swallows `Cancelled` payloads silently, defers to default hook
otherwise; process death is the worker loop's exit(1), never the hook).
`MainLoop::new_server(|client| ...)`: mpsc channel of `Job`s; two
`Arc<AtomicBool>` flags `cancel` + `idle_interrupt`; spawn OS thread
"roughly-engine" with large stack running the worker; tower stack:
TracingLayer → LifecycleLayer → CatchUnwindLayer →
ClientProcessMonitorLayer → Router. NO ConcurrencyLayer (the serial
worker is the concurrency bound; that layer's backpressure deadlocks).
Unix transport: async_lsp::stdio::PipeStdin/PipeStdout::lock_tokio();
non-Unix tokio stdin/stdout via tokio_util compat. `run_buffered`.

Registered requests: initialize, document_diagnostic, completion,
definition, type_definition, code_action, hover, inlay_hint,
semantic_tokens_full, signature_help, formatting, range_formatting,
references, rename, document_highlight, folding_range, document_symbol,
workspace symbol. Notifications: initialized, did_open, did_change,
did_close, did_save, did_change_watched_files, did_change_configuration.

Capability negotiation from InitializeParams: position encoding utf-8 if
offered else utf-16; pull diagnostics = text_document.diagnostic present;
refresh = workspace.diagnostic.refresh_support; label offsets =
signature_help.signature_information.parameter_information
.label_offset_support; snippets = completion_item.snippet_support.

ServerCapabilities: position_encoding = negotiated;
code_action_provider = Options{kinds:[QUICKFIX]}; completion trigger
chars ["$","@",":","\""]; definition/type_definition true;
diagnostic_provider Options{identifier:"roughly",
inter_file_dependencies:true, workspace_diagnostics:false};
formatting true; range_formatting only if experimental flag;
document_symbol true; hover true; inlay_hint true; signature_help
trigger ["(",","]; references true; document_highlight true;
folding_range true; rename plain true; semantic_tokens legend
[TYPE, TYPE_PARAMETER, PARAMETER, OPERATOR, DECORATOR] no modifiers,
full=true; text sync open_close, INCREMENTAL, save{include_text:false};
workspace_symbol true; server_info name+version from env!.

## 2. Threading

ServerState{sender, cancel, idle_interrupt} on the tokio thread; worker
owns everything else. Job enum: Initialize(params, oneshot) | Read(box
FnOnce(&mut Worker)) | Write(box FnOnce(&mut Worker)).
- read(): oneshot; send Job::Read; then idle_interrupt=true; await.
- notify_edit(): cancel=true FIRST, then send Job::Write (PANIC if send
  fails — sync edit loss is unrecoverable), idle_interrupt=true.
- notify(): same without cancel flip.
Worker loop: if has_idle_work → reset idle_interrupt=false BEFORE
try_recv; Empty → run one idle unit; else blocking recv. Every job in
catch_unwind → on panic exit(1). Read jobs reset cancel=false first.
Write before init → drop with warning.
Cancellation: reads wrap engine access in with_cancellation(cancel);
Err(Cancelled) policy: best-effort features → unwrap_or_default();
document_diagnostic → SERVER_CANCELLED + data {"retriggerRequest":true};
rename → CONTENT_MODIFIED error.
New stack mapping: salsa Storage<Db> is Clone; a setter on the main
handle cancels other handles' in-flight queries (Cancelled::PendingWrite
unwind). Catch via salsa::Cancelled::catch. RootDatabase needs
#[derive(Clone)] equivalent (manual impl Clone via storage.clone()).

## 3. Document sync

Worker state: workspace_root; open_documents set; documents map (open R
buffers); stub_documents (.Rtypes ropes); namespace_documents (NAMESPACE
ropes); file id/path tables; virtual_document_uris; symbol cache;
pending_semantic_publishes; prime_queue.
- URI→path: file: → fs path; other schemes → percent-encoded synthetic
  path under <root>/.roughly-virtual/ (deterministic, never on disk;
  is_package false).
- did_open: stub → store rope, publish stub diagnostics (push clients),
  return. NAMESPACE → same via namespace diagnostics. Else parse
  (panic on failure), track, feed input (Package iff under <root>/R),
  rebuild project files, sync open set; push → first wave + defer.
- did_change: apply changes sequentially against evolving buffer (no
  range = full replace; ranged = incremental edit); feed input; no
  project-files rebuild; push → first wave + defer.
- did_save: only for open tracked docs → refresh_all_diagnostics()
  (a package-visible save moves diagnostics in dependents).
- did_close: stub/NAMESPACE → drop. Package file → re-read from disk
  (revert to on-disk text); read failure → retract. Script/virtual →
  retract + rebuild.
- Startup: scan <root>/R non-recursive for *.R/*.r sorted, read
  (panic on failure), feed as Package. Project stubs from <root>/stubs.
  Prime queue filled in `initialized`.
New stack mapping: SourceFile salsa inputs per file; ProjectFiles input
(package files FIRST sorted by workspace-relative path, then scripts —
last-writer-wins winner order matches CLI); document text edits via
SourceFile setter. DocumentKind::Package iff under R/.

## 4. Diagnostics

Push clients: wave 1 immediately on open/change = cheap classes (syntax
+ naming + lints; strict escalation applied identically) with document
version; wave 2 at idle = full set (adds type/unused/strict per config +
per-file typing mode) published version-less. Wave 1 must be a subset of
wave 2. pending_semantic_publishes served LIFO (most recent last, pop
from back); cancelled idle publish → requeue. Idle with nothing owed →
prime one file from prime_queue (never published).
Pull clients: push fully suppressed. document_diagnostic → full report
with result_id = 16-hex hash of serialized diagnostics; matching
previous_result_id → Unchanged report. Untracked/unopened → empty full
report (result_id None). Cancelled → SERVER_CANCELLED +
retriggerRequest:true (pinned wire shape).
refresh_all_diagnostics (save/config change): pull+refresh-support →
workspace/diagnostic/refresh (spawned on runtime); push → defer publish
for every open doc.
LSP mapping: severity map; code = string; source "roughly"; tags
UNNECESSARY for unused/unused-parameter/unused-import.
Rendering tail: apply_suppressions (# roughly: allow(code,...) on same
line or line above; `all` wildcard) against source text, then encode.
Config-file diagnostic: malformed roughly.toml published ON the toml
file URI: range = (line-1,col-1)..(+1 col), ERROR, code "config",
message = error.to_string(); cleared with empty publish when it parses.
Init-time config error surfaced in `initialized` via showMessage ERROR +
config diagnostic.
New stack gating: file_diagnostics(db,file) has code field per class —
host filters: always syntax/annotation/naming(unresolved)/lints;
"unused" iff check.unused; "type" iff typing on; strict_diagnostics
appended iff strict on; typing mode per file via file_typing_mode
(# typing: comments + #: @strict) overrides config; strict escalates
unresolved to error.

## 5. Config

roughly.toml; discover = absolutize + lexical normalize + ancestor walk
from target dir; nearest wins; none → defaults. Schema: [format]
indent-width (2), line-ending auto|lf|cr-lf; [lint] naming-style +
per-lint levels (default|off|warn|error) incl. obsolete missing-comma;
[check] unused/typing/strict bools; top-level debug bool; legacy compat
top-level `case` → naming-style, `spaces` → indent-width; all tables
serde default+kebab+deny_unknown_fields; parse errors carry 1-based
line/col from toml span. Watched registrations (in `initialized`):
<root>/R *.[rR]; <root> roughly.toml; ancestor config dir roughly.toml
when governing config sits above root. Reload on watched change matched
by FILE NAME == roughly.toml → re-discover from root; failure → keep
previous config + report + config diagnostic; success → clear config
diagnostic; if changed → re-feed config + refresh_all_diagnostics.

## 6. Positions

Internal = byte offsets (new stack) / line+byte-col (legacy). LSP → 
internal: clamp line to last line, clamp col to line length; UTF-16 cols
convert via char walk. Internal → LSP symmetric. Conversions ONLY at the
edge, against the target document's text (cross-file targets convert
against that file's text). Signature-help label offsets are ALWAYS
UTF-16 code units regardless of negotiated encoding. Out-of-bounds
positions clamp, never error.

## 7. Features

- hover: markdown; config.debug appends debug section; range converted.
- definition: ide::definition → Scalar/Array. .Rtypes buffers: resolve
  type name under cursor via stub line tokens → jump to `@type NAME`
  line in the same file.
- references(include_declaration from params.context).
- document_highlight = references filtered to same doc, kind None.
- rename: reject invalid R identifier upfront (INVALID_PARAMS,
  "`x` is not a valid R identifier"; ident = letter or . not followed by
  digit, then alnum/./_; reserved words excluded); Cancelled →
  CONTENT_MODIFIED; result → WorkspaceEdit{changes} converted per-file.
- code actions: → CodeAction{title, kind QUICKFIX, edit}.
- completion: CompletionList{is_incomplete, items}; kind map
  Keyword→KEYWORD Variable→VARIABLE Function→FUNCTION Field→FIELD
  Type→STRUCT; label_details.description = source name; snippets iff
  client supports AND kind Function: no-args → "label()$0" else
  "label($0)", insert_text_format SNIPPET, command
  editor.action.triggerParameterHints "trigger parameter hints".
- inlay hints: viewport from params.range; InlayHint{position, label
  String, kind TYPE, padding false/false}.
- signature help: per signature label + parameters as UTF-16
  LabelOffsets pairs (or Simple text without client support);
  active_signature top-level + mirrored active_parameter.
- formatting: whole-doc; format refusal (syntax errors) → Ok(None);
  else single TextEdit spanning whole doc with formatted text.
- folding: multi-line brace/paren/argument-list nodes → Region folds
  (end line = closing line - 1); consecutive comment runs → Comment
  folds (#: blocks fold as one).
- document symbols: outline from tree (assignment forms incl. -> ->>,
  braced groups flattened, call recognizers setClass/setGeneric/
  setMethod/R6Class with children for R6 public/private/active) +
  @type/@alias declarations appended (STRUCT/INTERFACE, detail =
  directive name); sorted by position; Nested.
- workspace symbols: all package files + open scripts, memoized outline
  per file, shared fuzzy matcher, global sort by (score,name), cap 128,
  container_name for one child level.
- semantic tokens: #: comment bodies → type-notation tokens (TYPE,
  TYPE_PARAMETER, PARAMETER, OPERATOR for separators/variadic,
  DECORATOR for @directives), delta-encoded, skip zero-length. .Rtypes
  buffers: per-line stub lexer tokens. result_id None.

## 8. .Rtypes + NAMESPACE buffers

Served server-side only, never in the db. Stub buffers: diagnostics =
loader problems as whole-line ERROR code "stub"; semantic tokens per
line; goto type name within file. NAMESPACE buffers: parse importFrom →
validate against stub exports-by-namespace table → diagnostics. Both
answer pull.

## 9. CLI (from cli.rs/main.rs, test_cli.rs contract)

Commands: check [files] [--output human|json] [--min-severity ...];
fmt|format [files] [--check] [--diff] [-v]; server|lsp [--stdio];
debug analysis-stats|ast|index. Global --experimental-features.
Exit codes: 0 clean, 1 findings, 2 usage/config/IO errors (documented
contract). Commands run on a big-stack thread. check: discover config
per target, build project from package R/ dir + given scripts, assemble
diagnostics (same gating as server incl. suppressions), render human
(console colors) or json. unused-import lint: parse NAMESPACE imports,
validate usage via token scan (CLI-only). fmt: format files, --check
lists would-change files exit 1, --diff prints diffs.

## 10. Port order (remaining)

DONE: skeleton + CLI + test_cli (25 tests) + server.rs (full feature
surface per §1-§7) + test_lsp core (18 tests: capabilities, encodings
incl. non-BMP hover ranges, push waves, burst settling, pull result-id
semantics + unchanged reports, push suppression, formatting + refusal,
goto, workspace-root-from-client, malformed-config fallback). The
cancellation model: worker refreshes to a fresh db handle at every job
start when its token was cancelled (a flip is consumed by the in-flight
query it killed) — see §12.

REMAINING:
1. Port the rest of test_lsp coverage (~60 more tests in the legacy
   file): out-of-bounds safety per feature, references/rename behaviors,
   completion contexts + snippets assertions, inlay viewport, signature
   label offsets against a label-offset client, semantic-token payloads,
   config reload live (watched-files event → refresh), did_close disk
   reread, untitled documents, stub/.Rtypes + NAMESPACE buffer serving,
   dependency-affecting save → refresh request, pull-client refresh,
   breaking-one-file independence, coherence-panic death test.
2. Close §11 product gaps (globalVariables, duplicate-binding related
   notes, stub-loader problems + @masked validation, richer document
   symbols with kinds/hierarchy).
3. Memory re-measure ~700K LoC; witnesses as CI-checkable artifacts;
   flip workspace default-members to crates/roughly; update
   architecture.md/structure.md/testing.md for the cutover; decision
   record for Phase 3 completion.

## 11. Known product gaps found while porting the CLI contract (close before Phase 3 completes)

- `utils::globalVariables(c(...))` names must suppress unresolved warnings
  package-wide (legacy naming feature; new naming lacks it).
- Duplicate top-level binding warnings with related locations ("the
  later/earlier binding is here", package_naming class) — new stack has no
  related-location model on Diagnostic and no duplicate-binding warning.
- Stub-loader problem reporting: declarations the loader drops must render
  as per-line findings (`does not load`, stub-file positions) in check and
  on .Rtypes buffers; `@masked` on a non-variadic type must error
  ("requires a variadic function type"). New loader drops silently.
- CLI `debug analysis-stats` and `debug index` intentionally NOT ported
  (legacy-only instruments; test_stats covers measurement on the new
  stack). Related-notes JSON field emits `[]` until the related model
  exists.

## 12. Salsa cancellation mechanics (verified against salsa 0.28 source)

- `db.cancellation_token() -> CancellationToken` (Arc'd flag, per handle /
  ZalsaLocal). Frontend holds a clone; `token.cancel()` makes the worker's
  in-flight query unwind with `Cancelled::Local` at its next salsa
  operation. Catch with `salsa::Cancelled::catch(|| ...)`.
- The token resets via the attach machinery when a new top-level query
  attaches (`uncancel` in attach.rs) — verify empirically in the suite; if
  reset timing surprises, the worker can guard reads with its own
  is_cancelled check before starting.
- `db.trigger_cancellation()` (&mut self) = cancel ALL other handles and
  wait (zalsa_mut) — that is what input setters do implicitly; do not call
  it on the worker thread while the worker holds the only handle.
- RootDatabase already derives Clone (handle clone shares storage).
- Plan: single worker thread owns the db + all documents (same Job enum
  protocol as legacy: Initialize/Read/Write over mpsc; oneshot replies;
  cancel token replaces the engine's AtomicBool; idle_interrupt can stay a
  plain AtomicBool checked between idle units, with the salsa token used
  for intra-query interruption of idle work too).
