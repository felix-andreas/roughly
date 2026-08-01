/**
 * Real code text, in the DOM, for cards that are close enough to read.
 *
 * The GPU layer draws the shape of code; this draws the code. Handing the last
 * zoom decade to the DOM is what buys selectable text, browser find, font
 * fallback for anything non-ASCII, and screen-reader access — none of which a
 * glyph atlas can offer, and all of which a code reader expects.
 *
 * Cards are laid out in "bucket space": their font size is quantized, and the
 * leftover fraction rides on a CSS transform. Reflow then happens a handful of
 * times across the whole zoom range instead of on every frame, while the
 * residual scale stays close enough to 1 that text never looks soft.
 */

import type { Index } from "./analysis";
import { FONT_SIZE, LINE_HEIGHT, rowIndent, type Card, type Layout } from "./layout";
import type { Camera } from "./camera";

const BUCKET = 0.08;
/** Below this the DOM contributes nothing readable and is switched off. */
export const TEXT_FADE_IN = 0.52;
export const TEXT_FADE_FULL = 0.78;
/** Above this the text is big enough that selecting it is a sensible thing to do. */
const INTERACTIVE_SCALE = 0.95;

export class Overlay {
  readonly root: HTMLDivElement;
  private cards = new Map<number, { element: HTMLElement; revision: number }>();
  private bucket = 0;

  constructor(parent: HTMLElement) {
    this.root = document.createElement("div");
    this.root.className = "overlay";
    parent.append(this.root);
  }

  update(
    index: Index,
    layout: Layout,
    camera: Camera,
    revision: number,
    visible: Iterable<number>,
    interactive: boolean,
  ): void {
    const opacity = ramp(camera.scale, TEXT_FADE_IN, TEXT_FADE_FULL);
    this.root.style.opacity = `${opacity}`;
    if (opacity <= 0) {
      this.root.style.visibility = "hidden";
      return;
    }
    this.root.style.visibility = "visible";
    this.root.style.pointerEvents =
      interactive && camera.scale >= INTERACTIVE_SCALE ? "auto" : "none";

    const bucket = Math.max(BUCKET, Math.round(camera.scale / BUCKET) * BUCKET);
    if (bucket !== this.bucket) {
      this.bucket = bucket;
      this.root.style.fontSize = `${FONT_SIZE * bucket}px`;
      this.root.style.lineHeight = `${LINE_HEIGHT * bucket}px`;
      for (const [item, entry] of this.cards) {
        const card = layout.cards.get(item);
        if (card) position(entry.element, card, bucket);
      }
    }

    const residual = camera.scale / bucket;
    this.root.style.transform =
      `translate(${camera.viewportWidth / 2 - camera.x * camera.scale}px, ` +
      `${camera.viewportHeight / 2 - camera.y * camera.scale}px) scale(${residual})`;

    const wanted = new Set(visible);
    for (const [item, entry] of this.cards) {
      if (!wanted.has(item)) {
        entry.element.remove();
        this.cards.delete(item);
      }
    }
    for (const item of wanted) {
      const card = layout.cards.get(item);
      if (!card) continue;
      const existing = this.cards.get(item);
      if (existing && existing.revision === revision) {
        position(existing.element, card, bucket);
        continue;
      }
      existing?.element.remove();
      const element = renderCard(index, card);
      position(element, card, bucket);
      this.root.append(element);
      this.cards.set(item, { element, revision });
    }
  }

  clear(): void {
    for (const entry of this.cards.values()) entry.element.remove();
    this.cards.clear();
  }
}

function position(element: HTMLElement, card: Card, bucket: number): void {
  element.style.left = `${card.x * bucket}px`;
  element.style.top = `${card.y * bucket}px`;
  element.style.width = `${card.width * bucket}px`;
  element.style.height = `${card.height * bucket}px`;
  element.style.paddingTop = `${card.contentTop * bucket}px`;
}

function renderCard(index: Index, card: Card): HTMLElement {
  const element = document.createElement("div");
  element.className = "card-text";
  const item = index.items[card.item];
  if (!item) return element;

  const pieces: string[] = [];
  for (const row of card.rows) {
    const indent = rowIndent(row.depth);
    if (row.kind === "blank") {
      pieces.push(`<div class="row blank" style="height:${row.height / LINE_HEIGHT}em"></div>`);
      continue;
    }
    if (row.kind === "head") {
      const callee = index.items[row.item];
      pieces.push(
        `<div class="row head" style="padding-left:${indent}px;height:${row.height / LINE_HEIGHT}em">` +
          `<span class="head-mark">▾</span>` +
          `<span class="head-name">${escape(callee?.name ?? "")}</span>` +
          `<span class="head-signature">${escape(callee?.signature ?? "")}</span>` +
          `</div>`,
      );
      continue;
    }
    const line = index.items[row.item]?.lines[row.line];
    pieces.push(
      `<div class="row" style="padding-left:${indent}px">${line ? highlight(line) : ""}</div>`,
    );
  }

  element.innerHTML =
    `<div class="card-head">` +
    `<span class="card-kind k-${item.kind}"></span>` +
    `<span class="card-name">${escape(item.name)}</span>` +
    (item.type
      ? `<span class="card-type">${escape(stripName(item.type, item.name))}</span>`
      : `<span class="card-signature">${escape(item.signature ?? "")}</span>`) +
    `</div>` +
    pieces.join("");
  return element;
}

/**
 * One line as colored spans. References carry their coordinates so a click
 * anywhere in the overlay can be resolved back to the exact occurrence without
 * a second hit test.
 */
function highlight(line: { text: string; tokens: { start: number; end: number; class: number }[]; references: { start: number; end: number; target: number }[] }): string {
  if (line.text.length === 0) return "&nbsp;";
  const classes = new Int8Array(line.text.length).fill(-1);
  for (const token of line.tokens) {
    for (let index = token.start; index < token.end && index < classes.length; index += 1) {
      classes[index] = token.class;
    }
  }
  const referenced = new Uint8Array(line.text.length);
  for (const reference of line.references) {
    for (let index = reference.start; index < reference.end && index < referenced.length; index += 1) {
      referenced[index] = 1;
    }
  }

  const pieces: string[] = [];
  let cursor = 0;
  while (cursor < line.text.length) {
    const isReference = referenced[cursor] === 1;
    const tokenClass = classes[cursor] ?? -1;
    let end = cursor + 1;
    while (
      end < line.text.length &&
      classes[end] === tokenClass &&
      (referenced[end] === 1) === isReference
    ) {
      end += 1;
    }
    const text = escape(line.text.slice(cursor, end));
    if (isReference) {
      const reference = line.references.find((candidate) => candidate.start <= cursor && cursor < candidate.end);
      pieces.push(
        `<span class="t${tokenClass} ref" data-start="${reference?.start ?? cursor}" data-target="${reference?.target ?? -1}">${text}</span>`,
      );
    } else if (tokenClass >= 0) {
      pieces.push(`<span class="t${tokenClass}">${text}</span>`);
    } else {
      pieces.push(text);
    }
    cursor = end;
  }
  return pieces.join("");
}

/** Hover renders a type as `name: T`; the card already says the name. */
const stripName = (rendering: string, name: string): string =>
  rendering.startsWith(`${name}: `) ? rendering.slice(name.length + 2) : rendering;

const escape = (text: string): string =>
  text.replace(/[&<>]/g, (character) =>
    character === "&" ? "&amp;" : character === "<" ? "&lt;" : "&gt;",
  );

const ramp = (value: number, low: number, high: number): number => {
  const t = Math.min(1, Math.max(0, (value - low) / (high - low)));
  return t * t * (3 - 2 * t);
};
