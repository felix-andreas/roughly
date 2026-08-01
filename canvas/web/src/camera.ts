/**
 * The view onto the canvas: where we are and how far in.
 *
 * Scale is world units per CSS pixel, so at scale 1 a code line is exactly as
 * tall as it would be in an editor. Every zoom threshold in the renderer is
 * expressed against that, which keeps "is this text readable yet" a question
 * about real pixels rather than about an arbitrary unit.
 */

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export const MIN_SCALE = 0.012;
export const MAX_SCALE = 2.4;

export class Camera {
  x = 0;
  y = 0;
  scale = 1;

  private targetX = 0;
  private targetY = 0;
  private targetScale = 1;

  viewportWidth = 1;
  viewportHeight = 1;

  /** True while the view is still settling, so the loop keeps drawing. */
  settling(): boolean {
    return (
      Math.abs(this.x - this.targetX) * this.scale > 0.05 ||
      Math.abs(this.y - this.targetY) * this.scale > 0.05 ||
      Math.abs(Math.log(this.scale / this.targetScale)) > 0.0005
    );
  }

  advance(seconds: number): void {
    // Framerate-independent exponential smoothing: the same visual settling
    // time whether the display runs at 60 or 144 Hz.
    const blend = 1 - Math.exp(-seconds * 22);
    this.x += (this.targetX - this.x) * blend;
    this.y += (this.targetY - this.y) * blend;
    this.scale *= Math.pow(this.targetScale / this.scale, blend);
    if (!this.settling()) {
      this.x = this.targetX;
      this.y = this.targetY;
      this.scale = this.targetScale;
    }
  }

  /** Pan by a screen-pixel delta, applied immediately so dragging tracks 1:1. */
  panBy(screenX: number, screenY: number): void {
    this.targetX -= screenX / this.targetScale;
    this.targetY -= screenY / this.targetScale;
    this.x -= screenX / this.scale;
    this.y -= screenY / this.scale;
  }

  /** Zoom about a screen point, keeping the world point under it fixed. */
  zoomAt(screenX: number, screenY: number, factor: number): void {
    const next = clamp(this.targetScale * factor, MIN_SCALE, MAX_SCALE);
    const world = this.screenToWorld(screenX, screenY, this.targetX, this.targetY, this.targetScale);
    this.targetScale = next;
    this.targetX = world.x - (screenX - this.viewportWidth / 2) / next;
    this.targetY = world.y - (screenY - this.viewportHeight / 2) / next;
  }

  /** Frame `rect`, leaving `padding` screen pixels of margin. */
  flyTo(rect: Rect, padding = 64, maxScale = MAX_SCALE): void {
    const scale = clamp(
      Math.min(
        (this.viewportWidth - padding * 2) / Math.max(rect.width, 1),
        (this.viewportHeight - padding * 2) / Math.max(rect.height, 1),
      ),
      MIN_SCALE,
      maxScale,
    );
    this.targetScale = scale;
    this.targetX = rect.x + rect.width / 2;
    this.targetY = rect.y + rect.height / 2;
  }

  /** Jump without animating — used to place the opening view. */
  settle(): void {
    this.x = this.targetX;
    this.y = this.targetY;
    this.scale = this.targetScale;
  }

  worldToScreen(x: number, y: number): { x: number; y: number } {
    return {
      x: (x - this.x) * this.scale + this.viewportWidth / 2,
      y: (y - this.y) * this.scale + this.viewportHeight / 2,
    };
  }

  screenToWorld(
    screenX: number,
    screenY: number,
    originX = this.x,
    originY = this.y,
    scale = this.scale,
  ): { x: number; y: number } {
    return {
      x: (screenX - this.viewportWidth / 2) / scale + originX,
      y: (screenY - this.viewportHeight / 2) / scale + originY,
    };
  }

  /** The world rectangle currently on screen, for culling. */
  visible(margin = 0): Rect {
    const width = this.viewportWidth / this.scale + margin * 2;
    const height = this.viewportHeight / this.scale + margin * 2;
    return { x: this.x - width / 2, y: this.y - height / 2, width, height };
  }
}

export const clamp = (value: number, low: number, high: number): number =>
  Math.min(high, Math.max(low, value));

export const overlaps = (a: Rect, b: Rect): boolean =>
  a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;

/** Smoothstep in 0..1, for every crossfade the renderer does. */
export const ramp = (value: number, low: number, high: number): number => {
  const t = clamp((value - low) / (high - low), 0, 1);
  return t * t * (3 - 2 * t);
};
