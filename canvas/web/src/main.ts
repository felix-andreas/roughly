/**
 * The canvas application: input, the draw loop, and the level-of-detail
 * ladder that turns a project into something you can read at any distance.
 *
 * Four bands, crossfaded rather than switched, so nothing ever pops:
 *
 *   whole project   file frames and their names; definitions are colored blocks
 *   file            cards with names and signatures; bodies are per-line bars
 *   definition      bars resolve per token, so the shape of the code shows
 *   code            the DOM overlay takes over with real, selectable text
 */

import "./styles.css";
import { analyze, type Index, type Item, type Project } from "./analysis";
import { Camera, overlaps, ramp, type Rect } from "./camera";
import {
  Expansions,
  FONT_SIZE,
  LINE_HEIGHT,
  canvasWidthFor,
  layout as buildLayout,
  planFrames,
  pick,
  rowIndent,
  setCharWidth,
  type Card,
  type FramePlan,
  type Layout,
} from "./layout";
import { Overlay, TEXT_FADE_FULL, TEXT_FADE_IN } from "./overlay";
import { Renderer } from "./renderer";
import { theme, withAlpha } from "./theme";

/** Per-token bars below this zoom collapse to one bar per line. */
const TOKEN_BARS = 0.15;
/** Below this, cards stop showing structure and become solid blocks. */
const LINE_BARS = 0.045;
const CARD_LABELS = 0.085;

function element(id: string): HTMLElement {
  const found = document.getElementById(id);
  if (!found) throw new Error(`the page is missing #${id}`);
  return found;
}

const stage = element("stage");
const splash = element("splash");
const splashDetail = element("splash-detail");

const canvas = document.createElement("canvas");
stage.append(canvas);

const renderer = new Renderer(canvas);
setCharWidth(renderer.atlas.advanceRatio * FONT_SIZE);
const overlay = new Overlay(stage);
const camera = new Camera();
const expansions = new Expansions();

let index: Index | null = null;
let layout: Layout | null = null;
let plans: FramePlan[] = [];
let canvasWidth = 1;
let revision = 0;
let hovered: number | null = null;
let selected: number | null = null;
let dirty = true;

// ---- boot ----

void boot();

async function boot(): Promise<void> {
  try {
    splashDetail.textContent = "loading the demo project…";
    const project = await loadDemo();
    await open(project, "stringr");
  } catch (error) {
    splashDetail.className = "detail bad";
    splashDetail.textContent = `${error instanceof Error ? error.message : String(error)}`;
    return;
  }
  splash.classList.add("gone");
}

async function loadDemo(): Promise<Project> {
  const manifest = (await (await fetch("demo/manifest.json")).json()) as {
    files: string[];
    description: string | null;
    namespace: string | null;
  };
  const files = await Promise.all(
    manifest.files.map(async (path) => ({
      path,
      text: await (await fetch(`demo/${path}`)).text(),
    })),
  );
  return { files, description: manifest.description, namespace: manifest.namespace };
}

async function open(project: Project, name: string): Promise<void> {
  splashDetail.textContent = `analyzing ${project.files.length} files…`;
  const started = performance.now();
  index = await analyze(project);
  const elapsed = Math.round(performance.now() - started);

  expansions.collapse();
  revision += 1;
  selected = null;
  hovered = null;
  overlay.clear();

  resize();
  plans = planFrames(index);
  canvasWidth = canvasWidthFor(plans, camera.viewportWidth / camera.viewportHeight);
  layout = buildLayout(index, plans, expansions, canvasWidth);
  camera.flyTo(layout.bounds, 64, 0.9);
  camera.settle();

  const label = document.getElementById("project");
  if (label) {
    label.textContent =
      `${name} · ${index.files.length} files · ${index.items.length} definitions · ` +
      `${index.edges.length} references · analyzed in ${elapsed} ms`;
  }
  dirty = true;
}

function relayout(): void {
  if (!index) return;
  layout = buildLayout(index, plans, expansions, canvasWidth);
  revision += 1;
  dirty = true;
}

// ---- draw ----

function resize(): void {
  const ratio = Math.min(window.devicePixelRatio || 1, 2);
  const width = stage.clientWidth;
  const height = stage.clientHeight;
  canvas.width = Math.round(width * ratio);
  canvas.height = Math.round(height * ratio);
  canvas.style.width = `${width}px`;
  canvas.style.height = `${height}px`;
  camera.viewportWidth = width;
  camera.viewportHeight = height;
  dirty = true;
}

let lastFrame = performance.now();
function frame(now: number): void {
  const seconds = Math.min((now - lastFrame) / 1000, 0.05);
  lastFrame = now;

  const settling = camera.settling();
  if (settling) camera.advance(seconds);
  if (settling || dirty) draw();
  dirty = false;
  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);

function draw(): void {
  if (!index || !layout) return;
  const scale = camera.scale;
  const view = camera.visible(0);

  const textIn = ramp(scale, TEXT_FADE_IN, TEXT_FADE_FULL);
  const structure = 1 - textIn;
  const tokenDetail = ramp(scale, LINE_BARS * 1.4, TOKEN_BARS);
  const bodyVisible = ramp(scale, LINE_BARS * 0.6, LINE_BARS * 1.6);
  const labels = ramp(scale, CARD_LABELS, CARD_LABELS * 2.1);
  const ambientEdges = (1 - ramp(scale, 0.09, 0.4)) * 0.1;
  const activeEdges = 1 - ramp(scale, 0.35, 0.8);

  renderer.begin(theme.background);

  const highlight = hovered ?? selected;
  const related = new Set<number>();
  if (highlight !== null) {
    related.add(highlight);
    for (const other of index.items[highlight]?.outgoing ?? []) related.add(other);
    for (const other of index.items[highlight]?.incoming ?? []) related.add(other);
  }

  // Reference curves. The ambient layer answers "how is this wired" from far
  // away; up close it would be noise, so it fades out and only the curves
  // touching what you are pointing at remain.
  if (ambientEdges > 0.01) {
    for (const [from, to] of index.edges) {
      if (related.has(from) || related.has(to)) continue;
      const source = layout.cards.get(from);
      const target = layout.cards.get(to);
      if (!source || !target) continue;
      if (!overlaps(view, spanOf(source, target))) continue;
      const [fromX, toX] = anchors(source, target);
      renderer.curve(
        fromX, source.y + 13, toX, target.y + 13,
        withAlpha(theme.edge, ambientEdges), 1.0, scale, false,
      );
    }
  }

  for (const frame of layout.frames) {
    if (!overlaps(view, frame)) continue;
    const file = index.files[frame.file];
    if (!file) continue;

    renderer.rect(frame.x, frame.y, frame.width, frame.height, theme.frame, 12, 1, theme.frameBorder);

    // Frame names hold a constant on-screen size until they would outgrow the
    // frame, so the map still reads as a labelled map when fully zoomed out.
    const room = frame.width - 28;
    const nameSize = Math.min(
      Math.max(13 / scale, 13),
      frame.width * 0.1,
      room / Math.max(file.name.length * renderer.atlas.advanceRatio, 1),
    );
    renderer.text(
      frame.x + 14, frame.y + 12 + nameSize * 0.78,
      file.name, nameSize, withAlpha(theme.text, 0.92), room,
    );


    for (const card of frame.cards) {
      if (!overlaps(view, card)) continue;
      drawCard(index, card, {
        structure, tokenDetail, bodyVisible, labels,
        emphasis: highlight === card.item ? 1 : related.has(card.item) ? 0.5 : 0,
      });
    }
  }

  if (highlight !== null && activeEdges > 0.01) {
    for (const [from, to] of index.edges) {
      if (from !== highlight && to !== highlight) continue;
      const source = layout.cards.get(from);
      const target = layout.cards.get(to);
      if (!source || !target) continue;
      const [fromX, toX] = anchors(source, target);
      renderer.curve(
        fromX, source.y + 13, toX, target.y + 13,
        withAlpha(from === highlight ? theme.edgeActive : theme.edgeIncoming, 0.75 * activeEdges),
        1.4, scale, true,
      );
    }
  }

  renderer.end(camera);

  const readable: number[] = [];
  if (textIn > 0) {
    for (const frame of layout.frames) {
      if (!overlaps(view, frame)) continue;
      for (const card of frame.cards) {
        if (overlaps(view, card)) readable.push(card.item);
      }
    }
  }
  overlay.update(index, layout, camera, revision, readable, !dragging);

  const zoom = document.getElementById("zoom");
  if (zoom) zoom.textContent = `${Math.round(scale * 100)}%`;
}

interface Bands {
  structure: number;
  tokenDetail: number;
  bodyVisible: number;
  labels: number;
  emphasis: number;
}

function drawCard(index: Index, card: Card, bands: Bands): void {
  const item = index.items[card.item];
  if (!item) return;

  const kind = theme.kinds[item.kindIndex] ?? theme.muted;
  const border =
    bands.emphasis > 0.75
      ? theme.accent
      : bands.emphasis > 0
        ? withAlpha(theme.accent, 0.45)
        : theme.cardBorder;
  const fill = bands.emphasis > 0.75 ? theme.cardActive : theme.card;

  // Fully zoomed out a card carries no readable content, so it becomes the one
  // thing still worth seeing at that distance: a block of its kind's color.
  if (bands.bodyVisible < 1) {
    renderer.rect(
      card.x, card.y, card.width, card.height,
      withAlpha(kind, 0.5 * (1 - bands.bodyVisible) + 0.06), 5,
    );
  }
  renderer.rect(
    card.x, card.y, card.width, card.height,
    withAlpha(fill, bands.bodyVisible), 7, 1,
    withAlpha(border, Math.max(bands.bodyVisible, bands.emphasis)),
  );
  renderer.rect(card.x, card.y + 6, 2.5, 13, withAlpha(kind, bands.bodyVisible), 1.25);

  if (bands.labels > 0.01 && bands.structure > 0.01) {
    const alpha = bands.labels * bands.structure;
    const nameWidth = item.name.length * renderer.atlas.advanceRatio * 12.5;
    renderer.text(
      card.x + 11, card.y + 17,
      item.name, 12.5, withAlpha(theme.text, alpha), card.width - 22,
    );
    if (item.signature) {
      renderer.text(
        card.x + 11 + nameWidth + 7, card.y + 17,
        item.signature, 11, withAlpha(theme.faint, alpha), card.width - nameWidth - 30,
      );
    }
  }

  if (item.errors > 0 || item.warnings > 0) {
    renderer.rect(
      card.x + card.width - 13, card.y + 9, 6, 6,
      withAlpha(item.errors > 0 ? theme.error : theme.warning, Math.max(bands.bodyVisible, 0.7)),
      3,
    );
  }

  if (bands.bodyVisible < 0.02) return;

  const top = card.y + card.contentTop;
  const barAlpha = bands.structure * bands.bodyVisible;
  const height = LINE_HEIGHT * 0.46;

  for (const row of card.rows) {
    const y = top + row.y;
    if (row.kind === "blank") continue;

    if (row.kind === "head") {
      // The transcluded definition gets its own backing and rule so the seam
      // between "the code you opened" and "the code it called" stays obvious.
      // Drawn at every zoom, unlike the bars: this is structure, and it is what
      // stops a deep expansion from reading as one flat run of code.
      const gutter = card.x + rowIndent(row.depth) - 8;
      renderer.rect(
        gutter, y, card.width - rowIndent(row.depth), row.extent,
        withAlpha(theme.accent, 0.055 * bands.bodyVisible), 5,
      );
      renderer.rect(
        gutter, y, 1.5, row.extent,
        withAlpha(theme.accent, 0.5 * bands.bodyVisible),
      );
      const callee = index.items[row.item];
      if (callee && bands.labels > 0.01 && bands.structure > 0.01) {
        renderer.text(
          card.x + rowIndent(row.depth) + 3, y + 15,
          callee.name, 11.5,
          withAlpha(theme.accent, bands.labels * bands.structure),
          card.width - rowIndent(row.depth) - 14,
        );
      }
      continue;
    }

    if (bands.structure < 0.02) continue;
    const line = index.items[row.item]?.lines[row.line];
    if (!line || line.text.length === 0) continue;
    const left = card.x + rowIndent(row.depth);
    const charWidth = renderer.atlas.advanceRatio * FONT_SIZE;
    const barY = y + LINE_HEIGHT * 0.27;

    if (bands.tokenDetail > 0.02) {
      for (const token of line.tokens) {
        const color = theme.tokens[token.class] ?? theme.text;
        renderer.rect(
          left + token.start * charWidth, barY,
          (token.end - token.start) * charWidth, height,
          withAlpha(color, 0.55 * barAlpha * bands.tokenDetail), 1.5,
        );
      }
    }
    if (bands.tokenDetail < 0.98) {
      renderer.rect(
        left + line.indent * charWidth, barY,
        (line.text.length - line.indent) * charWidth, height,
        withAlpha(theme.faint, 0.4 * barAlpha * (1 - bands.tokenDetail)), 1.5,
      );
    }
    // Expandable names stay visible as accent marks even when the code itself
    // is far too small to read — that is the affordance the whole idea rests on.
    for (const reference of line.references) {
      renderer.rect(
        left + reference.start * charWidth, barY,
        (reference.end - reference.start) * charWidth, height,
        withAlpha(theme.accent, 0.5 * barAlpha), 1.5,
      );
    }
  }
}

/** Exit and entry x for a reference curve, on whichever sides face each other. */
const anchors = (source: Card, target: Card): [number, number] =>
  target.x + target.width / 2 >= source.x + source.width / 2
    ? [source.x + source.width, target.x]
    : [source.x, target.x + target.width];

const spanOf = (a: Card, b: Card): Rect => ({
  x: Math.min(a.x, b.x),
  y: Math.min(a.y, b.y),
  width: Math.abs(b.x - a.x) + Math.max(a.width, b.width),
  height: Math.abs(b.y - a.y) + Math.max(a.height, b.height),
});

// ---- input ----

let dragging = false;
let moved = 0;
let spaceHeld = false;

stage.addEventListener("pointerdown", (event) => {
  const onText =
    event.target instanceof Element && event.target.closest(".card-text") !== null;
  // While the overlay is showing real text, dragging over it selects rather
  // than pans — the same bargain every editor makes.
  if (onText && !spaceHeld && event.button === 0) return;
  dragging = true;
  moved = 0;
  stage.setPointerCapture(event.pointerId);
});

stage.addEventListener("pointermove", (event) => {
  if (dragging) {
    camera.panBy(event.movementX, event.movementY);
    moved += Math.abs(event.movementX) + Math.abs(event.movementY);
    dirty = true;
    return;
  }
  if (!layout || !index) return;
  const world = camera.screenToWorld(event.clientX, event.clientY);
  const hit = pick(layout, index, world.x, world.y);
  const next = hit?.card?.item ?? null;
  if (next !== hovered) {
    hovered = next;
    dirty = true;
  }
  canvas.style.cursor = hit?.reference ? "pointer" : dragging ? "grabbing" : "default";
});

stage.addEventListener("pointerup", (event) => {
  const wasDragging = dragging;
  dragging = false;
  if (stage.hasPointerCapture(event.pointerId)) stage.releasePointerCapture(event.pointerId);
  dirty = true;
  if (wasDragging && moved > 5) return;
  if (!layout || !index) return;

  const world = camera.screenToWorld(event.clientX, event.clientY);
  const hit = pick(layout, index, world.x, world.y);
  if (!hit) {
    selected = null;
    renderInspector(null);
    return;
  }
  if (hit.reference) {
    if (event.shiftKey) {
      flyToItem(hit.reference.target);
    } else {
      expansions.toggle(hit.card!.item, hit.reference.path, hit.reference.target);
      relayout();
    }
    return;
  }
  if (hit.row?.kind === "head" && hit.card) {
    expansions.toggle(hit.card.item, hit.row.path, hit.row.item);
    relayout();
    return;
  }
  if (hit.card) {
    selected = hit.card.item;
    renderInspector(index.items[hit.card.item] ?? null);
    dirty = true;
  }
});

stage.addEventListener("dblclick", (event) => {
  if (!layout || !index) return;
  const world = camera.screenToWorld(event.clientX, event.clientY);
  const hit = pick(layout, index, world.x, world.y);
  if (hit?.card) flyToItem(hit.card.item);
  else if (layout) camera.flyTo(layout.bounds, 90, 0.9);
});

stage.addEventListener(
  "wheel",
  (event) => {
    event.preventDefault();
    if (event.ctrlKey || event.metaKey) {
      camera.zoomAt(event.clientX, event.clientY, Math.exp(-event.deltaY * 0.01));
    } else {
      camera.panBy(-event.deltaX, -event.deltaY);
    }
    dirty = true;
  },
  { passive: false },
);

window.addEventListener("resize", () => {
  resize();
  if (index) {
    canvasWidth = canvasWidthFor(plans, camera.viewportWidth / camera.viewportHeight);
    relayout();
  }
});

window.addEventListener("keydown", (event) => {
  if (event.key === " ") spaceHeld = true;
  if (event.target instanceof HTMLInputElement) return;

  if (event.key === "/" || (event.key === "k" && (event.metaKey || event.ctrlKey))) {
    event.preventDefault();
    openPalette();
  } else if (event.key === "Escape") {
    if (paletteOpen()) closePalette();
    else {
      expansions.collapse();
      selected = null;
      renderInspector(null);
      relayout();
    }
  } else if (event.key === "0" && layout) {
    camera.flyTo(layout.bounds, 90, 0.9);
    dirty = true;
  } else if (event.key === "+" || event.key === "=") {
    camera.zoomAt(camera.viewportWidth / 2, camera.viewportHeight / 2, 1.4);
    dirty = true;
  } else if (event.key === "-") {
    camera.zoomAt(camera.viewportWidth / 2, camera.viewportHeight / 2, 1 / 1.4);
    dirty = true;
  }
});
window.addEventListener("keyup", (event) => {
  if (event.key === " ") spaceHeld = false;
});

function flyToItem(item: number): void {
  const card = layout?.cards.get(item);
  if (!card) return;
  camera.flyTo(card, 80, 1.1);
  selected = item;
  renderInspector(index?.items[item] ?? null);
  dirty = true;
}

// ---- chrome ----

function renderInspector(item: Item | null): void {
  const panel = document.getElementById("inspector");
  if (!panel) return;
  if (!item || !index) {
    panel.classList.remove("open");
    return;
  }
  const file = index.files[item.file];
  panel.classList.add("open");
  panel.innerHTML =
    `<h2>${text(item.name)}</h2>` +
    (item.signature ? `<div class="sig">${text(item.signature)}</div>` : "") +
    (item.type ? `<div class="type">${text(item.type)}</div>` : "") +
    (item.doc ? `<div class="doc">${text(item.doc)}</div>` : "") +
    `<dl>` +
    `<dt>kind</dt><dd>${text(item.kind)}</dd>` +
    `<dt>defined</dt><dd>${text(file?.path ?? "")}:${item.line + 1}</dd>` +
    `<dt>lines</dt><dd>${item.lines.length}</dd>` +
    `<dt>references out</dt><dd>${item.outgoing.length}</dd>` +
    `<dt>referenced by</dt><dd>${item.incoming.length}</dd>` +
    (item.errors + item.warnings > 0
      ? `<dt>findings</dt><dd>` +
        (item.errors ? `<span class="bad">${item.errors} error${item.errors > 1 ? "s" : ""}</span> ` : "") +
        (item.warnings ? `<span class="warn">${item.warnings} warning${item.warnings > 1 ? "s" : ""}</span>` : "") +
        `</dd>`
      : "") +
    `</dl>`;
}

const paletteElement = document.getElementById("palette");
const paletteInput = document.getElementById("palette-input") as HTMLInputElement | null;
const paletteList = document.getElementById("palette-list");
let paletteMatches: number[] = [];
let paletteCursor = 0;

const paletteOpen = (): boolean => paletteElement?.classList.contains("open") ?? false;

function openPalette(): void {
  paletteElement?.classList.add("open");
  if (paletteInput) {
    paletteInput.value = "";
    paletteInput.focus();
  }
  updatePalette("");
}

function closePalette(): void {
  paletteElement?.classList.remove("open");
  paletteInput?.blur();
}

function updatePalette(query: string): void {
  if (!index || !paletteList) return;
  const needle = query.toLowerCase();
  paletteMatches = index.items
    .map((item, id) => ({ item, id }))
    .filter(({ item }) => item.name.toLowerCase().includes(needle))
    .sort((a, b) => {
      const aStarts = a.item.name.toLowerCase().startsWith(needle) ? 0 : 1;
      const bStarts = b.item.name.toLowerCase().startsWith(needle) ? 0 : 1;
      return aStarts - bStarts || a.item.name.length - b.item.name.length;
    })
    .slice(0, 60)
    .map(({ id }) => id);
  paletteCursor = 0;
  paletteList.innerHTML = paletteMatches
    .map((id, position) => {
      const item = index!.items[id]!;
      const file = index!.files[item.file];
      return (
        `<li class="${position === 0 ? "on" : ""}" data-item="${id}">` +
        `<span class="name">${text(item.name)}</span>` +
        `<span class="where">${text(file?.name ?? "")}:${item.line + 1}</span>` +
        `</li>`
      );
    })
    .join("");
}

paletteInput?.addEventListener("input", () => updatePalette(paletteInput.value));
paletteInput?.addEventListener("keydown", (event) => {
  if (event.key === "Escape") return closePalette();
  if (event.key === "Enter") {
    const item = paletteMatches[paletteCursor];
    if (item !== undefined) flyToItem(item);
    closePalette();
    return;
  }
  if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
  event.preventDefault();
  paletteCursor = Math.max(
    0,
    Math.min(paletteMatches.length - 1, paletteCursor + (event.key === "ArrowDown" ? 1 : -1)),
  );
  paletteList?.querySelectorAll("li").forEach((element, position) => {
    element.classList.toggle("on", position === paletteCursor);
  });
  paletteList?.querySelectorAll("li")[paletteCursor]?.scrollIntoView({ block: "nearest" });
});
paletteList?.addEventListener("click", (event) => {
  const row = event.target instanceof Element ? event.target.closest("li") : null;
  const item = row?.getAttribute("data-item");
  if (item !== null && item !== undefined) flyToItem(Number(item));
  closePalette();
});

document.getElementById("search")?.addEventListener("click", openPalette);
document.getElementById("collapse")?.addEventListener("click", () => {
  expansions.collapse();
  relayout();
});
document.getElementById("open")?.addEventListener("click", () => void openFolder());

/**
 * Load a project from disk. The directory picker is the good path; browsers
 * without it fall back to a directory-mode file input, which asks the same
 * question with a worse dialog.
 */
async function openFolder(): Promise<void> {
  const picker = (window as unknown as { showDirectoryPicker?: () => Promise<FileSystemDirectoryHandle> })
    .showDirectoryPicker;
  if (!picker) {
    const input = document.createElement("input");
    input.type = "file";
    input.setAttribute("webkitdirectory", "");
    input.addEventListener("change", () => {
      void openFiles([...(input.files ?? [])].map((file) => [file.webkitRelativePath, file]));
    });
    input.click();
    return;
  }
  const handle = await picker.call(window);
  const collected: [string, File][] = [];
  await collect(handle, "", collected);
  await openFiles(collected);
}

async function collect(
  directory: FileSystemDirectoryHandle,
  prefix: string,
  into: [string, File][],
): Promise<void> {
  for await (const [name, entry] of directory as unknown as AsyncIterable<
    [string, FileSystemHandle]
  >) {
    const path = prefix ? `${prefix}/${name}` : name;
    if (entry.kind === "file") {
      into.push([path, await (entry as FileSystemFileHandle).getFile()]);
    } else if (name !== ".git" && name !== "node_modules") {
      await collect(entry as FileSystemDirectoryHandle, path, into);
    }
  }
}

async function openFiles(entries: [string, File][]): Promise<void> {
  splash.classList.remove("gone");
  splashDetail.className = "detail";
  const sources = entries.filter(([path]) => /\.[Rr]$/.test(path));
  if (sources.length === 0) {
    splashDetail.className = "detail bad";
    splashDetail.textContent = "that folder holds no .R files";
    return;
  }
  const strip = commonPrefix(sources.map(([path]) => path));
  const files = await Promise.all(
    sources.map(async ([path, file]) => ({ path: path.slice(strip), text: await file.text() })),
  );
  const metadata = async (name: string): Promise<string | null> => {
    const found = entries.find(([path]) => path.slice(strip) === name);
    return found ? await found[1].text() : null;
  };
  await open(
    { files, description: await metadata("DESCRIPTION"), namespace: await metadata("NAMESPACE") },
    entries[0]?.[0].split("/")[0] ?? "project",
  );
  splash.classList.add("gone");
}

/** The shared leading directory, so frames are labelled `R/utils.R` not the absolute path. */
function commonPrefix(paths: string[]): number {
  const first = paths[0];
  if (!first || paths.length === 0) return 0;
  const segments = first.split("/").slice(0, -1);
  let shared = segments.length;
  for (const path of paths) {
    const parts = path.split("/").slice(0, -1);
    let index = 0;
    while (index < shared && index < parts.length && parts[index] === segments[index]) index += 1;
    shared = index;
  }
  // Keep the last shared segment when it is the package's `R` directory: the
  // file name alone loses the fact that these are package sources.
  const keep = Math.max(0, shared - 1);
  return keep === 0 ? 0 : segments.slice(0, keep).join("/").length + 1;
}

const text = (value: string): string =>
  value.replace(/[&<>]/g, (character) =>
    character === "&" ? "&amp;" : character === "<" ? "&lt;" : "&gt;",
  );

