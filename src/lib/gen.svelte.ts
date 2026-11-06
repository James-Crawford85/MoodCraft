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

function start(card: ImageCard) {
  if (!card.gen) return;
  board.jobs[card.id] = { stage: "starting" };
  api.generate(card.id, board.dir, card.gen).catch((e) => fail(card.id, String(e)));
}

function fail(cardId: string, message: string) {
  const c = board.byId(cardId);
  if (c?.type === "image") {
    c.status = "error";
    c.error = message;
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

const VARIATION_PROMPT =
  "Create a variation of this image. Keep the same subject, style, color palette and mood, but change the composition, pose, camera angle and small details.";

/** Qwen-Image-Edit on a source image card. `instruction` empty → variation. */
export function editImage(src: ImageCard, instruction: string, count = 1) {
  if (!src.asset) return;
  const prompt = instruction.trim() ? instruction.trim() : VARIATION_PROMPT;
  const natW = src.naturalW ?? src.w, natH = src.naturalH ?? src.h;
  board.checkpoint();
  const ids: string[] = [];
  for (let i = 0; i < count; i++) {
    const gen: GenParams = {
      kind: "edit", prompt, seed: randomSeed(), width: natW, height: natH,
      steps: board.settings.editSteps, cfg: board.settings.editCfg, source: src.asset,
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
      c.naturalW = e.width;
      c.naturalH = e.height;
      c.h = Math.round((c.w * e.height) / e.width);
      board.touch();
    },
    error: (e) => {
      if (e.cancelled) {
        const c = board.byId(e.cardId);
        delete board.jobs[e.cardId];
        if (c?.type === "image") { c.status = "error"; c.error = "Cancelled"; board.touch(); }
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
