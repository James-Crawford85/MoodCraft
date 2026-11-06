<script lang="ts">
  import { onMount } from "svelte";
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import Canvas from "$lib/Canvas.svelte";
  import { board, bounds } from "$lib/board.svelte";
  import { api } from "$lib/api";
  import { comfyState, editImage, generateFromPrompt, initGenEvents, refreshCheck, reroll, resumePending } from "$lib/gen.svelte";
  import { FRAME_COLORS, NOTE_COLORS, type ImageCard } from "$lib/types";

  let canvas: Canvas;
  let prompt = $state("");
  let editPrompt = $state("");
  let showSettings = $state(false);
  let toasts = $state<{ id: number; text: string; kind: "info" | "error" }[]>([]);
  let dropping = $state(false);
  let urlDraft = $state(board.settings.comfyUrl);

  function toast(text: string, kind: "info" | "error" = "info") {
    const id = Math.random();
    toasts.push({ id, text, kind });
    setTimeout(() => (toasts = toasts.filter((t) => t.id !== id)), kind === "error" ? 7000 : 3000);
  }

  // --- selection-derived UI --------------------------------------------------
  const sel = $derived(board.selection);
  const single = $derived(sel.length === 1 ? sel[0] : null);
  const selImage = $derived(single?.type === "image" && single.status === "ready" ? (single as ImageCard) : null);
  const toolbarPos = $derived.by(() => {
    if (!sel.length) return null;
    const b = bounds(sel);
    const z = board.camera.zoom;
    const x = board.camera.x + (b.x + b.w / 2) * z;
    const y = board.camera.y + b.y * z;
    return { x: Math.max(160, Math.min(window.innerWidth - 160, x)), y: Math.max(64, y - 14) };
  });

  // --- persistence -----------------------------------------------------------
  let saveTimer: ReturnType<typeof setTimeout> | undefined;
  async function saveNow() {
    clearTimeout(saveTimer);
    if (!board.dir) return;
    try {
      await api.boardSave(board.dir, board.serialize());
      board.dirty = false;
      board.lastSaved = Date.now();
      board.saveError = null;
    } catch (e) {
      board.saveError = String(e);
      toast(`Save failed: ${e}`, "error");
    }
  }
  // Autosave: any change marks dirty; flush 1.2s after the last one.
  $effect(() => {
    if (board.dirty) {
      clearTimeout(saveTimer);
      saveTimer = setTimeout(saveNow, 1200);
    }
  });
  // Camera changes are saved lazily too (but don't count as edits).
  $effect(() => {
    void board.camera;
    clearTimeout(camTimer);
    camTimer = setTimeout(() => board.dir && !board.dirty && api.boardSave(board.dir, board.serialize()).catch(() => {}), 2500);
  });
  let camTimer: ReturnType<typeof setTimeout> | undefined;

  function remember(dir: string, scratch: boolean) {
    try {
      localStorage.setItem("bc.lastBoard", JSON.stringify({ dir, scratch }));
    } catch {}
  }

  async function newBoard() {
    await saveNow();
    const dir = await api.boardNew();
    board.load(dir, null, true);
    await saveNow();
    remember(dir, true);
  }

  async function openBoard(dir?: string) {
    if (!dir) {
      const picked = await openDialog({ directory: true, title: "Open board folder (contains board.json)" });
      if (!picked || Array.isArray(picked)) return;
      dir = picked;
    }
    await saveNow();
    try {
      const json = await api.boardOpen(dir);
      board.load(dir, json, false);
      resumePending();
      remember(dir, false);
      toast(`Opened ${board.name}`);
    } catch (e) {
      toast(String(e), "error");
    }
  }

  async function saveAs() {
    const picked = await openDialog({ directory: true, title: "Choose an (empty) folder for this board" });
    if (!picked || Array.isArray(picked)) return;
    if (Object.keys(board.jobs).length) toast("Images still generating will land in the old board folder.", "error");
    try {
      await api.boardSaveAs(board.dir, picked, board.serialize());
      board.dir = picked;
      board.isScratch = false;
      board.dirty = false;
      board.lastSaved = Date.now();
      remember(picked, false);
      toast(`Saved to ${picked}`);
    } catch (e) {
      toast(String(e), "error");
    }
  }

  // --- import ------------------------------------------------------------------
  async function importPaths(paths: string[], at?: { x: number; y: number }) {
    const imgs = paths.filter((p) => /\.(png|jpe?g|webp|gif|bmp)$/i.test(p));
    if (!imgs.length) return toast("Only image files can be dropped (png, jpg, webp, gif, bmp)", "error");
    board.checkpoint();
    const ids: string[] = [];
    for (const p of imgs) {
      try {
        const r = await api.importImage(board.dir, p);
        ids.push(board.addImage(r.asset, r.width, r.height, at).id);
      } catch (e) {
        toast(String(e), "error");
      }
    }
    board.selected = new Set(ids);
    board.touch();
  }

  async function onPaste(e: ClipboardEvent) {
    const t = e.target as HTMLElement;
    if (t.tagName === "INPUT" || t.tagName === "TEXTAREA") return;
    const items = [...(e.clipboardData?.items ?? [])].filter((i) => i.type.startsWith("image/"));
    if (!items.length) {
      const text = e.clipboardData?.getData("text/plain");
      if (text) board.addNote(undefined, undefined, text);
      return;
    }
    e.preventDefault();
    board.checkpoint();
    for (const it of items) {
      const f = it.getAsFile();
      if (!f) continue;
      const ext = f.type.split("/")[1]?.replace("jpeg", "jpg") ?? "png";
      try {
        const r = await api.importImageBytes(board.dir, new Uint8Array(await f.arrayBuffer()), ext);
        board.selected = new Set([board.addImage(r.asset, r.width, r.height).id]);
      } catch (err) {
        toast(String(err), "error");
      }
    }
  }

  // --- actions -----------------------------------------------------------------
  async function freeGpu() {
    try {
      await api.free();
      toast("Asked ComfyUI to unload its models. The next generation will reload them.");
    } catch (e) {
      toast(String(e), "error");
    }
  }

  function submitPrompt() {
    if (!prompt.trim()) return;
    if (!comfyState.connected) {
      toast("ComfyUI is not running. Start it from the Comfy Desktop app, then try again.", "error");
      return;
    }
    if (selImage) editImage(selImage, prompt, board.settings.count);
    else generateFromPrompt(prompt);
    prompt = "";
  }

  function onKeyDown(e: KeyboardEvent) {
    const t = e.target as HTMLElement;
    const typing = t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable;
    const mod = e.ctrlKey || e.metaKey;
    if (mod && e.key.toLowerCase() === "s") { e.preventDefault(); e.shiftKey ? saveAs() : saveNow().then(() => toast("Saved")); return; }
    if (mod && e.key.toLowerCase() === "o") { e.preventDefault(); openBoard(); return; }
    if (typing) return;
    if (mod && e.key.toLowerCase() === "z") { e.preventDefault(); e.shiftKey ? board.redo() : board.undo(); return; }
    if (mod && e.key.toLowerCase() === "y") { e.preventDefault(); board.redo(); return; }
    if (mod && e.key.toLowerCase() === "a") { e.preventDefault(); board.selected = new Set(board.cards.map((c) => c.id)); return; }
    if (mod && e.key.toLowerCase() === "d") { e.preventDefault(); board.duplicateSelected(); return; }
    if (mod && e.key === "0") { e.preventDefault(); board.fitTo(); return; }
    if (e.key === "Delete" || e.key === "Backspace") { board.deleteSelected(); return; }
    if (e.key === "Escape") { board.selected = new Set(); showSettings = false; return; }
    if (mod) return;
    if (e.key === "n" || e.key === "N") { e.preventDefault(); canvas.editCard(board.addNote().id); }
    else if (e.key === "f" || e.key === "F") { e.preventDefault(); board.addFrame(); }
    else if (e.key === "g" || e.key === "G") { board.settings.snap = !board.settings.snap; board.saveSettings(); toast(`Snap to grid ${board.settings.snap ? "on" : "off"}`); }
    else if (e.key === "1") { board.camera = { ...board.camera, zoom: 1 }; }
    else if (e.key === "/" ) { e.preventDefault(); document.getElementById("prompt")?.focus(); }
  }

  function zoomBy(f: number) {
    board.zoomAt(window.innerWidth / 2, window.innerHeight / 2, f);
  }

  async function applyUrl() {
    board.settings.comfyUrl = urlDraft.trim();
    board.saveSettings();
    try {
      await api.comfySetUrl(board.settings.comfyUrl);
      toast("ComfyUI URL updated; reconnecting…");
    } catch (e) {
      toast(String(e), "error");
    }
  }

  onMount(() => {
    let unlisten: (() => void) | undefined;
    let unDrop: (() => void) | undefined;
    (async () => {
      unlisten = await initGenEvents();
      if (board.settings.comfyUrl !== "http://127.0.0.1:8188") await api.comfySetUrl(board.settings.comfyUrl).catch(() => {});
      const [connected, url] = await api.comfyStatus();
      comfyState.connected = connected;
      comfyState.url = url;
      refreshCheck();

      // Reopen the last board, else start a scratch board.
      let last: { dir: string; scratch: boolean } | null = null;
      try { last = JSON.parse(localStorage.getItem("bc.lastBoard") ?? "null"); } catch {}
      let opened = false;
      if (last?.dir) {
        try {
          board.load(last.dir, await api.boardOpen(last.dir), last.scratch);
          resumePending();
          opened = true;
        } catch { /* folder moved or deleted */ }
      }
      if (!opened) await newBoard();

      unDrop = await getCurrentWebview().onDragDropEvent((ev) => {
        const p = ev.payload;
        if (p.type === "enter" || p.type === "over") dropping = true;
        else if (p.type === "leave") dropping = false;
        else if (p.type === "drop") {
          dropping = false;
          const dpr = window.devicePixelRatio || 1;
          const at = board.screenToWorld(p.position.x / dpr, p.position.y / dpr);
          importPaths(p.paths, at);
        }
      });
    })();
    const flush = () => { if (board.dirty) saveNow(); };
    window.addEventListener("beforeunload", flush);
    return () => { unlisten?.(); unDrop?.(); window.removeEventListener("beforeunload", flush); };
  });

  const pendingCount = $derived(Object.keys(board.jobs).length);
  const savedLabel = $derived(board.saveError ? "Save failed" : board.dirty ? "Saving…" : board.lastSaved ? "Saved" : "");
</script>

<svelte:window onkeydown={onKeyDown} onpaste={onPaste} />

<div class="app">
  <Canvas bind:this={canvas} />

  {#if dropping}<div class="dropzone">Drop images to add them to the board</div>{/if}

  <!-- Top bar -->
  <header class="topbar">
    <div class="brand"><span class="logo">◆</span> BoardCreate</div>
    <div class="board-name" title={board.dir}>{board.name}<span class="saved">{savedLabel}</span></div>
    <div class="spacer"></div>
    <button class="btn ghost" onclick={newBoard} title="New board">New</button>
    <button class="btn ghost" onclick={() => openBoard()} title="Open board folder (Ctrl+O)">Open…</button>
    <button class="btn ghost" onclick={saveAs} title="Save board to a folder (Ctrl+Shift+S)">{board.isScratch ? "Save to folder…" : "Save as…"}</button>
    <button class="btn ghost" onclick={() => revealItemInDir(board.dir + (board.dir.includes("\\") ? "\\" : "/") + "board.json").catch(() => {})} title="Show board folder">Folder</button>
    <div class="sep"></div>
    <button
      class="status"
      class:ok={comfyState.connected && !comfyState.problems.length}
      class:warn={comfyState.connected && comfyState.problems.length > 0}
      onclick={() => (showSettings = !showSettings)}
      title={comfyState.connected ? (comfyState.problems.length ? comfyState.problems.join("\n") : `Connected: ${comfyState.url}${comfyState.device ? "\n" + comfyState.device : ""}`) : "ComfyUI not reachable. Start the Comfy Desktop app."}
    >
      <span class="dot"></span>
      {comfyState.connected ? "ComfyUI" : "ComfyUI offline"}
      {#if pendingCount}<span class="badge">{pendingCount}</span>{/if}
    </button>
    <button class="btn ghost" onclick={freeGpu} disabled={!comfyState.connected || pendingCount > 0} title="Unload ComfyUI's models to free VRAM for Blender etc. (also done automatically when the app closes)">Free GPU</button>
    <button class="btn ghost icon" onclick={() => (showSettings = !showSettings)} title="Settings">⚙</button>
  </header>

  {#if !comfyState.connected}
    <div class="offline" data-action>
      <strong>ComfyUI not running</strong> at {comfyState.url || board.settings.comfyUrl}. Start it from the Comfy Desktop app; the board reconnects automatically. You can still arrange, add notes and drop images.
    </div>
  {/if}

  <!-- Contextual toolbar for the selection -->
  {#if toolbarPos && sel.length}
    <div class="ctx" style="left:{toolbarPos.x}px; top:{toolbarPos.y}px;" data-action>
      {#if selImage}
        <button class="btn" disabled={!comfyState.connected} onclick={() => editImage(selImage, "", board.settings.count)} title="Qwen-Image-Edit variation(s) of this image">✦ Variations</button>
        <form class="inline" onsubmit={(e) => { e.preventDefault(); if (editPrompt.trim()) { editImage(selImage, editPrompt); editPrompt = ""; } }}>
          <input placeholder="Edit: e.g. make it night time" bind:value={editPrompt} onkeydown={(e) => e.stopPropagation()} />
          <button class="btn" type="submit" disabled={!editPrompt.trim() || !comfyState.connected}>Edit</button>
        </form>
        {#if selImage.gen}<button class="btn ghost" onclick={() => reroll(selImage)} title="Same prompt, new seed">↻</button>{/if}
        {#if selImage.prompt}<button class="btn ghost" onclick={() => { prompt = selImage.prompt ?? ""; document.getElementById("prompt")?.focus(); }} title={selImage.prompt}>Prompt</button>{/if}
      {:else if single?.type === "note"}
        {#each NOTE_COLORS as c}
          <button class="swatch" class:on={single.color === c} style="background:{c}" aria-label="color" onclick={() => { board.checkpoint(); if (single.type === "note") single.color = c; }}></button>
        {/each}
        <button class="btn ghost" onclick={() => canvas.editCard(single.id)}>Edit text</button>
      {:else if single?.type === "frame"}
        {#each FRAME_COLORS as c}
          <button class="swatch" class:on={single.color === c} style="background:{c}" aria-label="color" onclick={() => { board.checkpoint(); if (single.type === "frame") single.color = c; }}></button>
        {/each}
        <button class="btn ghost" onclick={() => canvas.editCard(single.id)}>Rename</button>
      {/if}
      {#if sel.length > 1 || single?.type !== "frame"}
        <button class="btn ghost" onclick={() => board.addFrame()} title="Group in a frame (F)">Frame</button>
      {/if}
      <button class="btn ghost" onclick={() => board.bringToFront()} title="Bring to front">⤒</button>
      <button class="btn ghost danger" onclick={() => board.deleteSelected()} title="Delete (Del)">🗑</button>
    </div>
  {/if}

  <!-- Bottom prompt bar -->
  <div class="promptbar">
    {#if selImage}
      <div class="chip" title="Your prompt will edit the selected image with Qwen-Image-Edit">
        {#if selImage.asset}<span class="thumb-dot"></span>{/if}Editing selected image
        <button class="x" onclick={() => (board.selected = new Set())} aria-label="Clear selection">×</button>
      </div>
    {/if}
    <textarea
      id="prompt"
      rows="1"
      placeholder={selImage ? "Describe the change… (Enter)" : "Describe an image to generate… (Enter)  ·  / to focus"}
      bind:value={prompt}
      onkeydown={(e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); submitPrompt(); } if (e.key === "Escape") (e.target as HTMLElement).blur(); }}
    ></textarea>
    <select bind:value={board.settings.aspect} onchange={() => board.saveSettings()} title="Aspect ratio (text-to-image)" disabled={!!selImage}>
      {#each ["1:1", "4:3", "3:4", "16:9", "9:16"] as a}<option value={a}>{a}</option>{/each}
    </select>
    <select bind:value={board.settings.count} onchange={() => board.saveSettings()} title="How many images">
      {#each [1, 2, 3, 4] as n}<option value={n}>×{n}</option>{/each}
    </select>
    <button class="btn primary" onclick={submitPrompt} disabled={!prompt.trim()}>{selImage ? "Edit" : "Generate"}</button>
  </div>

  <!-- Zoom controls -->
  <div class="zoom">
    <button class="btn ghost icon" onclick={() => zoomBy(1 / 1.2)} title="Zoom out">−</button>
    <button class="btn ghost" onclick={() => (board.camera = { ...board.camera, zoom: 1 })} title="Reset zoom (1)">{Math.round(board.camera.zoom * 100)}%</button>
    <button class="btn ghost icon" onclick={() => zoomBy(1.2)} title="Zoom in">+</button>
    <button class="btn ghost" onclick={() => board.fitTo()} title="Fit all (Ctrl+0)">Fit</button>
    <button class="btn ghost" class:active={board.settings.snap} onclick={() => { board.settings.snap = !board.settings.snap; board.saveSettings(); }} title="Snap to grid (G)">Snap</button>
  </div>

  {#if showSettings}
    <aside class="settings" data-action>
      <div class="sh"><h3>Settings</h3><button class="btn ghost icon" onclick={() => (showSettings = false)}>×</button></div>
      <section>
        <h4>ComfyUI</h4>
        <div class="row">
          <input bind:value={urlDraft} />
          <button class="btn" onclick={applyUrl}>Apply</button>
        </div>
        <p class="hint">
          {#if comfyState.connected}Connected{comfyState.device ? ` · ${comfyState.device}` : ""}{#if comfyState.queue !== undefined} · queue {comfyState.queue}{/if}
          {:else}Not reachable. Start ComfyUI from the Comfy Desktop app.{/if}
        </p>
        {#if comfyState.problems.length}
          <ul class="problems">{#each comfyState.problems as p}<li>{p}</li>{/each}</ul>
        {:else if comfyState.connected && comfyState.checked}
          <p class="hint good">All required nodes and models found.</p>
        {/if}
        <button class="btn ghost" onclick={refreshCheck}>Re-check</button>
      </section>
      <section>
        <h4>Text → image <span class="sub">Flux3 Klein base</span></h4>
        <label>Size (long side ≈) <input type="number" min="256" max="1536" step="64" bind:value={board.settings.t2iSize} onchange={() => board.saveSettings()} /></label>
        <label>Steps <input type="number" min="1" max="80" bind:value={board.settings.t2iSteps} onchange={() => board.saveSettings()} /></label>
        <label>CFG <input type="number" min="1" max="10" step="0.1" bind:value={board.settings.t2iCfg} onchange={() => board.saveSettings()} /></label>
      </section>
      <section>
        <h4>Edit / variations <span class="sub">Qwen-Image-Edit 2509</span></h4>
        <label>Steps <input type="number" min="1" max="60" bind:value={board.settings.editSteps} onchange={() => board.saveSettings()} /></label>
        <label>CFG <input type="number" min="1" max="10" step="0.1" bind:value={board.settings.editCfg} onchange={() => board.saveSettings()} /></label>
        <p class="hint">Edits run at ~1 MP (FluxKontextImageScale); expect them to be slower than thumbnails.</p>
      </section>
      <section>
        <h4>Shortcuts</h4>
        <p class="hint keys">
          Space/middle/right-drag: pan · Wheel: zoom · Drag empty: select · Shift-click: add to selection<br />
          N note · F frame · G snap · Del delete · Ctrl+D duplicate · Ctrl+Z/Y undo/redo · Ctrl+0 fit · Ctrl+S save · Ctrl+V paste image/text · double-click empty: note
        </p>
      </section>
    </aside>
  {/if}

  <div class="toasts">
    {#each toasts as t (t.id)}<div class="toast {t.kind}">{t.text}</div>{/each}
  </div>
</div>

<style>
  :global(:root) {
    --bg: #121316;
    --grid: #2a2d34;
    --surface: #1c1e23;
    --surface-2: #25282f;
    --border: #343842;
    --text: #e8e9ec;
    --muted: #9aa0ab;
    --accent: #7c9cff;
    --accent-2: #5b7cf0;
    --danger: #ff7a85;
    color-scheme: dark;
    font-family: "Segoe UI Variable", "Segoe UI", Inter, system-ui, sans-serif;
    font-size: 14px;
    color: var(--text);
    background: var(--bg);
  }
  :global(html, body) {
    margin: 0;
    height: 100%;
    overflow: hidden;
    background: var(--bg);
  }
  .app {
    position: fixed;
    inset: 0;
  }
  .topbar {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 48px;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 12px;
    background: color-mix(in srgb, var(--surface) 88%, transparent);
    backdrop-filter: blur(10px);
    border-bottom: 1px solid var(--border);
    z-index: 10;
  }
  .brand {
    font-weight: 700;
    letter-spacing: 0.02em;
    margin-right: 12px;
  }
  .logo {
    color: var(--accent);
  }
  .board-name {
    color: var(--muted);
    max-width: 360px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .saved {
    margin-left: 10px;
    font-size: 12px;
    opacity: 0.7;
  }
  .spacer {
    flex: 1;
  }
  .sep {
    width: 1px;
    height: 22px;
    background: var(--border);
    margin: 0 4px;
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
    white-space: nowrap;
    transition: background 0.12s, border-color 0.12s;
  }
  .btn:hover:not(:disabled) {
    background: #2e323a;
    border-color: #454a56;
  }
  .btn:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .btn.ghost {
    background: transparent;
    border-color: transparent;
  }
  .btn.ghost:hover:not(:disabled) {
    background: var(--surface-2);
  }
  .btn.ghost.active {
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 14%, transparent);
  }
  .btn.icon {
    padding: 6px 9px;
  }
  .btn.primary {
    background: var(--accent-2);
    border-color: var(--accent-2);
    color: white;
    font-weight: 600;
  }
  .btn.primary:hover:not(:disabled) {
    background: var(--accent);
  }
  .btn.danger:hover {
    color: var(--danger);
  }
  .status {
    display: flex;
    align-items: center;
    gap: 7px;
    font: inherit;
    font-size: 13px;
    color: var(--muted);
    background: transparent;
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 4px 12px;
    cursor: pointer;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--danger);
  }
  .status.ok .dot {
    background: #4cd38a;
  }
  .status.warn .dot {
    background: #f0b440;
  }
  .status.ok {
    color: var(--text);
  }
  .badge {
    background: var(--accent-2);
    color: white;
    border-radius: 999px;
    font-size: 11px;
    padding: 0 7px;
  }
  .ctx {
    position: absolute;
    transform: translate(-50%, -100%);
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 5px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 10px 30px rgba(0, 0, 0, 0.45);
    z-index: 9;
  }
  .inline {
    display: flex;
    gap: 4px;
  }
  .ctx input,
  .settings input {
    font: inherit;
    font-size: 13px;
    color: var(--text);
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 6px 10px;
    outline: none;
  }
  .ctx input {
    width: 230px;
  }
  .ctx input:focus,
  .settings input:focus {
    border-color: var(--accent);
  }
  .swatch {
    width: 20px;
    height: 20px;
    border-radius: 50%;
    border: 2px solid transparent;
    cursor: pointer;
    padding: 0;
  }
  .swatch.on {
    border-color: white;
  }
  .promptbar {
    position: absolute;
    left: 50%;
    bottom: 22px;
    transform: translateX(-50%);
    width: min(780px, calc(100% - 300px));
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 8px 8px 14px;
    background: color-mix(in srgb, var(--surface) 94%, transparent);
    backdrop-filter: blur(12px);
    border: 1px solid var(--border);
    border-radius: 16px;
    box-shadow: 0 16px 40px rgba(0, 0, 0, 0.5);
    z-index: 10;
  }
  .promptbar textarea {
    flex: 1;
    font: inherit;
    font-size: 14px;
    color: var(--text);
    background: transparent;
    border: none;
    outline: none;
    resize: none;
    max-height: 120px;
    field-sizing: content;
    padding: 6px 0;
  }
  .promptbar select {
    font: inherit;
    font-size: 13px;
    color: var(--text);
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 5px 6px;
  }
  .chip {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    border-radius: 999px;
    padding: 3px 4px 3px 10px;
    white-space: nowrap;
  }
  .chip .x {
    border: none;
    background: transparent;
    color: inherit;
    cursor: pointer;
    font-size: 14px;
  }
  .zoom {
    position: absolute;
    right: 14px;
    bottom: 22px;
    display: flex;
    gap: 2px;
    padding: 4px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 12px;
    z-index: 10;
  }
  .settings {
    position: absolute;
    top: 56px;
    right: 12px;
    width: 340px;
    max-height: calc(100% - 140px);
    overflow: auto;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 14px;
    box-shadow: 0 16px 40px rgba(0, 0, 0, 0.5);
    padding: 6px 16px 16px;
    z-index: 11;
  }
  .sh {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .settings h3 {
    margin: 8px 0;
    font-size: 15px;
  }
  .settings h4 {
    margin: 14px 0 8px;
    font-size: 13px;
    color: var(--muted);
    font-weight: 600;
  }
  .sub {
    font-weight: 400;
    opacity: 0.7;
    margin-left: 6px;
  }
  .settings label {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin: 6px 0;
    font-size: 13px;
  }
  .settings label input {
    width: 90px;
  }
  .row {
    display: flex;
    gap: 6px;
  }
  .row input {
    flex: 1;
  }
  .hint {
    font-size: 12px;
    color: var(--muted);
    margin: 6px 0;
  }
  .hint.good {
    color: #4cd38a;
  }
  .keys {
    line-height: 1.7;
  }
  .problems {
    font-size: 12px;
    color: #f0b440;
    padding-left: 18px;
    margin: 6px 0;
  }
  .dropzone {
    position: absolute;
    inset: 60px 12px 12px;
    border: 2px dashed var(--accent);
    border-radius: 18px;
    background: color-mix(in srgb, var(--accent) 7%, transparent);
    display: grid;
    place-items: center;
    font-size: 18px;
    color: var(--accent);
    pointer-events: none;
    z-index: 20;
  }
  .offline {
    position: absolute;
    top: 56px;
    left: 50%;
    transform: translateX(-50%);
    max-width: 720px;
    padding: 8px 14px;
    font-size: 13px;
    color: #ffd7dc;
    background: #2a1a1d;
    border: 1px solid #6b2a33;
    border-radius: 10px;
    z-index: 9;
  }
  .toasts {
    position: absolute;
    left: 50%;
    bottom: 96px;
    transform: translateX(-50%);
    display: flex;
    flex-direction: column;
    gap: 6px;
    align-items: center;
    z-index: 30;
    pointer-events: none;
  }
  .toast {
    background: var(--surface-2);
    border: 1px solid var(--border);
    padding: 8px 14px;
    border-radius: 10px;
    font-size: 13px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
    max-width: 640px;
  }
  .toast.error {
    border-color: #6b2a33;
    color: #ffc0c7;
  }
</style>
