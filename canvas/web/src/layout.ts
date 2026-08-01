/**
 * Where everything sits, and what each card currently shows.
 *
 * Two rules drive the design. Positions are **deterministic**: the same
 * project always lays out the same way, because a canvas whose landmarks move
 * between runs cannot build spatial memory, which is the only reason to have a
 * canvas at all. And expansion is **local**: opening a definition inline grows
 * its card and pushes the rest of its column down, but never disturbs another
 * column, so what you were looking at stays where you left it.
 */

import type { Index, Item } from "./analysis";
import type { Rect } from "./camera";

export const FONT_SIZE = 12;
export const LINE_HEIGHT = 17;
/** Set once the glyph atlas has measured the font. */
export let CHAR_WIDTH = 7.2;

export function setCharWidth(width: number): void {
  CHAR_WIDTH = width;
}

const CONTENT_COLUMNS = 84;
const CARD_PADDING_X = 11;
const CARD_PADDING_Y = 7;
const CARD_HEADER = 25;
const CARD_GAP = 11;
const FRAME_PADDING = 14;
const FRAME_HEADER = 36;
const FRAME_GAP = 46;
const NEST_INDENT = 14;
const MAX_SUB_COLUMNS = 4;

export const cardWidth = (): number => CONTENT_COLUMNS * CHAR_WIDTH + CARD_PADDING_X * 2;

/** One rendered line inside a card. */
export interface Row {
  kind: "code" | "head" | "blank";
  /** The item whose text this row comes from. */
  item: number;
  /** Line index within that item, for `code` rows. */
  line: number;
  depth: number;
  /** The expansion path of the node this row belongs to. */
  path: string[];
  /** Distance from the card's content top, in world units. */
  y: number;
  height: number;
  /** For `head` rows, the height of the whole block it opens, so the nested
   * body can be drawn with its own backing and rule. */
  extent: number;
}

export interface Card {
  item: number;
  x: number;
  y: number;
  width: number;
  height: number;
  contentTop: number;
  rows: Row[];
}

export interface Frame {
  file: number;
  x: number;
  y: number;
  width: number;
  height: number;
  cards: Card[];
}

export interface Layout {
  frames: Frame[];
  cards: Map<number, Card>;
  bounds: Rect;
}

/**
 * Which references are open, as a tree per top-level card.
 *
 * A reference is keyed by `line:offset` inside its host item, which is stable
 * across re-layouts and unique per occurrence — the same callee referenced
 * twice in one body opens and closes independently, as it must, since the two
 * call sites are different places in the reader's head.
 */
export class Expansions {
  private roots = new Map<number, ExpansionNode>();

  /** Open or close `path` under `root`; returns the new open state. */
  toggle(root: number, path: string[], target: number): boolean {
    let node: ExpansionNode | undefined = this.roots.get(root);
    if (!node) {
      node = { children: new Map() };
      this.roots.set(root, node);
    }
    for (const key of path.slice(0, -1)) {
      const next: ExpansionChild | undefined = node.children.get(key);
      if (!next) return false;
      node = next.node;
    }
    const key = path[path.length - 1];
    if (key === undefined) return false;
    if (node.children.has(key)) {
      node.children.delete(key);
      return false;
    }
    node.children.set(key, { target, node: { children: new Map() } });
    return true;
  }

  collapse(root?: number): void {
    if (root === undefined) this.roots.clear();
    else this.roots.delete(root);
  }

  childrenOf(root: number, path: string[]): Map<string, ExpansionChild> {
    return this.find(root, path)?.children ?? new Map();
  }

  private find(root: number, path: string[]): ExpansionNode | null {
    const root_ = this.roots.get(root);
    if (!root_) return null;
    let node: ExpansionNode = root_;
    for (const key of path) {
      const next: ExpansionChild | undefined = node.children.get(key);
      if (!next) return null;
      node = next.node;
    }
    return node;
  }
}

interface ExpansionNode {
  children: Map<string, ExpansionChild>;
}

interface ExpansionChild {
  target: number;
  node: ExpansionNode;
}

export const referenceKey = (line: number, start: number): string => `${line}:${start}`;

/**
 * The part of the layout that depends only on the project: how wide each file
 * frame is and which of its sub-columns every definition sits in.
 *
 * Computed from **collapsed** sizes and then held fixed, so opening a
 * definition inline can only ever move things downward. A card that jumped to
 * a different sub-column because you expanded its neighbour would be exactly
 * the disorientation this whole layout exists to avoid.
 */
export interface FramePlan {
  file: number;
  /** Item ids per sub-column, each in source order. */
  columns: number[][];
  width: number;
  collapsedHeight: number;
}

export function planFrames(index: Index): FramePlan[] {
  return index.files.map((file, id) => {
    const heights = file.items.map((item) => collapsedCardHeight(index.items[item]!));
    const content = heights.reduce((total, height) => total + height + CARD_GAP, 0);
    // A 900-line file rendered as one ribbon is unreadable at any zoom and
    // sets the height of the entire canvas; splitting it across sub-columns
    // keeps every frame roughly page-shaped.
    const subColumns = Math.max(
      1,
      Math.min(MAX_SUB_COLUMNS, Math.round(Math.sqrt(content / (cardWidth() * 1.35)))),
    );

    const totals = new Array<number>(subColumns).fill(0);
    const columns: number[][] = Array.from({ length: subColumns }, () => []);
    file.items.forEach((item, position) => {
      let shortest = 0;
      for (let slot = 1; slot < subColumns; slot += 1) {
        if (totals[slot]! < totals[shortest]!) shortest = slot;
      }
      columns[shortest]!.push(item);
      totals[shortest] = totals[shortest]! + heights[position]! + CARD_GAP;
    });

    return {
      file: id,
      columns,
      width: subColumns * cardWidth() + (subColumns - 1) * CARD_GAP + FRAME_PADDING * 2,
      collapsedHeight: FRAME_HEADER + Math.max(...totals, CARD_GAP) - CARD_GAP + FRAME_PADDING,
    };
  });
}

/**
 * Lay the whole project out, shelf-packing frames left to right into rows.
 *
 * Row membership comes from the planned widths alone, so it never changes;
 * only the vertical offsets are recomputed from the current heights. Expanding
 * therefore pushes later rows down and disturbs nothing sideways.
 */
export function layout(
  index: Index,
  plans: FramePlan[],
  expansions: Expansions,
  canvasWidth: number,
): Layout {
  const frames: Frame[] = [];
  const cards = new Map<number, Card>();

  let x = 0;
  let rowTop = 0;
  let rowHeight = 0;
  let widest = 0;

  for (const plan of plans) {
    if (x > 0 && x + plan.width > canvasWidth) {
      rowTop += rowHeight + FRAME_GAP;
      x = 0;
      rowHeight = 0;
    }

    const built: Card[] = [];
    let tallest = 0;
    plan.columns.forEach((column, slot) => {
      let cursor = rowTop + FRAME_HEADER;
      const left = x + FRAME_PADDING + slot * (cardWidth() + CARD_GAP);
      for (const item of column) {
        const card = buildCard(index, expansions, item, left, cursor);
        built.push(card);
        cards.set(item, card);
        cursor += card.height + CARD_GAP;
      }
      tallest = Math.max(tallest, cursor - rowTop - CARD_GAP);
    });

    const height = tallest + FRAME_PADDING;
    frames.push({ file: plan.file, x, y: rowTop, width: plan.width, height, cards: built });
    x += plan.width + FRAME_GAP;
    widest = Math.max(widest, x - FRAME_GAP);
    rowHeight = Math.max(rowHeight, height);
  }

  return {
    frames,
    cards,
    bounds: {
      x: 0,
      y: 0,
      width: Math.max(widest, 1),
      height: Math.max(rowTop + rowHeight, 1),
    },
  };
}

/**
 * The shelf width that lands the packed canvas closest to the window's shape,
 * found by trying every row break rather than solved: frame widths are lumpy
 * enough that a closed form would miss.
 */
export function canvasWidthFor(plans: FramePlan[], aspect: number): number {
  const widest = Math.max(...plans.map((plan) => plan.width), 1);
  const total = plans.reduce((sum, plan) => sum + plan.width + FRAME_GAP, 0);

  let best = total;
  let bestError = Infinity;
  for (let candidate = widest; candidate <= total; candidate += widest / 4) {
    const { width, height } = packedExtent(plans, candidate);
    const error = Math.abs(Math.log(width / height / aspect));
    if (error < bestError) {
      bestError = error;
      best = candidate;
    }
  }
  return best;
}

function packedExtent(plans: FramePlan[], canvasWidth: number): { width: number; height: number } {
  let x = 0;
  let rowTop = 0;
  let rowHeight = 0;
  let widest = 0;
  for (const plan of plans) {
    if (x > 0 && x + plan.width > canvasWidth) {
      rowTop += rowHeight + FRAME_GAP;
      x = 0;
      rowHeight = 0;
    }
    x += plan.width + FRAME_GAP;
    widest = Math.max(widest, x - FRAME_GAP);
    rowHeight = Math.max(rowHeight, plan.collapsedHeight);
  }
  return { width: Math.max(widest, 1), height: Math.max(rowTop + rowHeight, 1) };
}

/** The rows a card shows, walking into every open reference. */
function buildCard(
  index: Index,
  expansions: Expansions,
  item: number,
  x: number,
  y: number,
): Card {
  const rows: Row[] = [];
  let cursor = 0;

  const emit = (row: Omit<Row, "y" | "extent">): number => {
    rows.push({ ...row, y: cursor, extent: 0 });
    cursor += row.height;
    return rows.length - 1;
  };

  const walk = (current: number, depth: number, path: string[], ancestors: Set<number>): void => {
    const node = index.items[current];
    if (!node) return;
    const children = expansions.childrenOf(item, path);
    for (let line = 0; line < node.lines.length; line += 1) {
      emit({ kind: "code", item: current, line, depth, path, height: LINE_HEIGHT });
      if (children.size === 0) continue;
      for (const reference of node.lines[line]!.references) {
        const key = referenceKey(line, reference.start);
        const child = children.get(key);
        // A reference back into an enclosing definition would nest forever,
        // so recursion is shown at the call site and never opened.
        if (!child || ancestors.has(child.target)) continue;
        const nested = [...path, key];
        const head = emit({
          kind: "head",
          item: child.target,
          line: 0,
          depth: depth + 1,
          path: nested,
          height: LINE_HEIGHT * 1.4,
        });
        walk(child.target, depth + 1, nested, new Set([...ancestors, child.target]));
        emit({
          kind: "blank",
          item: child.target,
          line: 0,
          depth: depth + 1,
          path: nested,
          height: LINE_HEIGHT * 0.55,
        });
        rows[head]!.extent = cursor - rows[head]!.y;
      }
    }
  };

  walk(item, 0, [], new Set([item]));

  return {
    item,
    x,
    y,
    width: cardWidth(),
    height: CARD_HEADER + CARD_PADDING_Y * 2 + cursor,
    contentTop: CARD_HEADER + CARD_PADDING_Y,
    rows,
  };
}

/** The x offset of a row's text inside its card, honoring nesting depth. */
export const rowIndent = (depth: number): number => CARD_PADDING_X + depth * NEST_INDENT;

const collapsedCardHeight = (item: Item): number =>
  CARD_HEADER + CARD_PADDING_Y * 2 + item.lines.length * LINE_HEIGHT;

export interface Hit {
  frame: Frame;
  card: Card | null;
  row: Row | null;
  /** The reference under the point, when the pointer is on a resolved name. */
  reference: { target: number; path: string[]; key: string } | null;
}

/** What is under a world-space point. */
export function pick(layout: Layout, index: Index, x: number, y: number): Hit | null {
  for (const frame of layout.frames) {
    if (x < frame.x || x > frame.x + frame.width) continue;
    if (y < frame.y || y > frame.y + frame.height) continue;
    for (const card of frame.cards) {
      if (y < card.y || y > card.y + card.height) continue;
      const local = y - card.y - card.contentTop;
      if (local < 0) return { frame, card, row: null, reference: null };
      const row = card.rows.find((candidate) => local >= candidate.y && local < candidate.y + candidate.height);
      if (!row || row.kind !== "code") {
        return { frame, card, row: row ?? null, reference: null };
      }
      const line = index.items[row.item]?.lines[row.line];
      if (!line) return { frame, card, row, reference: null };
      const column = (x - card.x - rowIndent(row.depth)) / CHAR_WIDTH;
      for (const reference of line.references) {
        if (column >= reference.start && column < reference.end) {
          return {
            frame,
            card,
            row,
            reference: {
              target: reference.target,
              path: [...row.path, referenceKey(row.line, reference.start)],
              key: referenceKey(row.line, reference.start),
            },
          };
        }
      }
      return { frame, card, row, reference: null };
    }
    return { frame, card: null, row: null, reference: null };
  }
  return null;
}
