---
title: REPL design
description: "How the R console loads R at runtime with no build-time linking"
---

**Status: v1 SHIPPED and e2e-VERIFIED against real R** as `crates/repl`
behind `ry repl` (user decisions: subcommand packaging, reedline +
nu-ansi-term kept, rofy frozen under `legacy/`). Implemented: discovery, the
typed runtime-binding layer, the ReadConsole-hosted reedline console with
lexer highlighting and conservative completeness, SIGINT interrupt routing,
pty e2e tests (skip-if-no-R) in the ry crate, the headless runner
(`ry run` / `repl --file`), vi keybindings, and the Windows embedding
(verified on real Windows + R — see its section). The full pty suite runs
green against a real R on Unix ptys AND Windows ConPTY (interactive
evaluation, multiline, error stream, Ctrl-C interruption, both batch
directions) — R installs in agent containers via apt + the CRAN repository,
so the suite is runnable anywhere, not just on a developer machine. Two harness facts a rewrite must keep: the pty driver
must *answer* reedline's cursor-position query (`ESC[6n` → `ESC[1;1R`) or
the editor blocks before its first prompt, and interactive sessions must
run serialized (in-session `SESSION_LOCK`) — concurrent R sessions on a
loaded machine are flaky while serial runs are stable. **Analysis-backed
Tab completion SHIPPED** (the first analysis rung): the repl crate exposes
a `SessionCompleter` seam (accept + complete; the console feeds accepted
lines back, an `Arc<Mutex>` shares the object between reedline's menu and
the accept path) so the crate stays syntax-only, and the host implements
`AnalysisCompleter` (`crates/ry/src/repl_completer.rs`) —
`ide::completion` over the session-as-script (one salsa `SourceFile`
updated per request; stubs + export manifests installed), so the menu
shows typed signatures for stdlib names, session bindings, `pkg::`
exports, and manifest names; Tab is bound in both emacs and vi-insert
modes. E2e caveat: menu interactions over a pty are stateful — the e2e
pins one menu render + accept; the completer's behavior is pinned by its
unit tests. Not yet: live-session facts (the R environment listing
unioned in), pre-eval diagnostics, hover on the input line, graphics
devices.

User-initiated: integrate a first-class REPL into ry — the successor to the
`rofy` experiment — **without any build-time link dependency on R**. This
document records the architecture that makes that possible (verified against a
production-grade Rust R kernel's source; techniques described on their own
terms) and how ry's analysis stack turns a console into something more
than an echo loop.

## Why rofy's approach is a dead end

`rofy` embeds R through `extendr`/libR-sys: bindgen runs at build time against
a local R's headers and the binary carries a load-time dynamic dependency on
libR. Consequences: the build machine needs a matching R (why rofy is excluded
from CI and every gate), the artifact is bound to the R it was built against,
and a missing symbol is a loader failure rather than a recoverable fact.

## The core technique: bind R at runtime, per symbol

Do not link. `dlopen` R's shared library at startup and resolve every needed
symbol by name, with per-symbol optionality:

- **A hand-curated binding surface, not bindgen.** One declaration list of the
  C-API functions, variadic functions (`Rf_error`, `Rprintf` — stored with real
  `...` types, exposed at fixed arity), mutable globals
  (`R_interrupts_pending`, `R_Interactive`, the `ptr_R_ReadConsole` /
  `ptr_R_WriteConsoleEx` hook pointers, `R_PolledEvents`, `R_SignalHandlers`),
  and value-snapshotted constants (`R_GlobalEnv`, `R_NilValue`, …). Declarative
  macros expand each declaration into a `static Option<fn ptr>` plus a
  passthrough wrapper; resolution is **eager and batched** at init (one dlsym
  sweep), not lazy per call.
- **A missing symbol is `None`, not a crash.** Every binding gets a
  `has::name()` probe; call sites branch on it to provide fallbacks on older
  R (e.g. a newer accessor when present, the classic macro-equivalent
  otherwise). This is the version-compatibility story: resolve optimistically,
  degrade per symbol. A hard version floor (parse
  `{R_HOME}/library/base/DESCRIPTION`) keeps the fallback matrix small —
  R >= 4.2 mirrors current ecosystem practice.
- **Two-phase init, load-bearing order.** Functions + mutable globals bind
  BEFORE `Rf_initialize_R`; the constant globals are copied by value only
  AFTER `setup_Rmainloop()` (R initializes them there). The library handle is
  leaked — held for the process lifetime.
- **Loader flags matter.** Unix: `RTLD_LAZY | RTLD_GLOBAL`, so compiled
  package `.so`s that link libR resolve R symbols as if the host had linked R
  itself; additionally set `LD_LIBRARY_PATH`/`DYLD_LIBRARY_PATH` to
  `{R_HOME}/lib` so those packages can *find* a libR at their own dlopen time
  (the `RTLD_GLOBAL` symbols then shadow it). macOS needs the
  dyld-environment entitlement on the binary. Windows: open `R.dll` plus its
  sibling DLLs (`Rblas`, `Rlapack`, `Riconv`, `Rgraphapp`) so the loaded-module
  list satisfies package imports; per-module symbol lookup means no
  `RTLD_GLOBAL` equivalent is needed.
- **Keep ABI-drifting structs off the surface.** R's `DevDesc`/`Rstart` change
  layout across versions; the reference approach mirrors them per engine
  version and casts at runtime. v1 of our REPL simply avoids those surfaces
  (no custom graphics device); the technique is recorded for when plots come.

## Discovery

1. `R_HOME` env var when set (editor/CI can pin the version).
2. Else run `R RHOME` from `PATH` (`R.exe`/`R.bat` on Windows) and read stdout;
   re-export `R_HOME` so R's own `R_HomeDir()` agrees.
3. Shared library at `{R_HOME}/lib/libR.{so,dylib}` (the macOS framework's
   R_HOME already points into `Resources/`), `{R_HOME}/bin/x64/R.dll` on
   Windows. Fail with an actionable message naming `--enable-R-shlib` when the
   shared library is absent.
4. Secondary vars (`R_SHARE_DIR`, `R_INCLUDE_DIR`, `R_DOC_DIR`) are exported
   by R's shell wrapper (`{R_HOME}/bin/R`), which embedding bypasses; on
   layouts that relocate those directories (Fedora, RHEL) `R.home("share")`
   would otherwise resolve to nonexistent `{R_HOME}/<dir>` paths. Recovered
   by parsing the wrapper's plain `VAR=value` lines (values are substituted
   literally at R's install time; a subprocess asking R directly would cost
   a few hundred milliseconds of startup). Unix-only — Windows R derives
   them from `R_HOME` internally.

## Process shape and the console loop

- **R owns the process main thread** (its stack checks and signal expectations
  assume it); everything else — protocol threads, the analysis engine, output
  capture — is background threads of the same process. R initializes once per
  process; tests therefore run one-process-per-test (exactly why the test
  harness choice matters; see Testing).
- Init: suppress R's signal handlers (`R_SignalHandlers = 0`, install our
  own), `Rf_initialize_R` with `--interactive --no-save --no-restore-data`,
  hook `ptr_R_ReadConsole` / `ptr_R_WriteConsoleEx` / `ptr_R_Busy` /
  `ptr_R_Suicide`, set `R_PolledEvents` + `R_wait_usec` so long-running R code
  polls us, then `setup_Rmainloop()` and `run_Rmainloop()` — drive R's REAL
  REPL through the console hooks (not `R_ReplDLLdo1`), which keeps browser
  prompts, `readline()`, and nested REPLs honest.
- **The ReadConsole callback is the scheduler.** When R asks for input, we are
  at a safe idle point: classify the prompt (top-level vs `browser()` vs
  `readline()` input request), then park in a channel-select over: user input,
  eval requests, and idle work — with a periodic tick that runs R's input
  handlers so background R machinery (help server, event-loop packages) stays
  live. Feed R **one expression per read**, parsed and split by US first.
- **One thread touches R — by construction.** The editor runs inside the
  ReadConsole hook, so the console and R share the main thread and no
  cross-thread marshaling layer exists. The analysis rung keeps it that way:
  analysis may run on background threads, but live-session facts (loaded
  namespaces, `ls()` of the global env, frame columns) are fetched only while
  R is parked at a prompt, on the main thread, between reads — background
  threads never call into R.
- **Terminal width is ours to set.** R's `width` option defaults to 80 columns
  whatever the terminal is, and R exports no setter, so a table with room on
  screen still wrapped. The width is measured once and applied with
  `R_ParseEvalString` in the window between `setup_Rmainloop()` and
  `run_Rmainloop()` — the profiles have been sourced, so a profile that chose a
  width of its own is left alone (only R's untouched default is replaced), and
  nothing the console does can be perturbed because no prompt has run yet.
  Feeding `options(width = …)` through the ReadConsole hook instead is what the
  first attempt did and it is wrong twice over: it costs a main-loop round
  trip, and a round trip taken before the editor's first prompt leaves the
  terminal in the state R found it in, which desynchronises the next read. A
  **resize is not tracked** — a stock terminal session handles `SIGWINCH` by
  calling `R_SetOptionWidth`, which is not exported, and the console is the
  only other way in.
- **Interrupts:** block SIGINT everywhere except the R thread; an interrupt
  request sets `R_interrupts_pending` (Unix via the signal, Windows via
  `UserBreak`) and R honors it at its next check; while waiting on input we
  poll the flag and long-jump via `Rf_onintr` ourselves.
- **Errors never cross Rust frames.** Every C→Rust callback body is a
  plain-old-frame guarded by `R_ToplevelExec` (+
  `R_withCallingErrorHandler` for structured condition capture);
  `extern "C-unwind"` throughout. Output capture is two-layer: the
  WriteConsoleEx hook for R-level output, plus fd-level dup/pipe capture for
  C `printf` output that bypasses R's console.

## Windows implementation (VERIFIED on real Windows + R 4.5.2: the full pty
e2e suite runs green over ConPTY, plus `ry run` exit codes, `system()`,
`~` expansion, and `.Platform$GUI`)

Windows R embedding does NOT use the Unix `ptr_R_ReadConsole` globals — it
wires callbacks through the `Rstart` struct. The working recipe (now
machine-verified end to end):

- **Load**: `{R_HOME}\bin\x64\R.dll` (plain `bin\` on ARM64) via
  `LoadLibrary`, after preloading the sibling DLLs (`Rblas`, `Rlapack`,
  `Riconv` best-effort) so compiled-package imports resolve; Windows'
  per-module symbol lookup means no `RTLD_GLOBAL` equivalent is needed.
  **`Rgraphapp.dll` is a hard requirement, not a best-effort preload: it —
  not `R.dll` — exports `GA_initapp`, and skipping the call leaves graphapp
  uninitialized, which crashes `readconsolecfg()` with an access violation**
  (this exact miss was the original Windows-crash root cause: resolving
  `GA_initapp` against `R.dll`, finding nothing, and treating it as
  optional).
- **Discovery**: `R_HOME` env, else `R.exe RHOME` from `PATH` (registry
  lookup can come later).
- **Init order (load-bearing)**: `cmdlineoptions(1, [name])` →
  `R_DefParamsEx(&rstart, RSTART_VERSION)` (the version handshake makes R
  validate the struct layout — this replaces per-R-version struct
  mirroring) → fill the callbacks (`ReadConsole`, `WriteConsoleEx` with
  plain `WriteConsole` NULLed, `ShowMessage`, `YesNoCancel`, `CallBack`,
  `Busy`, `Suicide`), `R_Interactive = 1`, `rhome` from discovery and
  `home` from R's own `getRUser()` (NOT `USERPROFILE`: R's `~` is the
  Documents folder, and the `R_LIBS_USER` default hangs off it — the wrong
  `home` silently loses the user's installed packages) →
  `CharacterMode = RGui` so `R_SetParams` wires the callback set →
  `GA_initapp(0, NULL)` (from `Rgraphapp.dll`, required) →
  `readconsolecfg()` → only then switch `CharacterMode` to `LinkDLL`,
  BEFORE `setup_Rmainloop`: keeps the RGui callback wiring while avoiding
  `do_system`'s `SetStdHandle` invalidation (which hangs `system()` calls;
  verified un-hung) → `setup_Rmainloop()` → `run_Rmainloop()`.
- **`.Platform$GUI`**: RGui-mode init stamps it `"Rgui"`, and the LinkDLL
  flip does not retroactively update it — packages take `"Rgui"` as license
  to call Rgui-only GUI functions (menus, dialogs) that fail here. The
  console feeds a first hidden line that rebinds it to `"ry"` in
  `baseenv()` (unlock/relock `.Platform`) before any user input. Known gap:
  R sources the startup profiles before the first console read, so profile
  code still sees `"Rgui"`; fixing that would mean suppressing native
  profile loading and sourcing them manually after init — deliberately not
  taken on.
- **Encoding**: `ry.exe` embeds a Windows application manifest
  declaring UTF-8 as the active code page (`crates/ry/build.rs`, MSVC
  linker `/MANIFESTINPUT` — no build dependency). Embedded R (4.2+, UCRT)
  takes its native encoding from the host process's code page and `R.exe`
  declares UTF-8 the same way; without the manifest R runs in the system
  ANSI code page on any machine that has not enabled UTF-8 system-wide, and
  text handling silently diverges from stock R. Machines WITH the
  system-wide UTF-8 option mask the gap — verify encoding claims on a
  default-locale machine (`l10n_info()` must report codepage 65001).
- **Line endings**: every path into the console feed normalizes CRLF (and
  lone CR) to `\n` — R's parser reports a raw `\r` as "unexpected invalid
  token". Two real carriers: script files on Windows, and the editor's
  multiline buffer, which joins continuation lines with `\r\n` there
  (single-line interactive input never carries one).
- **Interrupt**: a `SetConsoleCtrlHandler` handler sets BOTH `UserBreak`
  (the front-end break flag) and `R_interrupts_pending` (the deferred
  flag); clear both when handling. Ctrl-C over ConPTY reaches the handler
  only while R evaluates (raw editor mode disables `ENABLE_PROCESSED_INPUT`),
  exactly as intended; the e2e interrupt test passes.
- **Editor**: reedline runs on Windows terminals; the field carries a
  crossterm patch for VT input handling — expect that caveat at the editor
  layer.
- **E2e**: the pty suite drives the same harness through ConPTY
  (`portable-pty`'s native pty), so REPL-touching changes are verifiable on
  Windows machines with R exactly like on Unix.

## Console UX backlog (surveyed against the field)

Parity items observed in production Rust R consoles, all compatible with our
architecture; none block the analysis rung:

- **Line-editor upgrade** (we pin an old reedline): newer versions add an
  idle-callback hook — the natural seam for running analysis between
  keystrokes — plus vi mode and configurable keybindings that come for free.
  Note the editor's chrono dependency is unwanted baggage (only its default
  prompt clock and sqlite-history timestamps use it); see the Apple-framework
  decision record for why that matters at release time.
- **History**: sqlite backend (an editor feature flag) and import from
  `.Rhistory`/radian history formats — low cost, removes a migration step.
- **E2e assertions**: parse pty output through a vt100 screen model instead
  of grepping raw transcripts — robust against redraws and cursor movement.
- **Help**: a fuzzy help browser over installed packages, which comparable
  consoles provide; ours should come from the analysis stack (hover docs already
  exist) rather than a parallel Rd pipeline.
- **Reprex mode**, rendered through our own formatter.
- Auto-matching brackets / smart quotes; TOML-config for colors and prompts
  once a console config story exists.

## No kernel protocol (settled)

The reference architecture this design was verified against is a notebook
kernel: its frontend lives in another process, so it carries a wire protocol
(message sockets, serialization, signing, ordering, heartbeats), comm
channels for UI surfaces, and — the structural consequence — a marshaling
layer that ships work from protocol threads onto the R thread at safe
points. None of that applies here, and dropping it is a settled decision,
not a v1 gap: the frontend is in-process (the editor runs inside the
ReadConsole hook), so exactly one thread ever touches R and the "protocol"
is a function call. If a remote or GUI frontend is ever wanted, it becomes a
second frontend over the runtime layer (`libr.rs`) with its own process
shape — IPC does not get threaded through the console. Editor integration
is already the LSP's job.

## Headless runner (v1 SHIPPED) and file pre-loading

`ry run script.R` executes a file through the embedded runtime and
exits at its end; `ry repl --file script.R` (also `-f`) feeds the same
script and then hands over to the interactive prompt. Mechanism (decided):
the ReadConsole frontend feeds the script bytes exactly like accepted
console input, and in batch mode answers end-of-input once they are
consumed — no second driver, identical parse/eval/autoprint semantics to
the console. Exit-code propagation without new C surface: batch mode
prepends `options(error = function() q(status = 1, save = "no"))`, so a
top-level error halts the script and the process exits 1 (plain-Command e2e
tests pin exit 0 + output and exit 1 + halt; skip-if-no-R like the rest).
Vi keybindings shipped alongside: `ry repl --keybindings vi` (emacs
default) — the editor's built-in vi mode, no console config story needed
yet. Still ahead for the runner: run TypedR files directly (typecheck,
compile in memory, execute — see [Inline type syntax](/contributing/design/inline-type-syntax/)).

## What makes it better than rofy (the previous integration)

The REPL is not a goal in itself — the point is a console with the analyzer in
the same process:

- **Our parser drives input.** Continuation ("is this input complete?") comes
  from `crates/syntax`, not from feeding R and watching for parse state; we
  can syntax-highlight and error-squiggle the input line as it is typed.
- **Typed completions**: `crates/ide` completions over the script-so-far,
  UNIONED with live-session facts (loaded namespaces, `ls()` of the global
  env, column names of in-memory frames) fetched through the idle-task seam.
  The session becomes another resolution layer on top of the stub corpus.
- **Diagnostics before evaluation**: run the checker on the pending input
  against the accumulated session "document" (the REPL history is a script
  document; the engine already models script scoping top-down).
- **A runtime type bridge (later)**: observed classes/types of session values
  can seed or validate stubs — the CRAN-introspection idea from the backlog
  gets an interactive on-ramp.
- **Formatter on history**, hover on the input line, and `#:` annotations
  usable interactively.

## Plan sketch (when scheduled)

1. A runtime-binding crate of our own (hand-curated minimal surface — only
   what the console needs; grows on demand). Original declarations, written
   from R's public headers; the reference implementation is study material,
   not a source to copy.
2. A console host crate: discovery, init, the ReadConsole select loop,
   interrupt/output plumbing. TUI line editing reuses the existing rofy
   front-end experience where it fits.
3. Wire `semantics`/`ide` in behind the idle-task seam (completions first,
   then pre-eval diagnostics).
4. Retire `rofy` once parity is reached (it stays untouched until then).

## Testing

R initializes once per process: embedded-R tests need one-process-per-test
execution and a one-shot init fixture (raise `R_CStackLimit` when R runs off
the main thread in tests). CI has no R, so embedded tests stay excluded from
the workspace gates exactly as rofy's are — but the binding layer's
declaration list and discovery logic are plain Rust, testable everywhere; keep
the R-requiring surface as thin as possible.

## Constraints and costs (accepted with eyes open)

- No subprocess isolation: an R crash kills the REPL process (mitigate with
  trap handlers + frontend restart, not in-process recovery).
- The dyld entitlement on macOS, the `LD_LIBRARY_PATH` arrangement, and the
  Windows DLL preload set are distribution obligations that come with runtime
  loading.
- Env-var mutation on Windows must go through R (`Sys.setenv`) once R is up —
  the C and Win32 environment spaces diverge.
- The binding list is hand-maintained; the `has::` probes and a version floor
  keep that honest.
