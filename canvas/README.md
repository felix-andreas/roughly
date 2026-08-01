# ry canvas

A zoomable map of an R codebase, where following a reference means **opening it
where it is** rather than jumping somewhere else.

The whole project lives on one infinite surface. Zoom out and you see its
architecture; zoom in and definitions resolve through structure into real,
selectable source. Click any resolved name and its definition is spliced into
the body you are reading, indented and framed, recursively. The camera never
moves, so you never lose your place.

It is a standalone project that consumes ry as a library — `syntax` for the
parse, `semantics` for name resolution and type inference, `ide` for the
outline. Nothing about R is reimplemented here: every name the canvas will open
is a name ry resolved, and every type on a card is a type ry inferred.

## Running it

```
cd web
bun install
bun run core     # builds canvas-core to wasm32 and stages it in public/
bun run dev
```

`bun run core` needs the wasm target once: `rustup target add wasm32-unknown-unknown`.

It opens on a bundled copy of [stringr](https://stringr.tidyverse.org/) (see
`web/public/demo/NOTICE.md`). **Open folder…** analyzes any R package or script
directory from disk; nothing leaves the browser.

## How it is put together

```
core/    Rust → wasm32. Depends on ry's syntax + semantics + ide crates.
         One call in, one call out: sources as JSON, the canvas index as JSON.
web/     TypeScript. WebGL2 for structure, DOM for readable text.
```

**Why a raw C ABI instead of wasm-bindgen.** The surface is a single string in,
string out. A generated binding layer would add a build step and a toolchain to
install in exchange for nothing.

**Why WebGL2 with a DOM overlay, and not wgpu or three.js.** The graphics API
was never the bottleneck — text is. This workload is a few hundred thousand
quads, which WebGL2 draws in two instanced calls; WebGPU's advantages appear
when you are CPU-bound submitting many distinct materials, and it still has
gaps (Firefox on Linux) among exactly the developers who would use this. Going
to wgpu would mean owning glyph atlasing, shaping, font fallback, text
selection and hit testing forever, and for a read-only explorer that list *is*
the product. three.js is a 3D scene graph; this is a 2D zoomable UI, and its
text would be worse. Handing the last zoom decade to the DOM buys selectable
text, browser find, font fallback and screen-reader access for free.

A future native build via wgpu is the natural next step, and the analysis core
ports unchanged.

## The zoom ladder

Four bands, crossfaded rather than switched, so nothing pops:

| Band | What a definition looks like |
| --- | --- |
| whole project | a block in its kind's color; file frames carry the names |
| file | a card with its name and signature; the body is one bar per line |
| definition | bars resolve per token, so the shape of the code shows through |
| code | the DOM overlay takes over with real, selectable, highlighted source |

Expandable references stay visible as accent marks in the bar bands, well below
the zoom where the code itself can be read — the affordance the whole idea rests
on has to survive the zoom-out.

## Layout rules

Two constraints, both non-negotiable, because breaking either destroys the only
thing a canvas offers over a file tree:

- **Positions are deterministic.** The same project lays out the same way every
  time. Sub-column assignment inside a frame and row membership in the shelf
  packing are computed from *collapsed* sizes and then held fixed.
- **Expansion is local.** Opening a definition grows its card and pushes what
  is below it down. It never moves anything sideways, and never reflows another
  column.

Large files flow into up to four sub-columns rather than becoming a single
900-line ribbon, which would otherwise set the height of the entire canvas.

## What the core extracts

Per definition: its source text, classified tokens, its inferred type as hover
renders it, the leading roxygen line, diagnostics counts, and — the interesting
part — every reference that resolves to another project definition, as a span
plus a target. That last one comes from `item_naming(…).non_locals` resolved
through `package_definitions`, which is the same name resolution the language
server answers goto-definition with.

Reference edges between definitions come from `item_interface_reads`, a
projection ry already maintains for its incremental firewall.

## Status

An MVP, and read-only by design: it explores, it does not edit. Known gaps:

- The whole index is built in one pass and shipped as one payload. Fine into the
  low tens of thousands of lines; a large project wants incremental, viewport-driven
  loading.
- Only top-level definitions are cards. Nested definitions inside function
  bodies and class constructors are not addressable yet.
- No dependency-ordered layout mode. The call graph is drawn over a
  file-grouped map rather than being able to drive one.
- Reference curves are aggregate hairballs at the overview on a large project;
  they want bundling.
