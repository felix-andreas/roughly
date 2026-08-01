/**
 * A monospace glyph atlas, rasterized once with canvas2d.
 *
 * Labels on the canvas — file paths, definition names, signatures — must stay
 * legible from a whole-project overview down to nearly full zoom, which is a
 * range no single rasterization survives. Baking them large once and sampling
 * down covers everything up to the point where the DOM overlay takes over with
 * real, selectable text, so the atlas never has to be magnified.
 */

const RASTER_SIZE = 64;
const FIRST_CODE = 32;
const LAST_CODE = 126;
const COLUMNS = 16;
const PADDING = 2;

export const FONT_STACK =
  '"JetBrains Mono", "SF Mono", "Cascadia Mono", "Menlo", "Consolas", ui-monospace, monospace';

export interface Atlas {
  texture: WebGLTexture;
  width: number;
  height: number;
  cellWidth: number;
  cellHeight: number;
  /** Glyph advance as a fraction of font size — the monospace pitch. */
  advanceRatio: number;
  /** Baseline offset from the cell top, as a fraction of font size. */
  baselineRatio: number;
  /** Atlas-space rect of one character, or null when it is unprintable. */
  uv(code: number): readonly [number, number, number, number] | null;
}

export function buildAtlas(gl: WebGL2RenderingContext): Atlas {
  const probe = document.createElement("canvas").getContext("2d");
  if (!probe) throw new Error("canvas2d is unavailable, so text cannot be rasterized");
  probe.font = `${RASTER_SIZE}px ${FONT_STACK}`;
  const advance = probe.measureText("M").width;

  const cellWidth = Math.ceil(advance) + PADDING * 2;
  const cellHeight = Math.ceil(RASTER_SIZE * 1.36) + PADDING * 2;
  const baseline = Math.round(RASTER_SIZE * 1.02) + PADDING;
  const count = LAST_CODE - FIRST_CODE + 1;
  const rows = Math.ceil(count / COLUMNS);

  const canvas = document.createElement("canvas");
  canvas.width = cellWidth * COLUMNS;
  canvas.height = cellHeight * rows;
  const context = canvas.getContext("2d");
  if (!context) throw new Error("canvas2d is unavailable, so text cannot be rasterized");

  context.font = `${RASTER_SIZE}px ${FONT_STACK}`;
  context.textBaseline = "alphabetic";
  context.fillStyle = "#fff";
  for (let index = 0; index < count; index += 1) {
    const column = index % COLUMNS;
    const row = Math.floor(index / COLUMNS);
    context.fillText(
      String.fromCharCode(FIRST_CODE + index),
      column * cellWidth + PADDING,
      row * cellHeight + baseline,
    );
  }

  const texture = gl.createTexture();
  gl.bindTexture(gl.TEXTURE_2D, texture);
  gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, canvas);
  gl.generateMipmap(gl.TEXTURE_2D);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);

  const uvWidth = cellWidth / canvas.width;
  const uvHeight = cellHeight / canvas.height;

  return {
    texture,
    width: canvas.width,
    height: canvas.height,
    cellWidth,
    cellHeight,
    advanceRatio: advance / RASTER_SIZE,
    baselineRatio: baseline / RASTER_SIZE,
    uv(code) {
      if (code < FIRST_CODE || code > LAST_CODE) return null;
      const index = code - FIRST_CODE;
      const column = index % COLUMNS;
      const row = Math.floor(index / COLUMNS);
      return [column * uvWidth, row * uvHeight, uvWidth, uvHeight];
    },
  };
}

export const RASTER_CELL_RATIO = {
  /** Cell size relative to font size, for sizing glyph quads. */
  width: (cellWidth: number) => cellWidth / RASTER_SIZE,
  height: (cellHeight: number) => cellHeight / RASTER_SIZE,
};
