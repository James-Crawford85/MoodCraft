// Active model family/selection, shared by the settings panel and the
// canvas UI (which hides Edit/Variations for families that lack them).
import { api, type Family, type ModelConfig } from "./api";
import { board } from "./board.svelte";

export const models = $state<{ config: ModelConfig | null; families: Family[] }>({ config: null, families: [] });

export function activeFamily(): Family | undefined {
  return models.families.find((f) => f.id === models.config?.family);
}
export const canEdit = () => activeFamily()?.supportsEdit ?? true;
export const canVary = () => activeFamily()?.supportsVariation ?? true;

export async function loadModels() {
  const r = await api.modelsGet();
  models.config = r.config;
  models.families = r.families;
}

/** Pick the first file matching a family hint (all tokens, case-insensitive),
 *  else keep `current` if installed, else the first file. */
export function pickByHints(files: string[], hints: string[][], current?: string): string {
  for (const tokens of hints) {
    const hit = files.find((f) => tokens.every((t) => f.toLowerCase().includes(t)));
    if (hit) return hit;
  }
  if (current && files.includes(current)) return current;
  return files[0] ?? "";
}

/** Persist + activate. When the family changed, reset sampling settings to its defaults. */
export async function saveModels(next: ModelConfig) {
  const prevFamily = models.config?.family;
  await api.modelsSet(next);
  models.config = next;
  const fam = activeFamily();
  if (fam && prevFamily !== next.family) {
    const s = board.settings;
    s.t2iSize = fam.defaults.size;
    s.t2iSteps = fam.defaults.steps;
    s.t2iCfg = fam.defaults.cfg;
    s.fluxEditSteps = fam.defaults.editSteps;
    s.fluxEditCfg = fam.defaults.editCfg;
    s.variationStrength = fam.defaults.variationStrength;
    board.saveSettings();
  }
}
