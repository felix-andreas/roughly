---
title: Continuous integration
description: A working CI job for R, and how to decide what should fail the build
---

Roughly is one binary with no R dependency, so a CI job is a download and two commands. Nothing to
install, no package cache, no matrix over R versions.

## A working job

```yaml
# .github/workflows/roughly.yml
name: roughly
on: [push, pull_request]

jobs:
  roughly:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install Roughly
        env:
          ROUGHLY_VERSION: 0.3.0-alpha
        run: |
          curl -sSL "https://github.com/felix-andreas/roughly/releases/download/${ROUGHLY_VERSION}/roughly-x86_64-unknown-linux-gnu.tar.gz" \
            | tar xz
          sudo mv roughly /usr/local/bin/
      - name: Check
        run: roughly check
      - name: Format
        run: roughly fmt --check
```

Both commands exit `1` on findings, which is what fails the job. Neither needs R installed.

**Pin the version.** Every release so far is marked a pre-release, so
`releases/latest/download/…` resolves to an old stable tag rather than the newest build — and the type
system is still gaining capability, so a newer version can report findings an older one did not. Name
the tag explicitly, as above. See [project status](/why-roughly#project-status).

Asset names follow the Rust target triple — `roughly-aarch64-apple-darwin.tar.gz`,
`roughly-x86_64-pc-windows-gnu.zip` — and each archive contains the single `roughly` binary.

## Deciding what should fail the build

The default is strict: **warnings fail the job**. A run with nothing but `unused` warnings still
exits `1`.

That is usually right for a project that starts clean, and wrong for one adopting Roughly on an
existing codebase. To gate on errors only while you work through a backlog:

```bash
roughly check --min-severity error
```

The filter applies before the exit code is decided, so warnings still print but no longer fail
anything.

| You want | Command |
| --- | --- |
| Everything to matter | `roughly check` |
| Only errors to block the build | `roughly check --min-severity error` |
| Formatting enforced | `roughly fmt --check` |
| To see the diff CI would apply | `roughly fmt --diff` |

Be careful reading exit code `2` as "worse than 1" — it is a *different* failure. It means the run
could not be completed: an unparseable `roughly.toml`, a path that does not exist, an unreadable file.
A job that treats any non-zero as "findings" will report a broken config as a code problem. The full
table is in the [CLI reference](/reference/cli#exit-codes).

## JSON output

```bash
roughly check --output json
```

writes JSON Lines to stdout — one object per finding, nothing else on the stream, and no summary
line. In JSON mode stderr stays empty, so you can pipe stdout straight into a tool without filtering
anything out.

```json
{"code":"type-mismatch","column":21,"endColumn":27,"endLine":4,"line":4,"message":"expected `integer`, found `character`","path":"/home/you/demo/main.R","related":[],"severity":"error"}
```

Every field is documented in the [CLI reference](/reference/cli#json-output), and the field names are a
contract — they are covered by tests that fail when they change, so a script built on them will not
break silently.

Counting errors for a summary line, without jq:

```bash
roughly check --output json | grep -c '"severity":"error"'
```

## Adopting on an existing project

Do not start by putting `roughly check` in front of a merge gate on a codebase that has never run it.
Land the tool first, gate second. [Adopting an existing codebase](/guides/adopting) walks through the
order that works.
