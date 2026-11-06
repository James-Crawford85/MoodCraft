<script lang="ts">
  // Face of a subboard card: a mosaic of the child board's first images,
  // read from its board.json when the card appears.
  import { api, assetUrl } from "./api";
  import { board } from "./board.svelte";
  import type { BoardFile, BoardLinkCard } from "./types";

  let { card }: { card: BoardLinkCard } = $props();

  let thumbs = $state<string[]>([]);
  let count = $state<number | null>(null);
  let missing = $state(false);

  const childDir = $derived(joinPath(board.dir, card.path));

  function joinPath(dir: string, rel: string) {
    const sep = dir.includes("\\") ? "\\" : "/";
    return dir + sep + rel.replaceAll("/", sep);
  }

  $effect(() => {
    const dir = childDir;
    api.boardOpen(dir).then(
      (json) => {
        const f = JSON.parse(json) as BoardFile;
        const imgs = (f.cards ?? []).filter((c) => c.type === "image" && c.status === "ready" && c.asset);
        thumbs = imgs.slice(0, 4).map((c) => assetUrl(dir, (c as { asset: string }).asset));
        count = (f.cards ?? []).length;
        missing = false;
      },
      () => { missing = true; thumbs = []; count = null; },
    );
  });
</script>

<div class="tile">
  <div class="mosaic" class:one={thumbs.length === 1} class:two={thumbs.length === 2}>
    {#each thumbs as src}<img {src} alt="" draggable="false" />{/each}
    {#if !thumbs.length}
      <div class="empty">{missing ? "Board folder missing" : "Empty board"}</div>
    {/if}
  </div>
  <div class="label">
    <span class="icon">▦</span>
    <span class="name">{card.label || "Board"}</span>
    {#if count !== null}<span class="count">{count} item{count === 1 ? "" : "s"}</span>{/if}
  </div>
</div>

<style>
  .tile {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    background: var(--surface);
    border: 1.5px solid var(--border);
    border-radius: 12px;
    overflow: hidden;
  }
  .mosaic {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 1fr 1fr;
    grid-template-rows: 1fr 1fr;
    gap: 2px;
    background: var(--bg);
  }
  .mosaic.one {
    grid-template-columns: 1fr;
    grid-template-rows: 1fr;
  }
  .mosaic.two {
    grid-template-rows: 1fr;
  }
  .mosaic img {
    width: 100%;
    height: 100%;
    min-height: 0;
    object-fit: cover;
    user-select: none;
  }
  .empty {
    grid-column: 1 / -1;
    grid-row: 1 / -1;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--muted);
    font-size: 13px;
  }
  .label {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    font-size: 14px;
    font-weight: 600;
    color: var(--text);
  }
  .icon {
    color: var(--accent);
  }
  .name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .count {
    font-size: 12px;
    font-weight: 400;
    color: var(--muted);
  }
</style>
