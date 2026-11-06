// Generation controller: turns UI intents into pending cards + backend jobs,
// and applies backend events back onto the board.
import { api, listenAll, type StatusEvt } from "./api";
import { board, randomSeed, bounds, contains } from "./board.svelte";
import type { GenParams, ImageCard } from "./types";
import { canEdit } from "./models.svelte";

export const comfyState = $state<{ connected: boolean; url: string; queue?: number; problems: string[]; device?: string; checked: boolean }>({
  connected: false,
  url: "",
  problems: [],
  checked: false,
});

const ASPECTS: Record<string, [number, number]> = {
  "1:1": [1, 1], "4:3": [4, 3], "3:4": [3, 4], "16:9": [16, 9], "9:16": [9, 16],
};

function t2iSize(): { width: number; height: number } {
  const s = board.settings;
  const [a, b] = ASPECTS[s.aspect] ?? [1, 1];
  // Keep total pixel count ≈ size², snapped to 16.
  const area = s.t2iSize * s.t2iSize;
  const w = Math.sqrt((area * a) / b);
  const snap = (v: number) => Math.max(256, Math.round(v / 16) * 16);
  return { width: snap(w), height: snap((w * b) / a) };
}

function start(card: ImageCard, resumePromptId?: string) {
  if (!card.gen) return;
  board.jobs[card.id] = { stage: resumePromptId ? "queued" : "starting" };
  api.generate(card.id, board.dir, card.gen, resumePromptId).catch((e) => fail(card.id, String(e)));
}

/** After opening a board: re-attach pending cards to their ComfyUI prompts. */
export function resumePending() {
  for (const c of board.cards) {
    if (c.type === "image" && c.status === "pending" && c.promptId && !board.jobs[c.id]) start(c, c.promptId);
  }
}

function fail(cardId: string, message: string) {
  const c = board.byId(cardId);
  if (c?.type === "image") {
    c.status = "error";
    c.error = message;
    c.promptId = undefined;
  }
  delete board.jobs[cardId];
  board.touch();
}

/** The selection the app made itself after queueing images. A batch of new
 *  results shouldn't turn the next prompt into a remix of them; any selection
 *  the user makes replaces this Set, which re-enables remixing. */
let autoSelection: Set<string> | null = null;
function selectGenerated(ids: string[]) {
  board.selected = autoSelection = new Set(ids);
}

/** Text → image(s), placed at the viewport center (or right of the selection). */
export function generateFromPrompt(prompt: string, count = board.settings.count) {
  prompt = prompt.trim();
  if (!prompt) return;
  const { width, height } = t2iSize();
  const sel = board.selection;
  const near = sel.length
    ? (() => { const b = bounds(sel); return { x: b.x + b.w + 32, y: b.y }; })()
    : (() => { const c = board.viewportCenter(); return { x: c.x - 150 * count, y: c.y - 150 }; })();
  board.checkpoint();
  const ids: string[] = [];
  for (let i = 0; i < count; i++) {
    const gen: GenParams = { kind: "t2i", prompt, seed: randomSeed(), width, height, steps: board.settings.t2iSteps, cfg: board.settings.t2iCfg };
    const card = board.addPending(gen, near);
    ids.push(card.id);
    start(card);
  }
  selectGenerated(ids);
}

// --- Scatter: one prompt → N different takes ---------------------------------
// Each concept gets one modifier from every set (shuffled, so no two concepts
// share a lighting, camera, ... until a set runs out) and its own seed.
const SCATTER_SETS: string[][] = [
  // lighting
  ["golden hour light", "overcast soft light", "dramatic rim lighting", "harsh midday sun", "moody low-key lighting",
   "neon night lighting", "foggy diffused light", "candlelit warm glow", "cold moonlight", "backlit silhouette", "studio softbox lighting", "volumetric god rays"],
  // camera
  ["wide establishing shot", "close-up", "low angle", "high angle looking down", "eye-level medium shot", "aerial view",
   "telephoto compression", "dutch angle", "over-the-shoulder view", "symmetrical front view", "extreme wide shot", "macro detail shot"],
  // palette
  ["muted earth tones", "teal and orange palette", "monochrome blue palette", "desaturated greys with one red accent", "warm amber palette",
   "pastel palette", "high-contrast black and white", "sickly green tint", "deep purples and magentas", "sepia tones", "cool cyan palette", "rich jewel tones"],
  // medium
  ["cinematic film still", "concept art painting", "35mm photograph", "gouache illustration", "charcoal sketch", "3D render",
   "matte painting", "ink and wash", "oil painting", "polaroid photo", "digital painting", "pencil storyboard frame"],
  // mood
  ["eerie and unsettling", "calm and quiet", "tense", "melancholic", "mysterious", "hopeful", "oppressive", "dreamlike",
   "lonely", "chaotic", "serene", "foreboding"],
];
function shuffled<T>(a: T[]): T[] {
  const b = [...a];
  for (let i = b.length - 1; i > 0; i--) { const j = Math.floor(Math.random() * (i + 1)); [b[i], b[j]] = [b[j], b[i]]; }
  return b;
}
export const SCATTER_COUNTS = [8, 12];

/** N concepts from one prompt, in a grid inside a new frame titled with the prompt. */
export function scatterFromPrompt(prompt: string, n: number) {
  prompt = prompt.trim();
  if (!prompt) return;
  const { width, height } = t2iSize();
  const sets = SCATTER_SETS.map(shuffled);
  const cols = Math.ceil(Math.sqrt(n * 1.5)), rows = Math.ceil(n / cols);
  const cw = 300, ch = Math.round((cw * height) / width), gap = 24, pad = 32, head = 56;
  const fw = pad * 2 + cols * cw + (cols - 1) * gap, fh = head + pad + rows * ch + (rows - 1) * gap;
  const sel = board.selection;
  const want = sel.length
    ? (() => { const b = bounds(sel); return { x: b.x + b.w + 48, y: b.y }; })()
    : (() => { const c = board.viewportCenter(); return { x: c.x - fw / 2, y: c.y - fh / 2 }; })();
  const at = board.freeSpot(want.x, want.y, fw, fh, undefined, board.cards.filter((c) => c.type === "frame"));
  board.checkpoint();
  const label = prompt.length > 80 ? prompt.slice(0, 77) + "…" : prompt;
  const frame = board.addFrameRect({ x: at.x, y: at.y, w: fw, h: fh }, `Scatter: ${label}`);
  for (let i = 0; i < n; i++) {
    const mods = sets.map((s) => s[i % s.length]).join(", ");
    const gen: GenParams = { kind: "t2i", prompt: `${prompt}, ${mods}`, seed: randomSeed(), width, height, steps: board.settings.t2iSteps, cfg: board.settings.t2iCfg };
    const x = at.x + pad + (i % cols) * (cw + gap), y = at.y + head + Math.floor(i / cols) * (ch + gap);
    start(board.addPending(gen, { x, y }, undefined, cw, true));
  }
  board.selected = autoSelection = new Set([frame.id]);
}

/** Output size for edits/variations: the source's aspect at the t2i pixel budget. */
function sizeLike(src: ImageCard) {
  const natW = src.naturalW ?? src.w, natH = src.naturalH ?? src.h;
  const area = board.settings.t2iSize ** 2;
  const w = Math.sqrt((area * natW) / natH);
  const snap = (v: number) => Math.max(256, Math.round(v / 16) * 16);
  return { width: snap(w), height: snap((w * natH) / natW) };
}

/** The text that describes a card's content: its own t2i/variation prompt, or
 *  the nearest ancestor's (edit instructions describe a change, not the image). */
function describingPrompt(card: ImageCard | undefined, depth = 0): string | undefined {
  if (!card || depth > 20) return undefined;
  if (card.gen && card.gen.kind !== "edit") return card.gen.prompt;
  const parent = card.parentId ? board.byId(card.parentId) : undefined;
  return describingPrompt(parent?.type === "image" ? parent : undefined, depth + 1);
}

/** Flux Klein reference edit of a source image card. */
export function editImage(src: ImageCard, instruction: string, count = 1) {
  if (!src.asset || !instruction.trim()) return;
  board.checkpoint();
  const ids: string[] = [];
  for (let i = 0; i < count; i++) {
    const gen: GenParams = {
      kind: "edit", prompt: instruction.trim(), seed: randomSeed(), ...sizeLike(src),
      steps: board.settings.fluxEditSteps, cfg: board.settings.fluxEditCfg, source: src.asset,
    };
    const card = board.addPending(gen, { x: src.x + src.w + 32, y: src.y }, src.id, src.w);
    ids.push(card.id);
    start(card);
  }
  selectGenerated(ids);
}

/** With 2+ ready images selected (and an edit-capable family), the prompt bar
 *  remixes them. Order = selection order (Shift/Ctrl-click order), so
 *  "image 1" is the first one picked; the canvas shows these numbers. */
export function remixRefs(): ImageCard[] {
  if (board.selected.size < 2 || !canEdit() || board.selected === autoSelection) return [];
  const imgs = [...board.selected]
    .map((id) => board.byId(id))
    .filter((c): c is ImageCard => c?.type === "image" && c.status === "ready" && !!c.asset);
  return imgs.length >= 2 ? imgs : [];
}

/** Multi-reference edit: image 1 sets the output's aspect; the rest are extra references. */
export function remixImages(refs: ImageCard[], instruction: string, count = 1) {
  if (refs.length < 2 || !instruction.trim()) return;
  const [first, ...rest] = refs;
  const b = bounds(refs);
  board.checkpoint();
  const ids: string[] = [];
  for (let i = 0; i < count; i++) {
    const gen: GenParams = {
      kind: "edit", prompt: instruction.trim(), seed: randomSeed(), ...sizeLike(first),
      steps: board.settings.fluxEditSteps, cfg: board.settings.fluxEditCfg,
      source: first.asset, extraSources: rest.map((r) => r.asset!),
    };
    const card = board.addPending(gen, { x: b.x + b.w + 32, y: b.y }, first.id, first.w);
    ids.push(card.id);
    start(card);
  }
  selectGenerated(ids);
}

/** Flux img2img variations: same prompt, source as the starting latent. */
export function variations(src: ImageCard, count = 1) {
  if (!src.asset) return;
  const prompt = describingPrompt(src) ?? "high quality image, same subject, style and color palette";
  board.checkpoint();
  const ids: string[] = [];
  for (let i = 0; i < count; i++) {
    const gen: GenParams = {
      kind: "variation", prompt, seed: randomSeed(), ...sizeLike(src),
      steps: board.settings.t2iSteps, cfg: board.settings.t2iCfg, source: src.asset,
      denoise: board.settings.variationStrength,
    };
    const card = board.addPending(gen, { x: src.x + src.w + 32, y: src.y }, src.id, src.w);
    ids.push(card.id);
    start(card);
  }
  selectGenerated(ids);
}

// --- Brush inpaint ------------------------------------------------------------
/** Repaint the masked part of `src` per `instruction`; results go beside it.
 *  Uses the text-to-image steps/CFG (Flux Klein needs CFG ≈3.5 here; CFG 1
 *  left malformed shapes in testing). */
export function inpaintImage(src: ImageCard, maskAsset: string, instruction: string, count = 1) {
  if (!src.asset || !instruction.trim()) return;
  board.checkpoint();
  const ids: string[] = [];
  for (let i = 0; i < count; i++) {
    const gen: GenParams = {
      kind: "inpaint", prompt: instruction.trim(), seed: randomSeed(), ...sizeLike(src),
      steps: board.settings.t2iSteps, cfg: board.settings.t2iCfg, source: src.asset, mask: maskAsset,
    };
    const card = board.addPending(gen, { x: src.x + src.w + 32, y: src.y }, src.id, src.w);
    ids.push(card.id);
    start(card);
  }
  selectGenerated(ids);
}

// --- More like this ------------------------------------------------------------
/** Slots around a card, nearest first: right, below, left, above, then the diagonals. */
function fanSlots(src: ImageCard, w: number, h: number) {
  const g = 32;
  const R = src.x + src.w + g, L = src.x - w - g, B = src.y + src.h + g, T = src.y - h - g;
  return [[R, src.y], [src.x, B], [L, src.y], [src.x, T], [R, B], [L, B], [R, T], [L, T]].map(([x, y]) => ({ x, y }));
}

/** N img2img variations fanned around the source, which stays put. Strength
 *  is the variation strength (subtle 0.3 … loose 0.95).
 *  (A "keep identity" mode via Flux Klein reference edit was tried: asking for
 *  a different pose/angle returned near-copies at CFG 1–4, so it isn't offered.) */
export function moreLikeThis(src: ImageCard, n = board.settings.moreCount) {
  if (!src.asset) return;
  n = Math.max(1, Math.min(8, n));
  const size = sizeLike(src);
  const w = src.w, h = Math.round((w * size.height) / size.width);
  const slots = fanSlots(src, w, h);
  // Don't drop new takes into some other frame (they'd travel with it).
  const otherFrames = board.cards.filter((c) => c.type === "frame" && !contains(c, src));
  const prompt = describingPrompt(src) ?? "high quality image, same subject, style and color palette";
  board.checkpoint();
  const ids: string[] = [];
  for (let i = 0; i < n; i++) {
    const gen: GenParams = { kind: "variation", prompt, seed: randomSeed(), ...size, steps: board.settings.t2iSteps,
      cfg: board.settings.t2iCfg, source: src.asset, denoise: board.settings.variationStrength };
    const s = slots[i % slots.length];
    const card = board.addPending(gen, board.freeSpot(s.x, s.y, w, h, undefined, otherFrames), src.id, w, true);
    ids.push(card.id);
    start(card);
  }
  selectGenerated(ids);
}

/** Re-run a failed card in place (same params, same seed). */
export function retry(card: ImageCard) {
  if (!card.gen) return;
  card.status = "pending";
  card.error = undefined;
  card.promptId = undefined;
  board.touch();
  start(card);
}

/** New seed for a generated card, as a sibling card. */
export function reroll(card: ImageCard) {
  if (!card.gen) return;
  board.checkpoint();
  const gen = { ...card.gen, seed: randomSeed() };
  const c = board.addPending(gen, { x: card.x + card.w + 32, y: card.y }, card.parentId, card.w);
  start(c);
  board.selected = new Set([c.id]);
}

export function cancel(card: ImageCard) {
  api.cancel(card.id).catch(() => {});
}

export async function refreshCheck() {
  try {
    const r = await api.comfyCheck();
    comfyState.problems = r.problems;
    comfyState.device = r.device;
    comfyState.connected = r.connected;
  } catch (e) {
    comfyState.problems = [String(e)];
  }
  comfyState.checked = true;
}

const STAGE_LABEL: Record<string, string> = {
  preparing: "Preparing…", queued: "Queued in ComfyUI…", running: "Loading models…", sampling: "Generating", downloading: "Saving…",
};
export const stageLabel = (s: string) => STAGE_LABEL[s] ?? "Starting…";

export async function initGenEvents() {
  return listenAll({
    progress: (e) => {
      const j = board.jobs[e.cardId] ?? (board.jobs[e.cardId] = { stage: e.stage });
      j.stage = e.stage;
      if (e.stage === "sampling") { j.value = e.value; j.max = e.max; }
      if (e.promptId) {
        const c = board.byId(e.cardId);
        if (c?.type === "image" && c.promptId !== e.promptId) { c.promptId = e.promptId; board.touch(); }
      }
    },
    preview: (e) => {
      const j = board.jobs[e.cardId];
      if (j) j.preview = e.dataUrl;
    },
    done: (e) => {
      const c = board.byId(e.cardId);
      delete board.jobs[e.cardId];
      if (c?.type !== "image") return; // card was deleted meanwhile
      c.asset = e.asset;
      c.status = "ready";
      c.error = undefined;
      c.promptId = undefined;
      c.naturalW = e.width;
      c.naturalH = e.height;
      c.h = Math.round((c.w * e.height) / e.width);
      board.touch();
    },
    error: (e) => {
      if (e.cancelled) {
        const c = board.byId(e.cardId);
        delete board.jobs[e.cardId];
        if (c?.type === "image") { c.status = "error"; c.error = "Cancelled"; c.promptId = undefined; board.touch(); }
        return;
      }
      fail(e.cardId, e.message);
    },
    status: (e: StatusEvt) => {
      const was = comfyState.connected;
      comfyState.connected = e.connected;
      comfyState.url = e.url;
      if (e.queueRemaining !== undefined && e.queueRemaining !== null) comfyState.queue = e.queueRemaining;
      if (e.connected && !was) refreshCheck();
    },
  });
}
