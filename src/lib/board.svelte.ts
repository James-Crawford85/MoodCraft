import type { BoardFile, BoardLinkCard, Camera, Card, FrameCard, GenParams, ImageCard, JobState, NoteCard, Settings } from "./types";
import { NOTE_COLORS, FRAME_COLORS } from "./types";

export const uid = () => crypto.randomUUID().replaceAll("-", "").slice(0, 12);
export const randomSeed = () => Math.floor(Math.random() * 2 ** 48);

const DEFAULT_SETTINGS: Settings = {
  comfyUrl: "http://127.0.0.1:8188",
  t2iSteps: 20,
  t2iCfg: 3.5,
  t2iSize: 768,
  aspect: "1:1",
  fluxEditSteps: 20,
  fluxEditCfg: 1,
  variationStrength: 0.7,
  count: 1,
  moreCount: 4,
  snap: false,
};

function loadSettings(): Settings {
  try {
    return { ...DEFAULT_SETTINGS, ...JSON.parse(localStorage.getItem("bc.settings") ?? "{}") };
  } catch {
    return { ...DEFAULT_SETTINGS };
  }
}

export interface Rect { x: number; y: number; w: number; h: number }

export function intersects(a: Rect, b: Rect) {
  return a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y;
}
export function contains(outer: Rect, inner: Rect) {
  return inner.x >= outer.x && inner.y >= outer.y && inner.x + inner.w <= outer.x + outer.w && inner.y + inner.h <= outer.y + outer.h;
}

/** Central board state. One instance for the app (see `board` export). */
class Board {
  dir = $state<string>("");
  isScratch = $state(true);
  cards = $state<Card[]>([]);
  camera = $state<Camera>({ x: 0, y: 0, zoom: 1 });
  selected = $state<Set<string>>(new Set());
  jobs = $state<Record<string, JobState>>({});
  settings = $state<Settings>(loadSettings());
  dirty = $state(false);
  lastSaved = $state<number | null>(null);
  saveError = $state<string | null>(null);
  /** Display name stored in board.json (set for subboards). */
  title = $state<string | null>(null);
  /** Boards above this one when inside a subboard, root first (for the breadcrumb). */
  trail = $state<{ dir: string; name: string; scratch: boolean }[]>([]);

  #undo: string[] = [];
  #redo: string[] = [];

  get name() {
    if (this.title) return this.title;
    if (this.isScratch) return "Untitled board";
    return this.dir.split(/[\\/]/).filter(Boolean).pop() ?? "Board";
  }

  byId(id: string) {
    return this.cards.find((c) => c.id === id);
  }
  get selection(): Card[] {
    return this.cards.filter((c) => this.selected.has(c.id));
  }
  nextZ() {
    return this.cards.reduce((m, c) => Math.max(m, c.z), 0) + 1;
  }

  // --- history ---------------------------------------------------------------
  /** Call before a user-visible mutation to make it undoable. */
  checkpoint() {
    this.#undo.push(JSON.stringify(this.cards));
    if (this.#undo.length > 100) this.#undo.shift();
    this.#redo = [];
    this.dirty = true;
  }
  undo() {
    const s = this.#undo.pop();
    if (!s) return;
    this.#redo.push(JSON.stringify(this.cards));
    this.cards = this.#mergeLive(JSON.parse(s));
    this.dirty = true;
  }
  redo() {
    const s = this.#redo.pop();
    if (!s) return;
    this.#undo.push(JSON.stringify(this.cards));
    this.cards = this.#mergeLive(JSON.parse(s));
    this.dirty = true;
  }
  /** Keep results of generations that finished after the snapshot was taken. */
  #mergeLive(next: Card[]): Card[] {
    return next.map((c) => {
      const cur = this.byId(c.id);
      if (c.type === "image" && cur?.type === "image" && c.status === "pending" && cur.status !== "pending") {
        return { ...c, status: cur.status, asset: cur.asset, error: cur.error, naturalW: cur.naturalW, naturalH: cur.naturalH, h: cur.h };
      }
      return c;
    });
  }
  touch() {
    this.dirty = true;
  }

  // --- camera ---------------------------------------------------------------
  screenToWorld(sx: number, sy: number) {
    return { x: (sx - this.camera.x) / this.camera.zoom, y: (sy - this.camera.y) / this.camera.zoom };
  }
  zoomAt(sx: number, sy: number, factor: number) {
    const z = Math.min(8, Math.max(0.05, this.camera.zoom * factor));
    const w = this.screenToWorld(sx, sy);
    this.camera = { zoom: z, x: sx - w.x * z, y: sy - w.y * z };
  }
  viewportCenter() {
    return this.screenToWorld(window.innerWidth / 2, window.innerHeight / 2);
  }
  fitTo(cards: Card[] = this.cards) {
    if (!cards.length) {
      this.camera = { x: window.innerWidth / 2, y: window.innerHeight / 2, zoom: 1 };
      return;
    }
    const b = bounds(cards);
    const pad = 120;
    const vw = window.innerWidth, vh = window.innerHeight - 140;
    const z = Math.min(2, Math.max(0.05, Math.min((vw - pad) / b.w, (vh - pad) / b.h)));
    this.camera = { zoom: z, x: vw / 2 - (b.x + b.w / 2) * z, y: 56 + vh / 2 - (b.y + b.h / 2) * z };
  }

  // --- placement -------------------------------------------------------------
  /** Find a free spot of size w×h near (x,y), scanning right then down.
   *  Frames don't block (new cards may land inside one) unless listed in `frames`. */
  freeSpot(x: number, y: number, w: number, h: number, ignore: Set<string> = new Set(), frames: Card[] = []): { x: number; y: number } {
    const gap = 24;
    const blockers = [...this.cards.filter((c) => c.type !== "frame" && !ignore.has(c.id)), ...frames];
    let cx = x, cy = y;
    for (let row = 0; row < 40; row++) {
      for (let i = 0; i < 40; i++) {
        const r = { x: cx - gap / 2, y: cy - gap / 2, w: w + gap, h: h + gap };
        const hit = blockers.find((c) => intersects(r, c));
        if (!hit) return { x: cx, y: cy };
        cx = hit.x + hit.w + gap;
      }
      cx = x;
      cy += h + gap;
    }
    return { x, y };
  }

  // --- card factories --------------------------------------------------------
  addNote(x?: number, y?: number, text = ""): NoteCard {
    const c = this.viewportCenter();
    const card: NoteCard = {
      id: uid(), type: "note", x: (x ?? c.x - 110), y: (y ?? c.y - 90), w: 220, h: 180,
      z: this.nextZ(), text, color: NOTE_COLORS[0],
    };
    this.checkpoint();
    this.cards.push(card);
    this.selected = new Set([card.id]);
    return card;
  }

  /** Add a frame around the selection, or an empty frame at the viewport center.
   *  With `at`, always an empty frame whose top-left is that board point. */
  addFrame(at?: { x: number; y: number }): FrameCard {
    const sel = at ? [] : this.selection.filter((c) => c.type !== "frame");
    let r: Rect;
    if (at) {
      r = { x: at.x, y: at.y, w: 600, h: 400 };
    } else if (sel.length) {
      const b = bounds(sel);
      r = { x: b.x - 32, y: b.y - 56, w: b.w + 64, h: b.h + 88 };
    } else {
      const c = this.viewportCenter();
      r = { x: c.x - 300, y: c.y - 200, w: 600, h: 400 };
    }
    const minZ = this.cards.reduce((m, c) => Math.min(m, c.z), 0);
    const card: FrameCard = { id: uid(), type: "frame", ...r, z: minZ - 1, label: "Frame", color: FRAME_COLORS[0] };
    this.checkpoint();
    this.cards.push(card);
    this.selected = new Set([card.id]);
    return card;
  }

  /** A frame with an explicit rectangle and title, under everything else. */
  addFrameRect(r: Rect, label: string): FrameCard {
    const minZ = this.cards.reduce((m, c) => Math.min(m, c.z), 0);
    const card: FrameCard = { id: uid(), type: "frame", ...r, z: minZ - 1, label, color: FRAME_COLORS[0] };
    this.cards.push(card);
    return card;
  }

  addSubboardCard(path: string, label: string, at: { x: number; y: number }): BoardLinkCard {
    const card: BoardLinkCard = { id: uid(), type: "board", x: at.x, y: at.y, w: 280, h: 230, z: this.nextZ(), label, path };
    this.checkpoint();
    this.cards.push(card);
    this.selected = new Set([card.id]);
    return card;
  }

  addImage(asset: string, natW: number, natH: number, at?: { x: number; y: number }): ImageCard {
    const w = Math.min(360, natW);
    const h = Math.round((w * natH) / natW);
    const c = at ?? this.viewportCenter();
    const p = this.freeSpot(c.x - w / 2, c.y - h / 2, w, h);
    const card: ImageCard = {
      id: uid(), type: "image", ...p, w, h, z: this.nextZ(), asset, status: "ready", naturalW: natW, naturalH: natH,
    };
    this.cards.push(card);
    return card;
  }

  /** Create a pending image card for a generation; caller queues the job. */
  /** With `exact`, the card goes exactly at `near` (caller already found room). */
  addPending(gen: GenParams, near: { x: number; y: number }, parentId?: string, displayW = 300, exact = false): ImageCard {
    const w = displayW;
    const h = Math.round((w * gen.height) / gen.width);
    const p = exact ? { x: near.x, y: near.y } : this.freeSpot(near.x, near.y, w, h);
    const card: ImageCard = {
      id: uid(), type: "image", ...p, w, h, z: this.nextZ(), status: "pending",
      prompt: gen.prompt, gen, parentId,
    };
    this.cards.push(card);
    this.jobs[card.id] = { stage: "starting" };
    return card;
  }

  deleteSelected() {
    if (!this.selected.size) return;
    this.checkpoint();
    this.cards = this.cards.filter((c) => !this.selected.has(c.id));
    this.selected = new Set();
  }

  duplicateSelected() {
    const sel = this.selection;
    if (!sel.length) return;
    this.checkpoint();
    let z = this.nextZ();
    const copies = sel.map((c) => {
      const copy = { ...$state.snapshot(c), id: uid(), x: c.x + 32, y: c.y + 32, z: z++ } as Card;
      if (copy.type === "image" && copy.status === "pending") {
        copy.status = "error";
        copy.error = "Duplicated while generating. Retry?";
      }
      return copy;
    });
    this.cards.push(...copies);
    this.selected = new Set(copies.map((c) => c.id));
  }

  bringToFront() {
    this.checkpoint();
    let z = this.nextZ();
    for (const c of this.selection) if (c.type !== "frame") c.z = z++;
  }

  // --- persistence -----------------------------------------------------------
  serialize(): string {
    const file: BoardFile = { app: "MoodCraft", version: 1, ...(this.title ? { title: this.title } : {}), camera: $state.snapshot(this.camera), cards: $state.snapshot(this.cards) as Card[] };
    return JSON.stringify(file, null, 1);
  }

  load(dir: string, json: string | null, scratch: boolean) {
    this.dir = dir;
    this.isScratch = scratch;
    this.selected = new Set();
    this.jobs = {};
    this.#undo = [];
    this.#redo = [];
    if (json) {
      const f = JSON.parse(json) as BoardFile;
      if (f.app !== "MoodCraft" && f.app !== "BoardCreate") throw new Error("Not a MoodCraft board.json");
      // Pending cards with a ComfyUI prompt id are re-attached by gen.resumePending();
      // ones that never reached the queue can only be retried.
      this.cards = (f.cards ?? []).map((c) =>
        c.type === "image" && c.status === "pending" && !c.promptId
          ? { ...c, status: "error", error: "Generation was interrupted (app closed). Retry?" }
          : c,
      );
      this.camera = f.camera ?? { x: 0, y: 0, zoom: 1 };
      this.title = f.title ?? null;
    } else {
      this.title = null;
      this.cards = [];
      this.camera = { x: window.innerWidth / 2, y: window.innerHeight / 2, zoom: 1 };
    }
    this.dirty = false;
  }

  saveSettings() {
    try {
      localStorage.setItem("bc.settings", JSON.stringify($state.snapshot(this.settings)));
    } catch {
      /* ignore */
    }
  }
}

export function bounds(cards: Rect[]): Rect {
  const x = Math.min(...cards.map((c) => c.x));
  const y = Math.min(...cards.map((c) => c.y));
  const x2 = Math.max(...cards.map((c) => c.x + c.w));
  const y2 = Math.max(...cards.map((c) => c.y + c.h));
  return { x, y, w: x2 - x, h: y2 - y };
}

export const board = new Board();
