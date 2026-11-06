// Generation controller: turns UI intents into pending cards + backend jobs,
// and applies backend events back onto the board.
import { api, listenAll, type StatusEvt } from "./api";
import { board, randomSeed, bounds } from "./board.svelte";
import type { GenParams, ImageCard } from "./types";

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
  board.selected = new Set(ids);
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
  board.selected = new Set(ids);
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
  board.selected = new Set(ids);
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
