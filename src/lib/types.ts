export type CardType = "image" | "note" | "frame";

export interface BaseCard {
  id: string;
  type: CardType;
  x: number;
  y: number;
  w: number;
  h: number;
  z: number;
}

/** Parameters needed to (re)run a generation for a card. */
export interface GenParams {
  kind: "t2i" | "edit";
  prompt: string;
  seed: number;
  width: number;
  height: number;
  steps: number;
  cfg: number;
  /** Board-relative asset of the source image (edit only). */
  source?: string;
}

export interface ImageCard extends BaseCard {
  type: "image";
  asset?: string;
  status: "ready" | "pending" | "error";
  error?: string;
  /** Prompt / instruction that produced the image, if generated. */
  prompt?: string;
  gen?: GenParams;
  /** Card this image was derived from (variation / edit). */
  parentId?: string;
  /** ComfyUI prompt id while queued/running, so a reload can re-attach. */
  promptId?: string;
  /** Natural pixel size of the asset. */
  naturalW?: number;
  naturalH?: number;
}

export interface NoteCard extends BaseCard {
  type: "note";
  text: string;
  color: string;
}

export interface FrameCard extends BaseCard {
  type: "frame";
  label: string;
  color: string;
}

export type Card = ImageCard | NoteCard | FrameCard;

export interface Camera {
  x: number; // screen-space offset of world origin
  y: number;
  zoom: number;
}

export interface BoardFile {
  app: "BoardCreate";
  version: 1;
  camera: Camera;
  cards: Card[];
}

/** Live (unsaved) job state for a pending image card. */
export interface JobState {
  stage: string;
  value?: number;
  max?: number;
  preview?: string;
}

export interface Settings {
  comfyUrl: string;
  t2iSteps: number;
  t2iCfg: number;
  t2iSize: number;
  aspect: "1:1" | "4:3" | "3:4" | "16:9" | "9:16";
  editSteps: number;
  editCfg: number;
  count: number;
  snap: boolean;
}

export const NOTE_COLORS = ["#f7e27a", "#9be3b0", "#9cc9ff", "#f6a7c9", "#d6b8ff", "#2b2e35"];
export const FRAME_COLORS = ["#6c7380", "#5b8def", "#3fb67a", "#e0a43b", "#d65f87", "#9b6be0"];
