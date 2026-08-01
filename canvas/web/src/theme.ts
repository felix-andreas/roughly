/** The canvas palette. Colors are premultiplied-free RGBA in 0..1. */

export type Color = readonly [number, number, number, number];

const hex = (value: string, alpha = 1): Color => [
  parseInt(value.slice(1, 3), 16) / 255,
  parseInt(value.slice(3, 5), 16) / 255,
  parseInt(value.slice(5, 7), 16) / 255,
  alpha,
];

export const theme = {
  background: hex("#0d1014"),
  grid: hex("#171b21"),

  frame: hex("#12161b"),
  frameBorder: hex("#20262f"),
  frameLabel: hex("#8b949e"),

  card: hex("#171c22"),
  cardBorder: hex("#252c36"),
  cardHover: hex("#1c222a"),
  cardActive: hex("#1e252e"),
  accent: hex("#58a6ff"),
  accentSoft: hex("#58a6ff", 0.22),

  text: hex("#c9d1d9"),
  muted: hex("#7d8590"),
  faint: hex("#5a626d"),

  error: hex("#f85149"),
  warning: hex("#d29922"),

  edge: hex("#3f4854"),
  edgeActive: hex("#58a6ff"),
  edgeIncoming: hex("#3fb950"),

  /** Indexed by the token classes `canvas-core` emits. */
  tokens: [
    hex("#c9d1d9"), // plain
    hex("#ff7b72"), // keyword
    hex("#a5d6ff"), // string
    hex("#79c0ff"), // number
    hex("#6e7681"), // comment
    hex("#ff7b72"), // operator
    hex("#8b949e"), // punctuation
    hex("#d2a8ff"), // callee
    hex("#56d4bc"), // annotation
    hex("#ffa657"), // namespace
  ] as Color[],

  /** Indexed by `kindIndex` in `analysis.ts`. */
  kinds: [
    hex("#58a6ff"), // function
    hex("#ffa657"), // value
    hex("#3fb950"), // type
    hex("#3fb950"), // alias
    hex("#d2a8ff"), // s4class
    hex("#56d4bc"), // s4generic
    hex("#56d4bc"), // s4method
    hex("#d2a8ff"), // r6class
    hex("#56d4bc"), // r6method
    hex("#ffa657"), // r6field
    hex("#7d8590"), // statement
  ] as Color[],
} as const;

export const withAlpha = (color: Color, alpha: number): Color => [
  color[0],
  color[1],
  color[2],
  color[3] * alpha,
];
