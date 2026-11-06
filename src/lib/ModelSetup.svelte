<script lang="ts">
  import { onMount } from "svelte";
  import { api, type ModelConfig, type ModelLists } from "./api";
  import { comfyState, refreshCheck } from "./gen.svelte";
  import { models, pickByHints, saveModels } from "./models.svelte";

  let draft = $state<ModelConfig | null>(models.config ? { ...models.config } : null);
  let lists = $state<ModelLists | null>(null);
  let listError = $state<string | null>(null);
  let loading = $state(false);
  let testing = $state(false);
  let testResult = $state<{ ok: boolean; text: string } | null>(null);
  let saving = $state(false);
  let savedNote = $state<string | null>(null);

  const fam = $derived(models.families.find((f) => f.id === draft?.family));
  const dirty = $derived(!!draft && !!models.config && JSON.stringify(draft) !== JSON.stringify(models.config));
  const offline = $derived(!comfyState.connected || !lists);

  async function refresh() {
    loading = true;
    listError = null;
    try {
      lists = await api.modelsList();
    } catch (e) {
      lists = null;
      listError = String(e);
    }
    loading = false;
  }

  /** Options for a dropdown: installed files, plus the saved value if it's missing. */
  function options(files: string[] | undefined, value: string): { v: string; missing: boolean }[] {
    const out = (files ?? []).map((v) => ({ v, missing: false }));
    if (value && !out.some((o) => o.v === value)) out.unshift({ v: value, missing: !!files });
    return out;
  }

  function changeFamily(id: string) {
    if (!draft) return;
    const f = models.families.find((x) => x.id === id);
    if (!f) return;
    const d = { ...draft, family: id };
    if (lists) {
      if (f.loaders === "split") {
        d.unet = pickByHints(lists.diffusionModels, f.hints.unet, d.unet);
        d.clip = pickByHints(lists.textEncoders, f.hints.clip, d.clip);
        d.vae = pickByHints(lists.vae, f.hints.vae, d.vae);
      } else {
        d.ckpt = pickByHints(lists.checkpoints, f.hints.ckpt, d.ckpt);
      }
    }
    if (f.clipType) d.clipType = f.clipType;
    draft = d;
    testResult = null;
  }

  async function test() {
    if (!draft) return;
    testing = true;
    testResult = null;
    try {
      const r = await api.modelsTest($state.snapshot(draft));
      testResult = { ok: true, text: `Works: a ${fam?.defaults.testSteps ?? 4}-step 256×256 test image rendered in ${r.seconds.toFixed(1)} s (includes loading models).` };
    } catch (e) {
      testResult = { ok: false, text: String(e) };
    }
    testing = false;
  }

  async function save() {
    if (!draft) return;
    saving = true;
    try {
      const familyChanged = draft.family !== models.config?.family;
      await saveModels($state.snapshot(draft));
      savedNote = familyChanged
        ? "Saved. Steps, CFG and size were reset to this family's defaults. The next image loads the new models."
        : "Saved. The next image loads the new models.";
      refreshCheck();
    } catch (e) {
      savedNote = String(e);
    }
    saving = false;
  }

  onMount(() => {
    if (comfyState.connected) refresh();
  });
  // Load lists when ComfyUI comes online while the panel is open.
  $effect(() => {
    if (comfyState.connected && !lists && !loading) refresh();
  });
  $effect(() => {
    if (!draft && models.config) draft = { ...models.config };
  });
</script>

{#if draft}
  <div class="models">
    <label class="field">
      <span>Model family</span>
      <select value={draft.family} onchange={(e) => changeFamily((e.target as HTMLSelectElement).value)}>
        {#each models.families as f}<option value={f.id}>{f.label}</option>{/each}
      </select>
    </label>
    {#if fam}<p class="hint">{fam.description}</p>{/if}

    {#if fam?.loaders === "checkpoint"}
      <label class="field">
        <span>Checkpoint</span>
        <select bind:value={draft.ckpt} disabled={offline}>
          {#each options(lists?.checkpoints, draft.ckpt) as o}<option value={o.v}>{o.v}{o.missing ? " (not installed)" : ""}</option>{/each}
        </select>
      </label>
    {:else}
      <label class="field">
        <span>Diffusion model</span>
        <select bind:value={draft.unet} disabled={offline}>
          {#each options(lists?.diffusionModels, draft.unet) as o}<option value={o.v}>{o.v}{o.missing ? " (not installed)" : ""}</option>{/each}
        </select>
      </label>
      <label class="field">
        <span>Text encoder</span>
        <select bind:value={draft.clip} disabled={offline}>
          {#each options(lists?.textEncoders, draft.clip) as o}<option value={o.v}>{o.v}{o.missing ? " (not installed)" : ""}</option>{/each}
        </select>
      </label>
      <label class="field">
        <span>Encoder type</span>
        <select bind:value={draft.clipType} disabled={offline}>
          {#each options(lists?.clipTypes, draft.clipType) as o}<option value={o.v}>{o.v}</option>{/each}
        </select>
      </label>
      <label class="field">
        <span>VAE</span>
        <select bind:value={draft.vae} disabled={offline}>
          {#each options(lists?.vae, draft.vae) as o}<option value={o.v}>{o.v}{o.missing ? " (not installed)" : ""}</option>{/each}
        </select>
      </label>
    {/if}

    {#if fam && (!fam.supportsEdit || !fam.supportsVariation)}
      <p class="hint warn">
        This family doesn't support {[!fam.supportsEdit && "Edit", !fam.supportsVariation && "Variations"].filter(Boolean).join(" or ")}; those buttons are disabled while it's active.
      </p>
    {/if}
    {#if offline}
      <p class="hint warn">{comfyState.connected ? (listError ?? "Loading installed models…") : "ComfyUI is offline: showing the saved selection. Start ComfyUI to change it."}</p>
    {/if}

    <div class="row">
      <button class="btn ghost" onclick={refresh} disabled={!comfyState.connected || loading}>{loading ? "Refreshing…" : "Refresh"}</button>
      <button class="btn" onclick={test} disabled={!comfyState.connected || testing}>{testing ? "Testing…" : "Test"}</button>
      <button class="btn primary" onclick={save} disabled={!dirty || saving}>Save</button>
    </div>
    {#if testResult}<p class="hint" class:good={testResult.ok} class:bad={!testResult.ok}>{testResult.text}</p>{/if}
    {#if savedNote && !dirty}<p class="hint">{savedNote}</p>{/if}
    <p class="hint">Saving doesn't free or load anything. The first image after a change loads the new models, which can take a minute from a slow drive. Test runs a tiny 256×256 image with the selection above before you save.</p>
  </div>
{/if}

<style>
  .models {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 3px;
    font-size: 12px;
    color: var(--muted);
  }
  select {
    font: inherit;
    font-size: 12px;
    color: var(--text);
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 5px 6px;
    max-width: 100%;
  }
  select:disabled {
    opacity: 0.6;
  }
  .row {
    display: flex;
    gap: 6px;
    margin-top: 4px;
  }
  .btn {
    font: inherit;
    font-size: 13px;
    color: var(--text);
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 6px 12px;
    cursor: pointer;
  }
  .btn:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .btn.ghost {
    background: transparent;
  }
  .btn.primary {
    background: var(--accent-2);
    border-color: var(--accent-2);
    color: white;
    font-weight: 600;
  }
  .hint {
    font-size: 12px;
    color: var(--muted);
    margin: 2px 0;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .good {
    color: #4cd38a;
  }
  .bad,
  .warn {
    color: #f0b440;
  }
</style>
