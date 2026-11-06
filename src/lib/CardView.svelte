<script lang="ts">
  import type { Card } from "./types";
  import { board } from "./board.svelte";
  import { assetUrl } from "./api";
  import { retry, cancel, stageLabel } from "./gen.svelte";

  let { card, selected, editing, onStopEdit }: {
    card: Card; selected: boolean; editing: boolean; onStopEdit: () => void;
  } = $props();

  const job = $derived(board.jobs[card.id]);
  const pct = $derived(job?.max ? Math.round(((job.value ?? 0) / job.max) * 100) : null);

  function autofocus(el: HTMLElement) {
    el.focus();
    if (el instanceof HTMLTextAreaElement || el instanceof HTMLInputElement) el.select();
  }
  function keyStop(e: KeyboardEvent) {
    e.stopPropagation();
    if (e.key === "Escape" || (e.key === "Enter" && (card.type === "frame" || e.ctrlKey))) {
      (e.target as HTMLElement).blur();
    }
  }
</script>

<div
  class="card {card.type}"
  class:selected
  class:dark={card.type === "note" && card.color === "#2b2e35"}
  data-card={card.id}
  style="transform: translate({card.x}px, {card.y}px); width: {card.w}px; height: {card.h}px; z-index: {card.z};
         {card.type === 'note' ? `--note:${card.color}` : ''} {card.type === 'frame' ? `--frame:${card.color}` : ''}"
>
  {#if card.type === "image"}
    {#if card.status === "ready" && card.asset}
      <img src={assetUrl(board.dir, card.asset)} alt={card.prompt ?? "image"} draggable="false" />
    {:else if card.status === "pending"}
      <div class="pending">
        {#if job?.preview}<img class="preview" src={job.preview} alt="" draggable="false" />{/if}
        <div class="shimmer"></div>
        <div class="pending-info">
          <div class="stage">{stageLabel(job?.stage ?? "")}{pct !== null && job?.stage === "sampling" ? ` ${pct}%` : ""}</div>
          <div class="bar"><div class="fill" class:indeterminate={pct === null} style="width:{pct ?? 30}%"></div></div>
          {#if card.prompt}<div class="pprompt">{card.prompt}</div>{/if}
          <button class="mini" data-action onpointerdown={(e) => e.stopPropagation()} onclick={() => cancel(card)}>Cancel</button>
        </div>
      </div>
    {:else}
      <div class="errorbox">
        <div class="etitle">{card.error === "Cancelled" ? "Cancelled" : "Generation failed"}</div>
        {#if card.error !== "Cancelled"}<div class="emsg">{card.error}</div>{/if}
        {#if card.gen}
          <button class="mini" data-action onpointerdown={(e) => e.stopPropagation()} onclick={() => retry(card)}>Retry</button>
        {/if}
      </div>
    {/if}
  {:else if card.type === "note"}
    {#if editing}
      <textarea
        class="note-edit"
        bind:value={card.text}
        use:autofocus
        onblur={onStopEdit}
        onkeydown={keyStop}
        onpointerdown={(e) => e.stopPropagation()}
        oninput={() => board.touch()}
        placeholder="Write something…"
      ></textarea>
    {:else}
      <div class="note-text" class:empty={!card.text}>{card.text || "Double-click to write"}</div>
    {/if}
  {:else if card.type === "frame"}
    <div class="frame-label">
      {#if editing}
        <input
          bind:value={card.label}
          use:autofocus
          onblur={onStopEdit}
          onkeydown={keyStop}
          onpointerdown={(e) => e.stopPropagation()}
          oninput={() => board.touch()}
        />
      {:else}
        <span data-frame-label>{card.label || "Frame"}</span>
      {/if}
    </div>
  {/if}
  {#if selected}
    <div class="handle" data-resize></div>
  {/if}
</div>

<style>
  .card {
    position: absolute;
    left: 0;
    top: 0;
    border-radius: 10px;
    box-sizing: border-box;
    user-select: none;
  }
  .card.image {
    background: var(--surface-2);
    box-shadow: 0 6px 24px rgba(0, 0, 0, 0.35);
    overflow: hidden;
  }
  .card.image img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
    pointer-events: none;
  }
  .card.selected {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .card.note {
    background: var(--note);
    color: #1d1d1f;
    padding: 14px 16px;
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.3);
    font-size: 15px;
    line-height: 1.4;
  }
  .card.note.dark {
    color: var(--text);
  }
  .note-text {
    white-space: pre-wrap;
    overflow: hidden;
    height: 100%;
    word-break: break-word;
  }
  .note-text.empty {
    opacity: 0.45;
  }
  .note-edit {
    width: 100%;
    height: 100%;
    background: transparent;
    border: none;
    outline: none;
    resize: none;
    font: inherit;
    color: inherit;
    padding: 0;
  }
  .card.frame {
    background: color-mix(in srgb, var(--frame) 9%, transparent);
    border: 1.5px solid color-mix(in srgb, var(--frame) 55%, transparent);
    border-radius: 14px;
  }
  .frame-label {
    position: absolute;
    left: 14px;
    top: 10px;
    font-size: 15px;
    font-weight: 600;
    color: color-mix(in srgb, var(--frame) 60%, white);
    letter-spacing: 0.01em;
  }
  .frame-label input {
    font: inherit;
    color: inherit;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 2px 6px;
    outline: none;
  }
  .handle {
    position: absolute;
    right: -7px;
    bottom: -7px;
    width: 14px;
    height: 14px;
    background: var(--accent);
    border: 2px solid var(--bg);
    border-radius: 4px;
    cursor: nwse-resize;
  }
  .pending {
    position: relative;
    width: 100%;
    height: 100%;
    background: linear-gradient(135deg, #20232b, #1a1c22);
  }
  .preview {
    position: absolute;
    inset: 0;
    filter: blur(1px) saturate(0.9);
    opacity: 0.85;
  }
  .shimmer {
    position: absolute;
    inset: 0;
    background: linear-gradient(100deg, transparent 30%, rgba(255, 255, 255, 0.06) 50%, transparent 70%);
    background-size: 200% 100%;
    animation: shimmer 1.6s linear infinite;
  }
  @keyframes shimmer {
    from { background-position: 150% 0; }
    to { background-position: -50% 0; }
  }
  .pending-info {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    padding: 12px;
    background: linear-gradient(transparent, rgba(0, 0, 0, 0.7));
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: 12px;
  }
  .stage {
    color: var(--text);
    font-weight: 500;
  }
  .pprompt {
    color: var(--muted);
    overflow: hidden;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
  }
  .bar {
    height: 4px;
    border-radius: 2px;
    background: rgba(255, 255, 255, 0.12);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    background: var(--accent);
    transition: width 0.25s ease;
  }
  .fill.indeterminate {
    animation: indet 1.2s ease-in-out infinite;
  }
  @keyframes indet {
    0% { transform: translateX(-100%); }
    100% { transform: translateX(340%); }
  }
  .errorbox {
    width: 100%;
    height: 100%;
    padding: 14px;
    box-sizing: border-box;
    background: #2a1a1d;
    border: 1px solid #6b2a33;
    border-radius: 10px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    overflow: auto;
    font-size: 12px;
    user-select: text;
  }
  .etitle {
    color: #ff8f9c;
    font-weight: 600;
    font-size: 13px;
  }
  .emsg {
    color: #e9c2c7;
    white-space: pre-wrap;
    word-break: break-word;
    flex: 1;
    overflow: auto;
  }
  .mini {
    align-self: flex-start;
    font-size: 12px;
    padding: 4px 10px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--surface);
    color: var(--text);
    cursor: pointer;
  }
  .mini:hover {
    background: var(--surface-2);
  }
</style>
