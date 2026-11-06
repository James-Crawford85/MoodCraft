<script lang="ts">
  // Paint-a-mask editor: brush over the part of an image to change, describe
  // the change, and the result lands as a new card next to the original.
  import { onMount, untrack } from "svelte";
  import { api, assetUrl } from "./api";
  import { board } from "./board.svelte";
  import { comfyState, inpaintImage } from "./gen.svelte";
  import type { ImageCard } from "./types";

  let { card, onClose }: { card: ImageCard; onClose: () => void } = $props();

  // The editor works on the card as it was when opened.
  const natW = untrack(() => card.naturalW ?? Math.round(card.w * 3));
  const natH = untrack(() => card.naturalH ?? Math.round(card.h * 3));

  let paint: HTMLCanvasElement;
  let wrap: HTMLDivElement;
  let promptEl: HTMLInputElement;
  /** Brush diameter in image pixels. */
  let size = $state(Math.round(Math.max(natW, natH) / 14));
  let erasing = $state(false);
  let prompt = $state("");
  let count = $state(1);
  let painted = $state(false);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let cursor = $state<{ x: number; y: number } | null>(null);
  /** Displayed size of the image (CSS px), to scale the brush cursor. */
  let shownW = $state(1);
  let stageW = $state(800), stageH = $state(600);
  /** Image fitted inside the stage (never upscaled past 2×). */
  const fit = $derived.by(() => {
    const k = Math.min((stageW - 48) / natW, stageH / natH, 2);
    return { w: Math.max(1, Math.floor(natW * k)), h: Math.max(1, Math.floor(natH * k)) };
  });

  let drawing = false;
  let last: { x: number; y: number } | null = null;
  const undoStack: ImageData[] = [];

  const ctx = () => paint.getContext("2d")!;

  function toImage(e: PointerEvent) {
    const r = paint.getBoundingClientRect();
    return { x: ((e.clientX - r.left) / r.width) * natW, y: ((e.clientY - r.top) / r.height) * natH };
  }

  function stroke(a: { x: number; y: number }, b: { x: number; y: number }) {
    const c = ctx();
    c.globalCompositeOperation = erasing ? "destination-out" : "source-over";
    c.strokeStyle = "rgb(255, 64, 64)";
    c.lineWidth = size;
    c.lineCap = "round";
    c.lineJoin = "round";
    c.beginPath();
    c.moveTo(a.x, a.y);
    c.lineTo(b.x, b.y);
    c.stroke();
  }

  function down(e: PointerEvent) {
    if (e.button !== 0) return;
    undoStack.push(ctx().getImageData(0, 0, natW, natH));
    if (undoStack.length > 30) undoStack.shift();
    drawing = true;
    paint.setPointerCapture(e.pointerId);
    last = toImage(e);
    stroke(last, last);
    if (!erasing) painted = true;
  }
  function move(e: PointerEvent) {
    const r = wrap.getBoundingClientRect();
    cursor = { x: e.clientX - r.left, y: e.clientY - r.top };
    if (!drawing || !last) return;
    if (e.buttons === 0) return up(e);
    const p = toImage(e);
    stroke(last, p);
    last = p;
  }
  function up(e: PointerEvent) {
    drawing = false;
    last = null;
    if (paint.hasPointerCapture(e.pointerId)) paint.releasePointerCapture(e.pointerId);
    painted = hasPaint();
  }

  function hasPaint() {
    const d = ctx().getImageData(0, 0, natW, natH).data;
    for (let i = 3; i < d.length; i += 16) if (d[i] > 0) return true;
    return false;
  }
  function clear() {
    undoStack.push(ctx().getImageData(0, 0, natW, natH));
    ctx().clearRect(0, 0, natW, natH);
    painted = false;
  }
  function undo() {
    const s = undoStack.pop();
    if (s) { ctx().putImageData(s, 0, 0); painted = hasPaint(); }
  }

  /** White-on-black mask at the image's size, edges softened a little. */
  async function maskBytes(): Promise<Uint8Array> {
    const m = document.createElement("canvas");
    m.width = natW;
    m.height = natH;
    const c = m.getContext("2d")!;
    // Paint → white silhouette
    const white = document.createElement("canvas");
    white.width = natW;
    white.height = natH;
    const w = white.getContext("2d")!;
    w.drawImage(paint, 0, 0);
    w.globalCompositeOperation = "source-in";
    w.fillStyle = "#fff";
    w.fillRect(0, 0, natW, natH);
    c.fillStyle = "#000";
    c.fillRect(0, 0, natW, natH);
    c.filter = `blur(${Math.max(2, Math.round(Math.max(natW, natH) / 200))}px)`;
    c.drawImage(white, 0, 0);
    const blob: Blob = await new Promise((res, rej) => m.toBlob((b) => (b ? res(b) : rej(new Error("mask export failed"))), "image/png"));
    return new Uint8Array(await blob.arrayBuffer());
  }

  async function submit() {
    if (!prompt.trim() || !painted || busy) return;
    if (!comfyState.connected) { error = "ComfyUI is not running."; return; }
    busy = true;
    error = null;
    try {
      const m = await api.importImageBytes(board.dir, await maskBytes(), "png");
      inpaintImage(card, m.asset, prompt, count);
      onClose();
    } catch (e) {
      error = String(e);
      busy = false;
    }
  }

  function onKey(e: KeyboardEvent) {
    const typing = e.target === promptEl;
    // Keep the board's shortcuts (Delete, N, F, Ctrl+Z…) from acting underneath.
    e.stopImmediatePropagation();
    if (e.key === "Escape") { e.preventDefault(); onClose(); return; }
    if (typing) return;
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "z") { e.preventDefault(); undo(); }
    else if (e.key === "[") size = Math.max(4, Math.round(size / 1.25));
    else if (e.key === "]") size = Math.min(800, Math.round(size * 1.25));
    else if (e.key === "e" || e.key === "E") erasing = !erasing;
    else if (e.key === "b" || e.key === "B") erasing = false;
  }
  const swallow = (e: KeyboardEvent) => e.stopImmediatePropagation();

  onMount(() => {
    paint.width = natW;
    paint.height = natH;
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("keyup", swallow, true);
    const ro = new ResizeObserver(() => (shownW = paint.getBoundingClientRect().width || 1));
    ro.observe(paint);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("keyup", swallow, true);
      ro.disconnect();
    };
  });
</script>

<div class="inpaint" role="dialog" aria-modal="true" aria-label="Paint a mask to edit part of the image">
  <div class="bar top">
    <strong>Paint the area to change</strong>
    <div class="tools">
      <div class="seg">
        <button class:on={!erasing} onclick={() => (erasing = false)} title="Brush (B)">Brush</button>
        <button class:on={erasing} onclick={() => (erasing = true)} title="Eraser (E)">Eraser</button>
      </div>
      <label class="size" title="Brush size ([ and ])">
        Size
        <input type="range" min="4" max={Math.round(Math.max(natW, natH) / 3)} bind:value={size} />
      </label>
      <button class="btn ghost" onclick={undo} title="Undo stroke (Ctrl+Z)">Undo</button>
      <button class="btn ghost" onclick={clear}>Clear</button>
    </div>
    <button class="btn ghost x" onclick={onClose} title="Close (Esc)" aria-label="Close">✕</button>
  </div>

  <div class="stage" bind:clientWidth={stageW} bind:clientHeight={stageH}>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="imgwrap" bind:this={wrap} onpointerleave={() => (cursor = null)} style="width: {fit.w}px; height: {fit.h}px">
      <img src={assetUrl(board.dir, card.asset!)} alt={card.prompt ?? "image"} draggable="false" />
      <canvas bind:this={paint} class="paint" onpointerdown={down} onpointermove={move} onpointerup={up} onpointercancel={up}></canvas>
      {#if cursor}
        {@const d = (size / natW) * shownW}
        <div class="cursor" class:erase={erasing} style="left: {cursor.x}px; top: {cursor.y}px; width: {d}px; height: {d}px"></div>
      {/if}
    </div>
  </div>

  <form class="bar bottom" onsubmit={(e) => { e.preventDefault(); submit(); }}>
    <input bind:this={promptEl} bind:value={prompt} placeholder={painted ? "Describe what goes in the painted area, e.g. a golden sheriff badge" : "First paint over the part to change"} />
    <select bind:value={count} title="How many results">
      {#each [1, 2, 3, 4] as n}<option value={n}>×{n}</option>{/each}
    </select>
    <button class="btn primary" type="submit" disabled={!painted || !prompt.trim() || busy || !comfyState.connected}>{busy ? "Starting…" : "Inpaint"}</button>
  </form>
  {#if error}<p class="err">{error}</p>{/if}
</div>

<style>
  .inpaint {
    position: fixed;
    inset: 0;
    z-index: 1000;
    background: var(--bg);
    display: flex;
    flex-direction: column;
    color: var(--text);
    font-size: 13px;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 16px;
  }
  .top {
    justify-content: space-between;
  }
  .tools {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .seg {
    display: flex;
    border: 1px solid var(--border);
    border-radius: 8px;
    overflow: hidden;
  }
  .seg button {
    font: inherit;
    color: var(--muted);
    background: transparent;
    border: 0;
    padding: 6px 12px;
    cursor: pointer;
  }
  .seg button.on {
    background: var(--accent-2);
    color: white;
  }
  .size {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--muted);
  }
  .size input {
    accent-color: var(--accent);
  }
  .btn {
    font: inherit;
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
  .stage {
    flex: 1;
    min-height: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
  }
  .imgwrap {
    position: relative;
    flex: none;
    touch-action: none;
  }
  .imgwrap img,
  .paint {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    border-radius: 6px;
    user-select: none;
  }
  .imgwrap img {
    object-fit: contain;
  }
  .paint {
    opacity: 0.5;
    cursor: none;
  }
  .cursor {
    position: absolute;
    transform: translate(-50%, -50%);
    border: 1.5px solid white;
    border-radius: 50%;
    box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.6);
    pointer-events: none;
  }
  .cursor.erase {
    border-style: dashed;
  }
  .bottom {
    justify-content: center;
    padding-bottom: 18px;
  }
  .bottom input {
    width: min(620px, 60vw);
    font: inherit;
    color: var(--text);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 9px 12px;
    outline: none;
  }
  .bottom input:focus {
    border-color: var(--accent);
  }
  .bottom select {
    font: inherit;
    color: var(--text);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 7px 6px;
  }
  .err {
    color: #f0b440;
    text-align: center;
    margin: 0 0 12px;
  }
</style>
