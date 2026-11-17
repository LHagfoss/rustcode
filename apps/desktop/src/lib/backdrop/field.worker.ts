// Ported from Sable (https://github.com/Leonxlnx/sable, src/art/field.worker.ts), MIT License,
// Copyright (c) 2026 Leonxlnx.
/**
 * Light fields off the main thread. Every field on the page draws here on its own
 * OffscreenCanvas, so the backdrop never takes a frame from hover, typing or motion.
 * The page posts sizes, looks, the pointer and whether each field may run; this worker
 * only answers with "drew a frame" (and, in dev, the frame cost).
 */
import type { DitherTexture, FieldBeat, Look, Metrics } from "./fieldBase";
import { FieldEngine } from "./fieldEngine";

export type FieldMessage =
  | { type: "init"; id: number; canvas: OffscreenCanvas; texture: DitherTexture; look: Look }
  | { type: "resize"; id: number; metrics: Metrics; atlas?: ImageBitmap }
  | { type: "look"; id: number; look: Look }
  | { type: "pointer"; id: number; pointer: [number, number] | null }
  | { type: "beat"; id: number; kind: FieldBeat; x: number; y: number; strength: number }
  | { type: "rest"; id: number; rest: boolean }
  /** Development only: freeze the clock and step it (frame sheets); null lets it run. */
  | { type: "clock"; id: number; advance: number | null }
  | { type: "state"; id: number; animate: boolean; running: boolean }
  | { type: "pressure"; id: number; ms: number }
  | { type: "texture"; id: number; texture: DitherTexture }
  /** Copied (structured clone), not transferred: the page keeps its bitmap for other fields. */
  | { type: "image"; id: number; image: ImageBitmap | null }
  | { type: "dispose"; id: number };

export interface FieldReply {
  id: number;
  first: boolean;
  ms: number | null;
  /** Set when the field changed its quality level (see FieldEngine). */
  quality?: number;
}

const scope = self as unknown as {
  onmessage: ((e: MessageEvent<FieldMessage>) => void) | null;
  postMessage: (msg: FieldReply) => void;
};
const engines = new Map<number, FieldEngine>();

scope.onmessage = (e) => {
  const m = e.data;
  if (m.type === "init") {
    const engine = FieldEngine.create(m.canvas, m.texture, m.look, {
      frame: (first, ms) => scope.postMessage({ id: m.id, first, ms }),
      quality: (level) => scope.postMessage({ id: m.id, first: false, ms: null, quality: level }),
    });
    if (engine) engines.set(m.id, engine);
    return;
  }
  const engine = engines.get(m.id);
  if (!engine) return;
  switch (m.type) {
    case "resize":
      engine.resize(m.metrics, m.atlas);
      break;
    case "look":
      engine.setLook(m.look);
      break;
    case "pointer":
      engine.setPointer(m.pointer);
      break;
    case "beat":
      engine.beat(m.kind, m.x, m.y, m.strength);
      break;
    case "rest":
      engine.setRest(m.rest);
      break;
    case "clock":
      engine.clock(m.advance);
      break;
    case "state":
      engine.setState(m.animate, m.running);
      break;
    case "pressure":
      engine.setPressure(m.ms);
      break;
    case "texture":
      engine.setTexture(m.texture);
      break;
    case "image":
      engine.setImage(m.image);
      break;
    case "dispose":
      engine.dispose();
      engines.delete(m.id);
      break;
  }
};
