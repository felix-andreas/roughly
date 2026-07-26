# Work log

One line per autonomous work cycle, newest last. **This is the one file in
`.agents/memory/` that is deliberately chronological** — the timeless rule that
governs `MEMORY.md`, `backlog.md` and `decisions.md` does not apply here, so do
not "fix" it into a knowledge document. Durable facts still belong in those files;
this only records what happened, so a later session can see the shape of recent
work without re-reading the git log.

Format: `YYYY-MM-DD HH:MM — what landed. Loose end, if any.`

Keep entries to one line. A cycle that found nothing worth doing says so; an empty
line is more useful than invented work.

---

- 2026-07-26 22:05 — Overload selection reduced to plain first-match (removed the last-fitting tiebreak, which conflicted with the value-use rule); `lapply`/`Filter`/`rev`/`unique`/`head`/`tail` now preserve input shape and names; all-fail diagnostics report the deepest candidate instead of the wrapper; signature help no longer needs a committed candidate. Recorded the HM-only bar and declined traits. Rewrote the inline-typing proposal three times against two adversarial reviews, which caught a contract reversal and three false motivating claims. Naming decided: `.ry` in `Ry/`, `base.ry.stub`, CLI rename to `ry` recommended but unsettled. Loose end: the silently-dropped annotation in a non-attachable position is filed but unfixed.
- 2026-07-26 23:10 — Renamed the language and toolchain to `ry`: crate `crates/ry` published as `ry-lang`, binary and lib `ry`, docs/editors/CI/README swept, docs pointed at ry-lang.org, README carries the shorter-name joke with verified parser output. Former spellings all still work — `roughly.toml`, `# roughly: allow(...)`, `ROUGHLY_*`, the extension's `roughly.*` settings — and the REPL history directory migrates itself. Loose ends for the user: repository rename, Marketplace identifier, crates.io registration, and the hero animation, which still spells the old name and is user-owned.
