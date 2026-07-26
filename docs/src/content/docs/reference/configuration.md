---
title: Configuration
description: Every roughly.toml key, discovery rule, and editor setting in one place
---

Everything you can change about Roughly's behavior lives in one file, `roughly.toml`; editor settings only say where the binary is.

## Project discovery

`roughly.toml` is the only configuration file. There is no home-directory config, no environment variable naming one, and no merging — the nearest file replaces the built-in defaults wholesale.

| Where Roughly runs | Search starts at |
| --- | --- |
| `roughly check R/utils.R` | the file's own directory |
| `roughly check .` | that directory |
| The language server | the workspace folder your editor announces; failing that, the process working directory |

| Rule | Behavior |
| --- | --- |
| Search | walk up from the starting directory; the first `roughly.toml` wins. None found: built-in defaults. |
| Merging | none — one file supplies every key. |
| Reload | the language server watches `roughly.toml` and re-discovers on every change, so deleting it falls back to an ancestor or to the defaults. |
| Several CLI targets | discovery runs once per argument, so two arguments can resolve two different files. |
| `..` in a path | cancelled textually before the search, so `project/roughly.toml` does **not** govern `project/../outside.R`. |

```console
$ cat project/roughly.toml
spaces = 8
$ roughly fmt --diff project/inside.R
Diff in project/inside.R:
1   1    | f <- function(x) {
2        |-  x
    2    |+        x
3   3    | }
1 file would be reformatted, 0 files already formatted
$ roughly fmt --diff project/../outside.R
0 files would be reformatted, 1 file already formatted
```

### Project root

The project root is a separate decision: it sets the analysis scope — which files see each other's definitions — not which config is loaded.

| Situation | Root |
| --- | --- |
| An ancestor holds `roughly.toml` or `DESCRIPTION` | the nearest such directory |
| Otherwise, the target is a directory | that directory |
| Otherwise, the target sits directly under an `R/` directory | the parent of `R/` |
| Otherwise | the file's own directory |

## `[format]`

| Key | Type | Default | Effect |
| --- | --- | --- | --- |
| `indent-width` | integer | `2` | Spaces per indentation level, for `roughly fmt` and for formatting in the editor. |
| `line-ending` | `"auto"`, `"lf"`, `"cr-lf"` | `"auto"` | Line ending the formatter writes. `"auto"` keeps whatever the file already uses. |

## `[lint]`

Every key except `naming-style` takes a level: `"off"`, `"warn"`, `"error"`, or `"default"` — which means the built-in severity, exactly as if you omitted the key.

| Key | Type | Default | Effect |
| --- | --- | --- | --- |
| `naming-style` | `"snake_case"`, `"camelCase"` | unset — check off | Reports `naming-style` for variables and function parameters that do not match. `SCREAMING_SNAKE_CASE` always conforms. Always a warning; the value is a style, not a level. |
| `assignment-operator` | level | `"warn"` | `=` used for assignment. |
| `boolean-shorthand` | level | `"warn"` | `T` or `F` written instead of `TRUE` or `FALSE`. |
| `trailing-comma` | level | `"error"` | A comma after the last argument of a call. |
| `unused-parameter` | level | `"off"` | Function formals never read. S3 methods and your project's own generics are exempt. |
| `unused-import` | level | `"off"` | An `importFrom(pkg, name)` in `NAMESPACE` whose name appears nowhere in your sources. Whole-namespace `import(pkg)` is never checked, and this finding is raised by `roughly check` only — not in the editor. |
| `shadows-builtin` | level | `"off"` | A top-level binding with the same name as a `base` export. |
| `shadows-namespace` | level | `"off"` | A top-level binding with the same name as an export of another namespace, such as `stats::filter`. |

For a single exception, prefer a [suppression comment](/reference/diagnostic-codes#suppressing-a-finding) over turning a lint off across the whole project.

## `[check]`

Type inference always runs — hover, inlay hints, and signature help work regardless of these keys. `[check]` only decides which findings are reported.

| Key | Type | Default | Effect |
| --- | --- | --- | --- |
| `unused` | boolean | `true` | Report `unused` — bindings whose value is never read. |
| `typing` | boolean | `false` | Report `type-mismatch`. See the [tutorial](/type-checking/tutorial). |
| `strict` | boolean | `false` | Report each site with a genuinely undetermined type, **and** raise every `unresolved` finding from warning to error. See [strict mode](/reference/type-system#strict-mode). |
| `exclude` | array of strings | `[]` | Gitignore-style patterns the directory walk of `roughly check` skips. |

A `# typing: off`, `# typing: on`, or `# typing: strict` line at the top of a file replaces both `typing` and `strict` for that file — see [the per-file directive](/reference/type-system#per-file-directive).

Four things worth knowing about `exclude`:

- Patterns are anchored at the directory holding `roughly.toml`, and follow gitignore rules: `scripts/` excludes that whole subtree, `**/generated` matches at any depth, `!` re-includes.
- Excluded directories are pruned without being walked, so exclusion cuts checking time, not just output.
- A file named on the command line is always checked, files open in the editor are always analyzed, and `roughly fmt` ignores the key entirely.
- Some paths are skipped with no configuration at all, because they hold vendored dependencies rather than your code: `renv/`, `packrat/`, `revdep/`, `.Rproj.user/`, `.Rcheck/`. `.gitignore` is honored too, git checkout or not.

```console
$ cat roughly.toml
[check]
exclude = ["scripts/"]
$ roughly check .
warning[unused]: `v` is assigned but never used.
 --> R/a.R:1:19
1 | f <- function() { v <- 1; 2 }
                      ^

1 problem in 1 file
```

## Invalid and unknown keys

An unknown key is never fatal — a config written for a newer Roughly still starts an older one.

| Situation | Result |
| --- | --- |
| Unknown key | Ignored, with one warning naming it. Known keys beside it still load, and the exit code is unaffected. |
| Wrong type on a known key | Hard error. |
| Malformed TOML | Hard error. |
| Invalid `[check] exclude` pattern | Hard error. |
| The file disappears between discovery and reading | Silently falls back to the defaults. |
| Any other read error | Hard error. |

```console
$ cat roughly.toml
strict = true

[check]
stric = true
typing = true

[format]
indent = 4
$ roughly check .
warning: ignoring unknown config key `strict` — check the spelling, or update roughly
warning: ignoring unknown config key `check.stric` — check the spelling, or update roughly
warning: ignoring unknown config key `format.indent` — check the spelling, or update roughly
1 file checked, no problems
```

A hard error names the file, the key, and the exact position:

```console
$ cat roughly.toml
[check]
typing = "yes"
$ roughly check .
error: invalid config in /home/you/project/roughly.toml for `check.typing` at line 2, column 10: invalid type: string "yes", expected a boolean
$ echo $?
2
```

Where that lands depends on how Roughly runs:

| | Behavior |
| --- | --- |
| CLI | The message goes to stderr and the command exits 2 — see the [exit codes](/reference/cli#exit-codes). |
| The language server | Never crashes. At startup it falls back to the defaults; on a live edit it keeps the previous configuration. Either way it shows the message and publishes a `config` finding on `roughly.toml` at the offending line, cleared once the file loads again. |

## Legacy keys

| Old key | Modern key | Note |
| --- | --- | --- |
| `case` (top level) | `lint.naming-style` | Still parses, and wins when both are set. |
| `spaces` (top level) | `format.indent-width` | Still parses, and wins when both are set. |
| `lint.missing-comma` | none | Accepted so old files keep loading, and does nothing: a missing argument comma is now a parse error. |

## Editor settings

These say where the binary is and how to launch it; none of them changes analysis. The language server ignores LSP workspace configuration outright, so every behavioural key must live in `roughly.toml`.

### VS Code

| Setting | Default | Effect |
| --- | --- | --- |
| `roughly.path` | `null` | Location of the `roughly` executable. |
| `roughly.args` | `null`, meaning `["server"]` | Arguments passed to the executable. |
| `roughly.experimentalFeatures` | `null` | Feature names forwarded as `--experimental-features`. Currently only `range_formatting` — format the selected range instead of the whole file. |

Changing any of the three prompts you to restart the server; it takes effect only then. The extension finds the binary in this order: the `SERVER_PATH` environment variable, `roughly.path`, its own bundled copy, then `roughly` on your `PATH`.

| Command | Does |
| --- | --- |
| Roughly: Restart Server | Restarts the language server |
| Roughly: Start Server | Same as restart |
| Roughly: Stop Server | Shuts the language server down |
| Roughly: Open Logs | Opens the server's output channel |

### Zed

The extension contributes no settings of its own, so use Zed's generic `lsp.roughly` block.

| Setting | Default | Effect |
| --- | --- | --- |
| `lsp.roughly.binary.path` | unset | Absolute path to the binary. Setting it skips the `PATH` lookup and the release download. |
| `lsp.roughly.binary.arguments` | unset, meaning `["server", "--stdio"]` | Arguments passed to the binary, however it was found. |
| `lsp.roughly.settings` | unset | Forwarded to the server, which ignores it. Put configuration in `roughly.toml`. |

Zed highlights R with tree-sitter, which sees a `#:` annotation as an ordinary comment. The colors come from the server as semantic tokens, which Zed leaves off by default:

```json
// settings.json
{
  "languages": {
    "R": {
      "semantic_tokens": "combined"
    }
  }
}
```

Without a path or a binary on your `PATH`, the Zed extension downloads a release itself and reuses it afterwards.
