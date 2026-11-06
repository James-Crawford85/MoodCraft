<script lang="ts">
  import { onMount } from "svelte";
  import { assetUrl } from "./api";
  import { board } from "./board.svelte";
  import type { ImageCard } from "./types";

  let { startId, onClose }: { startId: string; onClose: (lastId: string) => void } = $props();

  /** Ready images in reading order: rows top to bottom, left to right within a row.
   *  A card starts a new row when its top is below the middle of the current row's first card. */
  function readingOrder(): ImageCard[] {
    const imgs = board.cards.filter((c): c is ImageCard => c.type === "image" && c.status === "ready" && !!c.asset);
    imgs.sort((a, b) => a.y - b.y || a.x - b.x);
    const rows: ImageCard[][] = [];
    for (const c of imgs) {
      const row = rows[rows.length - 1];
      if (row && c.y < row[0].y + row[0].h / 2) row.push(c);
      else rows.push([c]);
    }
    return rows.flatMap((r) => r.sort((a, b) => a.x - b.x));
  }

  const order = readingOrder();
  let index = $state(Math.max(0, order.findIndex((c) => c.id === startId)));
  const current = $derived(order[index]);

  function step(d: number) {
    if (order.length) index = (index + d + order.length) % order.length;
  }
  const close = () => onClose(current?.id ?? startId);

  function onKey(e: KeyboardEvent) {
    // Capture phase on window: the board's own shortcuts (Delete, N, F, Space…) must not fire underneath.
    e.stopImmediatePropagation();
    if (e.key === "Escape") { e.preventDefault(); close(); }
    else if (e.key === "ArrowRight" || e.key === "ArrowDown") { e.preventDefault(); step(1); }
    else if (e.key === "ArrowLeft" || e.key === "ArrowUp") { e.preventDefault(); step(-1); }
    else if (e.key === "Home") { e.preventDefault(); index = 0; }
    else if (e.key === "End") { e.preventDefault(); index = order.length - 1; }
  }
  const swallow = (e: KeyboardEvent) => e.stopImmediatePropagation();

  onMount(() => {
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("keyup", swallow, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("keyup", swallow, true);
    };
  });
</script>

{#if current}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="lightbox" role="dialog" tabindex="-1" aria-modal="true" aria-label="Image viewer" onclick={(e) => { if (e.target === e.currentTarget) close(); }} onwheel={(e) => { e.preventDefault(); step(e.deltaY > 0 ? 1 : -1); }}>
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
    <img src={assetUrl(board.dir, current.asset!)} alt={current.prompt ?? "image"} draggable="false" onclick={(e) => e.stopPropagation()} />
    {#if order.length > 1}
      <button class="nav prev" onclick={() => step(-1)} title="Previous (←)" aria-label="Previous image">‹</button>
      <button class="nav next" onclick={() => step(1)} title="Next (→)" aria-label="Next image">›</button>
    {/if}
    <button class="x" onclick={close} title="Close (Esc)" aria-label="Close viewer">✕</button>
    <div class="caption">
      {#if current.prompt}<p class="prompt">{current.prompt}</p>{/if}
      <p class="meta">
        {index + 1} / {order.length}
        {#if current.naturalW && current.naturalH} · {current.naturalW}×{current.naturalH}{/if}
        {#if current.gen} · {current.gen.kind === "t2i" ? "generated" : current.gen.kind} · seed {current.gen.seed}{/if}
      </p>
    </div>
  </div>
{/if}

<style>
  .lightbox {
    position: fixed;
    inset: 0;
    z-index: 1000;
    background: rgba(8, 9, 11, 0.92);
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 48px 72px 16px;
    box-sizing: border-box;
    gap: 12px;
  }
  img {
    max-width: 100%;
    max-height: calc(100% - 64px);
    min-height: 0;
    object-fit: contain;
    border-radius: 6px;
    box-shadow: 0 10px 40px rgba(0, 0, 0, 0.6);
    user-select: none;
  }
  .caption {
    max-width: min(900px, 100%);
    text-align: center;
    color: var(--muted);
    font-size: 13px;
  }
  .caption p {
    margin: 2px 0;
  }
  .prompt {
    color: var(--text);
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
    user-select: text;
  }
  .meta {
    font-size: 12px;
  }
  button {
    font: inherit;
    color: var(--text);
    background: color-mix(in srgb, var(--surface) 80%, transparent);
    border: 1px solid var(--border);
    cursor: pointer;
  }
  button:hover {
    background: var(--surface-2);
  }
  .nav {
    position: absolute;
    top: 50%;
    transform: translateY(-50%);
    width: 44px;
    height: 64px;
    border-radius: 10px;
    font-size: 30px;
    line-height: 1;
  }
  .prev {
    left: 14px;
  }
  .next {
    right: 14px;
  }
  .x {
    position: absolute;
    top: 12px;
    right: 14px;
    width: 34px;
    height: 34px;
    border-radius: 8px;
    font-size: 15px;
  }
</style>
