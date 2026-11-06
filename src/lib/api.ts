import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { GenParams } from "./types";

export interface CheckReport {
  connected: boolean;
  problems: string[];
  device?: string;
  vramFreeMb?: number;
}

export interface Imported {
  asset: string;
  width: number;
  height: number;
}

export interface ModelConfig { family: string; unet: string; clip: string; clipType: string; vae: string; ckpt: string }
type HintList = string[][];
export interface Family {
  id: string; label: string; description: string; loaders: "split" | "checkpoint"; clipType: string;
  hints: { unet: HintList; clip: HintList; vae: HintList; ckpt: HintList };
  defaults: { size: number; steps: number; cfg: number; editSteps: number; editCfg: number; variationStrength: number; sampler: string; scheduler: string; testSteps: number };
  supportsEdit: boolean; supportsVariation: boolean;
}
export interface ModelLists { diffusionModels: string[]; textEncoders: string[]; vae: string[]; checkpoints: string[]; clipTypes: string[] }

export const api = {
  modelsGet: () => invoke<{ config: ModelConfig; families: Family[] }>("models_get"),
  modelsList: () => invoke<ModelLists>("models_list"),
  modelsSet: (config: ModelConfig) => invoke<void>("models_set", { config }),
  modelsTest: (config: ModelConfig) => invoke<{ seconds: number }>("models_test", { config }),
  comfyCheck: () => invoke<CheckReport>("comfy_check"),
  comfyStatus: () => invoke<[boolean, string]>("comfy_status"),
  comfySetUrl: (url: string) => invoke<void>("comfy_set_url", { url }),
  generate: (cardId: string, boardDir: string, p: GenParams, resumePromptId?: string) =>
    invoke<void>("generate", { req: { cardId, boardDir, ...p, negative: "", resumePromptId } }),
  free: () => invoke<void>("comfy_free"),
  cancel: (cardId: string) => invoke<void>("cancel_generation", { cardId }),
  boardNew: () => invoke<string>("board_new"),
  boardOpen: (dir: string) => invoke<string>("board_open", { dir }),
  boardSave: (dir: string, json: string) => invoke<void>("board_save", { dir, json }),
  boardSaveAs: (from: string, to: string, json: string) => invoke<void>("board_save_as", { from, to, json }),
  importUrl: (boardDir: string, url: string) => invoke<Imported>("import_url", { boardDir, url }),
  subboardCreate: (parent: string, title: string) => invoke<string>("subboard_create", { parent, title }),
  importImage: (boardDir: string, path: string) => invoke<Imported>("import_image", { boardDir, path }),
  importImageBytes: (boardDir: string, bytes: Uint8Array, ext: string) =>
    invoke<Imported>("import_image_bytes", bytes, {
      headers: { "x-board-dir": encodeURIComponent(boardDir), "x-ext": ext },
    }),
};

export function assetUrl(boardDir: string, asset: string): string {
  const sep = boardDir.includes("\\") ? "\\" : "/";
  return convertFileSrc(boardDir + sep + asset.replaceAll("/", sep));
}

export interface ProgressEvt { cardId: string; stage: string; value?: number; max?: number; node?: string; promptId?: string }
export interface PreviewEvt { cardId: string; dataUrl: string }
export interface DoneEvt { cardId: string; asset: string; width: number; height: number; seed: number }
export interface ErrorEvt { cardId: string; message: string; oom: boolean; cancelled: boolean }
export interface StatusEvt { connected: boolean; url: string; queueRemaining?: number }

export async function listenAll(h: {
  progress: (e: ProgressEvt) => void;
  preview: (e: PreviewEvt) => void;
  done: (e: DoneEvt) => void;
  error: (e: ErrorEvt) => void;
  status: (e: StatusEvt) => void;
}): Promise<UnlistenFn> {
  const fns = await Promise.all([
    listen<ProgressEvt>("gen://progress", (e) => h.progress(e.payload)),
    listen<PreviewEvt>("gen://preview", (e) => h.preview(e.payload)),
    listen<DoneEvt>("gen://done", (e) => h.done(e.payload)),
    listen<ErrorEvt>("gen://error", (e) => h.error(e.payload)),
    listen<StatusEvt>("comfy://status", (e) => h.status(e.payload)),
  ]);
  return () => fns.forEach((f) => f());
}
