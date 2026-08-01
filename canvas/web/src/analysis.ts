/**
 * Loading `canvas_core.wasm` and normalizing the index it returns.
 *
 * The core speaks byte offsets, because that is what a Rust lexer produces;
 * the DOM speaks UTF-16 code units. Every offset is translated once here, so
 * nothing downstream has to remember which of the two it is holding.
 */

export interface RawItem {
  file: number;
  name: string;
  kind: string;
  signature: string | null;
  type_rendering: string | null;
  doc: string | null;
  code: string;
  line: number;
  tokens: number[];
  references: number[];
  errors: number;
  warnings: number;
}

export interface RawIndex {
  files: { path: string; lines: number; items: number[] }[];
  items: RawItem[];
  edges: [number, number][];
}

export interface Token {
  start: number;
  end: number;
  class: number;
}

export interface Reference {
  start: number;
  end: number;
  target: number;
}

/** One source line of an item, with everything needed to draw it. */
export interface Line {
  text: string;
  tokens: Token[];
  references: Reference[];
  /** Leading whitespace, in characters — the minimap's x offset. */
  indent: number;
}

export interface Item {
  id: number;
  file: number;
  name: string;
  kind: string;
  kindIndex: number;
  signature: string | null;
  type: string | null;
  doc: string | null;
  line: number;
  lines: Line[];
  errors: number;
  warnings: number;
  /** Items this one references, deduplicated, in first-use order. */
  outgoing: number[];
  incoming: number[];
  /** The longest line, in characters. */
  width: number;
}

export interface FileNode {
  path: string;
  name: string;
  directory: string;
  items: number[];
}

export interface Index {
  files: FileNode[];
  items: Item[];
  edges: [number, number][];
}

const KIND_ORDER = [
  "function",
  "value",
  "type",
  "alias",
  "s4class",
  "s4generic",
  "s4method",
  "r6class",
  "r6method",
  "r6field",
  "statement",
];

export interface Project {
  files: { path: string; text: string }[];
  description: string | null;
  namespace: string | null;
}

interface CoreExports {
  memory: WebAssembly.Memory;
  canvas_alloc: (length: number) => number;
  canvas_analyze: (request: number, length: number) => number;
  canvas_release: (response: number, length: number) => void;
}

let core: CoreExports | null = null;

async function load(): Promise<CoreExports> {
  if (core) return core;
  const url = new URL("../canvas_core.wasm", import.meta.url);
  const { instance } = await WebAssembly.instantiateStreaming(fetch(url), {});
  core = instance.exports as unknown as CoreExports;
  return core;
}

/** Analyze a project and return the normalized index. */
export async function analyze(project: Project): Promise<Index> {
  const exports = await load();
  const payload = new TextEncoder().encode(JSON.stringify(project));

  const request = exports.canvas_alloc(payload.length);
  new Uint8Array(exports.memory.buffer, request, payload.length).set(payload);
  const response = exports.canvas_analyze(request, payload.length);

  // `memory.buffer` is detached and replaced whenever the module grows its
  // heap, so every view must be taken after the call that could have grown it.
  const length = new DataView(exports.memory.buffer).getUint32(response, true);
  const json = new TextDecoder().decode(
    new Uint8Array(exports.memory.buffer, response + 4, length),
  );
  exports.canvas_release(response, length);

  const result = JSON.parse(json) as { index?: RawIndex; error?: string };
  if (result.error !== undefined || result.index === undefined) {
    throw new Error(result.error ?? "the analysis returned no index");
  }
  return normalize(result.index);
}

function normalize(raw: RawIndex): Index {
  const items: Item[] = raw.items.map((item, id) => {
    const toUtf16 = offsetTranslator(item.code);
    const lines = splitLines(item.code);

    for (let cursor = 0; cursor + 2 < item.tokens.length; cursor += 3) {
      const start = toUtf16(item.tokens[cursor]!);
      const end = toUtf16(item.tokens[cursor]! + item.tokens[cursor + 1]!);
      place(lines, start, end, (line, from, to) =>
        line.tokens.push({ start: from, end: to, class: item.tokens[cursor + 2]! }),
      );
    }
    for (let cursor = 0; cursor + 2 < item.references.length; cursor += 3) {
      const start = toUtf16(item.references[cursor]!);
      const end = toUtf16(item.references[cursor]! + item.references[cursor + 1]!);
      place(lines, start, end, (line, from, to) =>
        line.references.push({ start: from, end: to, target: item.references[cursor + 2]! }),
      );
    }

    const kindIndex = Math.max(0, KIND_ORDER.indexOf(item.kind));
    return {
      id,
      file: item.file,
      name: item.name,
      kind: item.kind,
      kindIndex,
      signature: item.signature,
      // `Unknown` is the checker having nothing to say; a card showing it
      // reads as a finding rather than as silence.
      type: item.type_rendering?.endsWith("Unknown") ? null : item.type_rendering,
      doc: item.doc,
      line: item.line,
      lines,
      errors: item.errors,
      warnings: item.warnings,
      outgoing: [],
      incoming: [],
      width: lines.reduce((widest, line) => Math.max(widest, line.text.length), 0),
    };
  });

  for (const [from, to] of raw.edges) {
    items[from]?.outgoing.push(to);
    items[to]?.incoming.push(from);
  }

  const files: FileNode[] = raw.files.map((file) => {
    const cut = file.path.lastIndexOf("/");
    return {
      path: file.path,
      name: cut < 0 ? file.path : file.path.slice(cut + 1),
      directory: cut < 0 ? "" : file.path.slice(0, cut),
      items: file.items,
    };
  });

  return { files, items, edges: raw.edges };
}

function splitLines(code: string): Line[] {
  return code.split("\n").map((text) => ({
    text,
    tokens: [],
    references: [],
    indent: text.length - text.trimStart().length,
  }));
}

/**
 * Assign a span to the lines it covers, clipped to each. Spans that straddle a
 * newline are real — a multi-line string, a `#:` annotation block — so each
 * line gets the piece that falls inside it.
 */
function place(
  lines: Line[],
  start: number,
  end: number,
  add: (line: Line, from: number, to: number) => void,
): void {
  let offset = 0;
  for (const line of lines) {
    const lineEnd = offset + line.text.length;
    if (start < lineEnd && end > offset) {
      add(line, Math.max(0, start - offset) , Math.min(line.text.length, end - offset));
    }
    if (lineEnd >= end) break;
    offset = lineEnd + 1;
  }
}

/**
 * Byte offset to UTF-16 index. Pure-ASCII code — the overwhelming majority of
 * R sources — skips the table entirely and translates by identity.
 */
function offsetTranslator(code: string): (offset: number) => number {
  let ascii = true;
  for (let index = 0; index < code.length; index += 1) {
    if (code.charCodeAt(index) > 127) {
      ascii = false;
      break;
    }
  }
  if (ascii) return (offset) => offset;

  const table = new Map<number, number>();
  let bytes = 0;
  for (let index = 0; index < code.length; ) {
    table.set(bytes, index);
    const point = code.codePointAt(index)!;
    const units = point > 0xffff ? 2 : 1;
    bytes += point < 0x80 ? 1 : point < 0x800 ? 2 : point < 0x10000 ? 3 : 4;
    index += units;
  }
  table.set(bytes, code.length);
  return (offset) => table.get(offset) ?? code.length;
}
