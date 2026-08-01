/**
 * The WebGL2 layer: rounded rectangles, reference curves, and atlas text.
 *
 * Everything below the readability threshold is drawn here — frames, cards,
 * the per-line bars that stand in for code when it is too small to read, and
 * the labels. Real code text is the DOM overlay's job, so this renderer never
 * has to magnify a glyph beyond the size it was baked at.
 *
 * Three passes in a fixed order — ambient curves, rectangles, active curves,
 * then glyphs — which is also the paint order, so no depth buffer is needed.
 */

import { buildAtlas, RASTER_CELL_RATIO, type Atlas } from "./atlas";
import type { Camera } from "./camera";
import type { Color } from "./theme";

const RECT_STRIDE = 14;
const GLYPH_STRIDE = 12;

export class Renderer {
  readonly gl: WebGL2RenderingContext;
  readonly atlas: Atlas;

  private rectProgram: WebGLProgram;
  private glyphProgram: WebGLProgram;
  private curveProgram: WebGLProgram;

  private rectArray: WebGLVertexArrayObject;
  private glyphArray: WebGLVertexArrayObject;
  private curveArray: WebGLVertexArrayObject;

  private rectBuffer: WebGLBuffer;
  private glyphBuffer: WebGLBuffer;
  private curveBuffer: WebGLBuffer;

  private rects = new Pool(RECT_STRIDE);
  private glyphs = new Pool(GLYPH_STRIDE);
  private ambient = new Pool(6);
  private active = new Pool(6);

  constructor(canvas: HTMLCanvasElement) {
    const gl = canvas.getContext("webgl2", {
      alpha: false,
      antialias: false,
      premultipliedAlpha: true,
    });
    if (!gl) throw new Error("WebGL2 is unavailable in this browser");
    this.gl = gl;
    this.atlas = buildAtlas(gl);

    this.rectProgram = link(gl, RECT_VERTEX, RECT_FRAGMENT);
    this.glyphProgram = link(gl, GLYPH_VERTEX, GLYPH_FRAGMENT);
    this.curveProgram = link(gl, CURVE_VERTEX, CURVE_FRAGMENT);

    const corners = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, corners);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([0, 0, 1, 0, 0, 1, 1, 1]), gl.STATIC_DRAW);

    this.rectBuffer = gl.createBuffer();
    this.rectArray = gl.createVertexArray();
    gl.bindVertexArray(this.rectArray);
    bindCorner(gl, corners);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.rectBuffer);
    instanced(gl, 1, 4, RECT_STRIDE, 0); // rect
    instanced(gl, 2, 4, RECT_STRIDE, 4); // fill
    instanced(gl, 3, 4, RECT_STRIDE, 8); // border color
    instanced(gl, 4, 2, RECT_STRIDE, 12); // radius, border width

    this.glyphBuffer = gl.createBuffer();
    this.glyphArray = gl.createVertexArray();
    gl.bindVertexArray(this.glyphArray);
    bindCorner(gl, corners);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.glyphBuffer);
    instanced(gl, 1, 4, GLYPH_STRIDE, 0); // dest
    instanced(gl, 2, 4, GLYPH_STRIDE, 4); // source
    instanced(gl, 3, 4, GLYPH_STRIDE, 8); // color

    this.curveBuffer = gl.createBuffer();
    this.curveArray = gl.createVertexArray();
    gl.bindVertexArray(this.curveArray);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.curveBuffer);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 6 * 4, 0);
    gl.enableVertexAttribArray(1);
    gl.vertexAttribPointer(1, 4, gl.FLOAT, false, 6 * 4, 2 * 4);
    gl.bindVertexArray(null);

    gl.enable(gl.BLEND);
    gl.blendFuncSeparate(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA, gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
  }

  begin(background: Color): void {
    this.rects.reset();
    this.glyphs.reset();
    this.ambient.reset();
    this.active.reset();
    const gl = this.gl;
    gl.viewport(0, 0, gl.drawingBufferWidth, gl.drawingBufferHeight);
    gl.clearColor(background[0], background[1], background[2], 1);
    gl.clear(gl.COLOR_BUFFER_BIT);
  }

  rect(
    x: number,
    y: number,
    width: number,
    height: number,
    fill: Color,
    radius = 0,
    borderWidth = 0,
    borderColor: Color = fill,
  ): void {
    if (width <= 0 || height <= 0 || (fill[3] <= 0 && borderColor[3] <= 0)) return;
    this.rects.push(
      x, y, width, height,
      fill[0], fill[1], fill[2], fill[3],
      borderColor[0], borderColor[1], borderColor[2], borderColor[3],
      Math.min(radius, Math.min(width, height) / 2), borderWidth,
    );
  }

  /** Draw `text` with its left edge at `x` and its baseline on `y`. */
  text(x: number, y: number, text: string, size: number, color: Color, maxWidth = Infinity): void {
    if (color[3] <= 0 || size <= 0) return;
    const advance = this.atlas.advanceRatio * size;
    const quadWidth = RASTER_CELL_RATIO.width(this.atlas.cellWidth) * size;
    const quadHeight = RASTER_CELL_RATIO.height(this.atlas.cellHeight) * size;
    const top = y - this.atlas.baselineRatio * size;
    const limit = Math.min(text.length, Math.floor(maxWidth / advance));
    for (let index = 0; index < limit; index += 1) {
      const uv = this.atlas.uv(text.charCodeAt(index));
      if (!uv) continue;
      this.glyphs.push(
        x + index * advance, top, quadWidth, quadHeight,
        uv[0], uv[1], uv[2], uv[3],
        color[0], color[1], color[2], color[3],
      );
    }
  }

  /**
   * A reference curve. `screenWidth` keeps the stroke a constant thickness on
   * screen, so the graph stays readable at every zoom instead of dissolving.
   */
  curve(
    fromX: number,
    fromY: number,
    toX: number,
    toY: number,
    color: Color,
    screenWidth: number,
    scale: number,
    onTop: boolean,
  ): void {
    const pool = onTop ? this.active : this.ambient;
    const half = screenWidth / scale / 2;
    // Signed: the anchors are already chosen on facing sides, so bending
    // along the actual direction keeps the curve monotone instead of looping
    // back on itself when the target sits to the left.
    const bend = (toX - fromX) * 0.45 + Math.sign(toX - fromX || 1) * 18;
    const segments = 14;

    let previousX = fromX;
    let previousY = fromY;
    for (let step = 1; step <= segments; step += 1) {
      const t = step / segments;
      const inverse = 1 - t;
      const x =
        inverse * inverse * inverse * fromX +
        3 * inverse * inverse * t * (fromX + bend) +
        3 * inverse * t * t * (toX - bend) +
        t * t * t * toX;
      const y =
        inverse * inverse * inverse * fromY +
        3 * inverse * inverse * t * fromY +
        3 * inverse * t * t * toY +
        t * t * t * toY;

      const dx = x - previousX;
      const dy = y - previousY;
      const length = Math.hypot(dx, dy) || 1;
      const nx = (-dy / length) * half;
      const ny = (dx / length) * half;

      pool.push(previousX + nx, previousY + ny, ...color);
      pool.push(previousX - nx, previousY - ny, ...color);
      pool.push(x + nx, y + ny, ...color);
      pool.push(previousX - nx, previousY - ny, ...color);
      pool.push(x - nx, y - ny, ...color);
      pool.push(x + nx, y + ny, ...color);

      previousX = x;
      previousY = y;
    }
  }

  end(camera: Camera): void {
    const gl = this.gl;
    const view = new Float32Array([camera.x, camera.y, camera.scale, 0]);
    const viewport = new Float32Array([camera.viewportWidth, camera.viewportHeight]);

    this.drawCurves(this.ambient, view, viewport);

    if (this.rects.count > 0) {
      gl.useProgram(this.rectProgram);
      setView(gl, this.rectProgram, view, viewport);
      gl.bindVertexArray(this.rectArray);
      gl.bindBuffer(gl.ARRAY_BUFFER, this.rectBuffer);
      gl.bufferData(gl.ARRAY_BUFFER, this.rects.view(), gl.DYNAMIC_DRAW);
      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, this.rects.count);
    }

    this.drawCurves(this.active, view, viewport);

    if (this.glyphs.count > 0) {
      gl.useProgram(this.glyphProgram);
      setView(gl, this.glyphProgram, view, viewport);
      gl.activeTexture(gl.TEXTURE0);
      gl.bindTexture(gl.TEXTURE_2D, this.atlas.texture);
      gl.uniform1i(gl.getUniformLocation(this.glyphProgram, "uAtlas"), 0);
      gl.bindVertexArray(this.glyphArray);
      gl.bindBuffer(gl.ARRAY_BUFFER, this.glyphBuffer);
      gl.bufferData(gl.ARRAY_BUFFER, this.glyphs.view(), gl.DYNAMIC_DRAW);
      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, this.glyphs.count);
    }

    gl.bindVertexArray(null);
  }

  private drawCurves(pool: Pool, view: Float32Array, viewport: Float32Array): void {
    if (pool.count === 0) return;
    const gl = this.gl;
    gl.useProgram(this.curveProgram);
    setView(gl, this.curveProgram, view, viewport);
    gl.bindVertexArray(this.curveArray);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.curveBuffer);
    gl.bufferData(gl.ARRAY_BUFFER, pool.view(), gl.DYNAMIC_DRAW);
    gl.drawArrays(gl.TRIANGLES, 0, pool.count);
  }
}

/** A growable float pool; `count` is entries, not floats. */
class Pool {
  private data = new Float32Array(1024);
  private length = 0;

  constructor(private readonly stride: number) {}

  get count(): number {
    return this.length / this.stride;
  }

  reset(): void {
    this.length = 0;
  }

  push(...values: number[]): void {
    if (this.length + values.length > this.data.length) {
      const grown = new Float32Array(Math.max(this.data.length * 2, this.length + values.length));
      grown.set(this.data);
      this.data = grown;
    }
    this.data.set(values, this.length);
    this.length += values.length;
  }

  view(): Float32Array {
    return this.data.subarray(0, this.length);
  }
}

function bindCorner(gl: WebGL2RenderingContext, buffer: WebGLBuffer): void {
  gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
  gl.enableVertexAttribArray(0);
  gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
}

function instanced(
  gl: WebGL2RenderingContext,
  location: number,
  size: number,
  stride: number,
  offset: number,
): void {
  gl.enableVertexAttribArray(location);
  gl.vertexAttribPointer(location, size, gl.FLOAT, false, stride * 4, offset * 4);
  gl.vertexAttribDivisor(location, 1);
}

function setView(
  gl: WebGL2RenderingContext,
  program: WebGLProgram,
  view: Float32Array,
  viewport: Float32Array,
): void {
  gl.uniform4fv(gl.getUniformLocation(program, "uView"), view);
  gl.uniform2fv(gl.getUniformLocation(program, "uViewport"), viewport);
}

function link(gl: WebGL2RenderingContext, vertex: string, fragment: string): WebGLProgram {
  const program = gl.createProgram();
  for (const [type, source] of [
    [gl.VERTEX_SHADER, vertex],
    [gl.FRAGMENT_SHADER, fragment],
  ] as const) {
    const shader = gl.createShader(type);
    if (!shader) throw new Error("the GPU refused to allocate a shader");
    gl.shaderSource(shader, source);
    gl.compileShader(shader);
    if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
      throw new Error(`shader failed to compile: ${gl.getShaderInfoLog(shader)}`);
    }
    gl.attachShader(program, shader);
    gl.deleteShader(shader);
  }
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
    throw new Error(`shader program failed to link: ${gl.getProgramInfoLog(program)}`);
  }
  return program;
}

const PROJECT = `
uniform vec4 uView;      // camera x, camera y, scale, unused
uniform vec2 uViewport;
vec4 project(vec2 world) {
  vec2 screen = (world - uView.xy) * uView.z + uViewport * 0.5;
  vec2 clip = screen / uViewport * 2.0 - 1.0;
  return vec4(clip.x, -clip.y, 0.0, 1.0);
}
`;

const RECT_VERTEX = `#version 300 es
precision highp float;
layout(location = 0) in vec2 aCorner;
layout(location = 1) in vec4 aRect;
layout(location = 2) in vec4 aFill;
layout(location = 3) in vec4 aBorder;
layout(location = 4) in vec2 aParams;
${PROJECT}
out vec2 vLocal;
out vec2 vHalf;
out vec4 vFill;
out vec4 vBorder;
out vec2 vParams;
void main() {
  vHalf = aRect.zw * 0.5;
  vLocal = (aCorner - 0.5) * aRect.zw;
  vFill = aFill;
  vBorder = aBorder;
  vParams = aParams;
  gl_Position = project(aRect.xy + aCorner * aRect.zw);
}
`;

const RECT_FRAGMENT = `#version 300 es
precision highp float;
in vec2 vLocal;
in vec2 vHalf;
in vec4 vFill;
in vec4 vBorder;
in vec2 vParams;
out vec4 fragment;
float roundedBox(vec2 point, vec2 half_, float radius) {
  vec2 q = abs(point) - half_ + radius;
  return min(max(q.x, q.y), 0.0) + length(max(q, 0.0)) - radius;
}
void main() {
  float distance = roundedBox(vLocal, vHalf, vParams.x);
  float edge = fwidth(distance) * 0.7 + 1e-5;
  float inside = 1.0 - smoothstep(-edge, edge, distance);
  vec4 color = vFill;
  if (vParams.y > 0.0) {
    float border = smoothstep(-edge, edge, distance + vParams.y);
    color = mix(vFill, vBorder, border * vBorder.a);
    color.a = mix(vFill.a, max(vFill.a, vBorder.a), border);
  }
  fragment = vec4(color.rgb, color.a * inside);
  if (fragment.a < 0.002) discard;
}
`;

const GLYPH_VERTEX = `#version 300 es
precision highp float;
layout(location = 0) in vec2 aCorner;
layout(location = 1) in vec4 aDest;
layout(location = 2) in vec4 aSource;
layout(location = 3) in vec4 aColor;
${PROJECT}
out vec2 vUv;
out vec4 vColor;
void main() {
  vUv = aSource.xy + aCorner * aSource.zw;
  vColor = aColor;
  gl_Position = project(aDest.xy + aCorner * aDest.zw);
}
`;

const GLYPH_FRAGMENT = `#version 300 es
precision highp float;
uniform sampler2D uAtlas;
in vec2 vUv;
in vec4 vColor;
out vec4 fragment;
void main() {
  float coverage = texture(uAtlas, vUv).a;
  if (coverage < 0.01) discard;
  fragment = vec4(vColor.rgb, vColor.a * coverage);
}
`;

const CURVE_VERTEX = `#version 300 es
precision highp float;
layout(location = 0) in vec2 aPosition;
layout(location = 1) in vec4 aColor;
${PROJECT}
out vec4 vColor;
void main() {
  vColor = aColor;
  gl_Position = project(aPosition);
}
`;

const CURVE_FRAGMENT = `#version 300 es
precision highp float;
in vec4 vColor;
out vec4 fragment;
void main() { fragment = vColor; }
`;
