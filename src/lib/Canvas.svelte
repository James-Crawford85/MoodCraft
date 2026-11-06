<script lang="ts">
  import { board, bounds, contains, intersects, type Rect } from "./board.svelte";
  import CardView from "./CardView.svelte";
  import Lightbox from "./Lightbox.svelte";
  import { remixRefs } from "./gen.svelte";
  import type { Card } from "./types";

  let el: HTMLDivElement;
  let editingId = $state<string | null>(null);
  let marquee = $state<Rect | null>(null);
  let spaceDown = $state(false);
  let panning = $state(false);
  /** Image card the viewer opened on (null = closed). */
  let viewing = $state<string | null>(null);
  /** Right-click menu: screen position (canvas-local) and the board point it was opened at. */
  let menu = $state<{ sx: number; sy: number; wx: number; wy: number } | null>(null);
  /** Set when a right-button drag actually panned, so its contextmenu event doesn't open the menu. */
  let rightPanned = false;
  /** Element the last press landed on. The canvas captures the pointer on press, so the browser
   *  fires dblclick/contextmenu at the canvas itself; this says what was really clicked. */
  let pressTarget: HTMLElement | null = null;

  /** Cards a group operation acts on: the selection plus anything inside a selected frame. */
  function groupItems(): Card[] {
    const out = new Map<string, Card>();
    for (const c of board.selection) {
      out.set(c.id, c);
      if (c.type === "frame") {
        for (const o of board.cards) if (o.id !== c.id && o.type !== "frame" && contains(c, o)) out.set(o.id, o);
      }
    }
    return [...out.values()];
  }

  /** Bounding box shown (with its own handle) when more than one item is selected. */
  const groupBox = $derived(board.selected.size > 1 ? bounds(board.selection) : null);

  /** Start scaling the whole selection about its top-left; (cornerX, cornerY) is the dragged corner. */
  function startGroupResize(cornerX: number, cornerY: number) {
    const items = groupItems();
    const b = bounds(items);
    const vx = cornerX - b.x, vy = cornerY - b.y;
    if (vx * vx + vy * vy < 1) return false;
    // Don't let any item shrink below 40 units on its short side (or below its current size if it's already smaller).
    const minScale = Math.max(...items.map((c) => Math.min(1, 40 / Math.max(1, Math.min(c.w, c.h)))));
    drag = {
      kind: "group-resize", moved: false, ax: b.x, ay: b.y, vx, vy, minScale,
      items: items.map((c) => ({ c, x: c.x, y: c.y, w: c.w, h: c.h })),
    };
    return true;
  }

  /** "image N" numbers shown on cards while a multi-image remix is set up. */
  const refNums = $derived(new Map(remixRefs().map((c, i) => [c.id, i + 1])));

  // Frames render first (lowest z), then everything else by z.
  const sorted = $derived([...board.cards].sort((a, b) => (a.type === "frame" ? 0 : 1) - (b.type === "frame" ? 0 : 1) || a.z - b.z));

  type Drag =
    | { kind: "pan"; sx: number; sy: number; cx: number; cy: number }
    | { kind: "marquee"; wx: number; wy: number; additive: boolean; base: Set<string> }
    | { kind: "move"; wx: number; wy: number; items: { c: Card; x: number; y: number }[]; anchor: Card; moved: boolean }
    | { kind: "resize"; c: Card; wx: number; wy: number; w: number; h: number; moved: boolean }
    | {
        kind: "group-resize"; moved: boolean;
        /** Fixed point: the top-left of the selection's bounding box (opposite the handles). */
        ax: number; ay: number;
        /** Dragged corner relative to the anchor at the start; the scale follows the pointer's projection onto it. */
        vx: number; vy: number;
        minScale: number;
        items: { c: Card; x: number; y: number; w: number; h: number }[];
      };
  let drag: Drag | null = null;

  const GRID = 16;
  const snap = (v: number) => (board.settings.snap ? Math.round(v / GRID) * GRID : v);

  function local(e: PointerEvent | WheelEvent | MouseEvent) {
    const r = el.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  }

  function onPointerDown(e: PointerEvent) {
    if (menu && !(e.target as HTMLElement).closest(".ctxmenu")) menu = null;
    if ((e.target as HTMLElement).closest("[data-action]")) return;
    pressTarget = e.target as HTMLElement;
    const p = local(e);
    const w = board.screenToWorld(p.x, p.y);
    if (e.button === 1 || e.button === 2 || (e.button === 0 && spaceDown)) {
      drag = { kind: "pan", sx: p.x, sy: p.y, cx: board.camera.x, cy: board.camera.y };
      if (e.button === 2) rightPanned = false;
      panning = true;
      el.setPointerCapture(e.pointerId);
      e.preventDefault();
      return;
    }
    if (e.button !== 0) return;
    if (editingId) editingId = null;

    const cardEl = (e.target as HTMLElement).closest("[data-card]") as HTMLElement | null;
    const card = cardEl ? board.byId(cardEl.dataset.card!) : undefined;

    if (!card && groupBox && (e.target as HTMLElement).closest("[data-group-resize]")) {
      if (startGroupResize(groupBox.x + groupBox.w, groupBox.y + groupBox.h)) el.setPointerCapture(e.pointerId);
      return;
    }

    if (!card) {
      const base = e.shiftKey || e.ctrlKey ? new Set(board.selected) : new Set<string>();
      board.selected = base;
      drag = { kind: "marquee", wx: w.x, wy: w.y, additive: e.shiftKey || e.ctrlKey, base };
      marquee = { x: w.x, y: w.y, w: 0, h: 0 };
      el.setPointerCapture(e.pointerId);
      return;
    }

    if ((e.target as HTMLElement).closest("[data-resize]")) {
      // Resizing one item of a multi-selection scales the whole selection with it.
      if (board.selected.size > 1 && board.selected.has(card.id)) {
        if (startGroupResize(card.x + card.w, card.y + card.h)) el.setPointerCapture(e.pointerId);
        return;
      }
      drag = { kind: "resize", c: card, wx: w.x, wy: w.y, w: card.w, h: card.h, moved: false };
      el.setPointerCapture(e.pointerId);
      return;
    }

    // Selection
    if (e.shiftKey || e.ctrlKey) {
      const s = new Set(board.selected);
      s.has(card.id) ? s.delete(card.id) : s.add(card.id);
      board.selected = s;
    } else if (!board.selected.has(card.id)) {
      board.selected = new Set([card.id]);
    }
    if (!board.selected.has(card.id)) return;

    // Moving: selected cards + anything sitting inside a selected frame.
    const moving = new Map<string, Card>();
    for (const c of board.selection) {
      moving.set(c.id, c);
      if (c.type === "frame") {
        for (const o of board.cards) if (o.id !== c.id && o.type !== "frame" && contains(c, o)) moving.set(o.id, o);
      }
    }
    drag = {
      kind: "move", wx: w.x, wy: w.y, anchor: card, moved: false,
      items: [...moving.values()].map((c) => ({ c, x: c.x, y: c.y })),
    };
    el.setPointerCapture(e.pointerId);
  }

  function onPointerMove(e: PointerEvent) {
    if (!drag) return;
    // No button held: the release never arrived (WebView2 drops pointerup after a right-press
    // that shows a context menu), so end the drag instead of letting it follow the mouse.
    if (e.buttons === 0 && e.pointerType === "mouse") { endDrag(e); return; }
    const p = local(e);
    if (drag.kind === "pan") {
      if (Math.hypot(p.x - drag.sx, p.y - drag.sy) > 4) rightPanned = true;
      board.camera = { ...board.camera, x: drag.cx + p.x - drag.sx, y: drag.cy + p.y - drag.sy };
      return;
    }
    const w = board.screenToWorld(p.x, p.y);
    if (drag.kind === "marquee") {
      const r = { x: Math.min(drag.wx, w.x), y: Math.min(drag.wy, w.y), w: Math.abs(w.x - drag.wx), h: Math.abs(w.y - drag.wy) };
      marquee = r;
      const s = new Set(drag.base);
      for (const c of board.cards) {
        // Frames only get selected when fully enclosed, so a marquee inside a frame picks its contents.
        if (c.type === "frame" ? contains(r, c) : intersects(r, c)) s.add(c.id);
      }
      board.selected = s;
    } else if (drag.kind === "move") {
      let dx = w.x - drag.wx, dy = w.y - drag.wy;
      if (!drag.moved) {
        if (Math.hypot(dx, dy) * board.camera.zoom < 3) return;
        board.checkpoint();
        drag.moved = true;
      }
      const anchorId = drag.anchor.id;
      const a = drag.items.find((i) => i.c.id === anchorId)!;
      dx = snap(a.x + dx) - a.x;
      dy = snap(a.y + dy) - a.y;
      for (const i of drag.items) {
        i.c.x = i.x + dx;
        i.c.y = i.y + dy;
      }
    } else if (drag.kind === "resize") {
      if (!drag.moved) { board.checkpoint(); drag.moved = true; }
      const c = drag.c;
      let nw = Math.max(60, snap(drag.w + (w.x - drag.wx)));
      if (c.type === "image") {
        const ratio = (c.naturalH ?? drag.h) / (c.naturalW ?? drag.w);
        c.w = nw;
        c.h = Math.round(nw * ratio);
      } else {
        c.w = nw;
        c.h = Math.max(60, snap(drag.h + (w.y - drag.wy)));
      }
    } else if (drag.kind === "group-resize") {
      const qx = w.x - drag.ax, qy = w.y - drag.ay;
      let k = (qx * drag.vx + qy * drag.vy) / (drag.vx * drag.vx + drag.vy * drag.vy);
      if (!drag.moved) {
        if (Math.abs(k - 1) * Math.hypot(drag.vx, drag.vy) * board.camera.zoom < 3) return;
        board.checkpoint(); // one undo step for the whole group resize
        drag.moved = true;
      }
      k = Math.max(drag.minScale, k);
      for (const i of drag.items) {
        i.c.x = drag.ax + (i.x - drag.ax) * k;
        i.c.y = drag.ay + (i.y - drag.ay) * k;
        i.c.w = i.w * k;
        i.c.h = i.h * k;
      }
    }
  }

  function endDrag(e?: PointerEvent) {
    if ((drag?.kind === "move" || drag?.kind === "group-resize") && drag.moved) {
      if (drag.kind === "group-resize") for (const i of drag.items) {
        const c = i.c;
        c.x = Math.round(c.x); c.y = Math.round(c.y); c.w = Math.round(c.w); c.h = Math.round(c.h);
      }
      board.touch();
    }
    drag = null;
    marquee = null;
    panning = false;
    if (e && el.hasPointerCapture(e.pointerId)) el.releasePointerCapture(e.pointerId);
  }
  const onPointerUp = (e: PointerEvent) => endDrag(e);

  function onWheel(e: WheelEvent) {
    e.preventDefault();
    menu = null;
    const p = local(e);
    // Mouse wheels send whole notches with no horizontal component; trackpads
    // send small/fractional deltas (pan) or ctrl+wheel for pinch (zoom).
    const looksLikeMouse = e.deltaMode !== 0 || (e.deltaX === 0 && Math.abs(e.deltaY) >= 50 && Number.isInteger(e.deltaY));
    if (e.ctrlKey || looksLikeMouse) {
      const d = e.deltaMode === 1 ? e.deltaY * 33 : e.deltaY;
      board.zoomAt(p.x, p.y, Math.exp(-d * (e.ctrlKey && !looksLikeMouse ? 0.01 : 0.0015)));
    } else {
      board.camera = { ...board.camera, x: board.camera.x - e.deltaX, y: board.camera.y - e.deltaY };
    }
  }

  function onDblClick(e: MouseEvent) {
    const t = (e.target === el && pressTarget ? pressTarget : e.target) as HTMLElement;
    const cardEl = t.closest("[data-card]") as HTMLElement | null;
    if (!cardEl) return;
    const card = board.byId(cardEl.dataset.card!);
    if (!card) return;
    if (card.type === "image" && card.status === "ready" && card.asset) { viewing = card.id; return; }
    if (card.type === "note") { board.checkpoint(); editingId = card.id; }
    if (card.type === "frame" && t.closest("[data-frame-label]")) { board.checkpoint(); editingId = card.id; }
  }

  function onContextMenu(e: MouseEvent) {
    e.preventDefault();
    const t = (e.target === el && pressTarget ? pressTarget : e.target) as HTMLElement;
    if (drag?.kind === "pan") endDrag();
    // A right-drag pans; only a plain right-click on empty canvas opens the menu.
    if (rightPanned || t.closest("[data-card]") || t.closest(".ctxmenu")) { rightPanned = false; return; }
    const p = local(e);
    const w = board.screenToWorld(p.x, p.y);
    const r = el.getBoundingClientRect();
    // Keep the menu on screen.
    menu = { sx: Math.min(p.x, r.width - 170), sy: Math.min(p.y, r.height - 90), wx: w.x, wy: w.y };
  }

  function menuAdd(kind: "note" | "frame") {
    if (!menu) return;
    const { wx, wy } = menu;
    menu = null;
    if (kind === "note") editingId = board.addNote(wx, wy).id;
    else board.addFrame({ x: wx, y: wy });
  }

  function closeViewer(lastId: string) {
    viewing = null;
    if (board.byId(lastId)) board.selected = new Set([lastId]);
  }

  export function editCard(id: string) {
    editingId = id;
  }

  function onKey(e: KeyboardEvent, down: boolean) {
    if (down && menu && e.key === "Escape") { menu = null; return; }
    if (e.code === "Space" && !isTyping(e)) {
      spaceDown = down;
      if (down) e.preventDefault();
    }
  }
  function isTyping(e: Event) {
    const t = e.target as HTMLElement;
    return t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable;
  }

  // Background grid follows the camera.
  const gridSize = $derived(24 * board.camera.zoom);
</script>

<svelte:window onkeydown={(e) => onKey(e, true)} onkeyup={(e) => onKey(e, false)} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  bind:this={el}
  class="canvas"
  class:pan-ready={spaceDown}
  class:panning
  style="background-size: {gridSize}px {gridSize}px; background-position: {board.camera.x}px {board.camera.y}px;"
  onpointerdown={onPointerDown}
  onpointermove={onPointerMove}
  onpointerup={onPointerUp}
  onpointercancel={onPointerUp}
  onwheel={onWheel}
  ondblclick={onDblClick}
  oncontextmenu={onContextMenu}
>
  <div class="world" style="transform: translate({board.camera.x}px, {board.camera.y}px) scale({board.camera.zoom});">
    {#each sorted as card (card.id)}
      <CardView {card} selected={board.selected.has(card.id)} editing={editingId === card.id} refIndex={refNums.get(card.id) ?? 0} onStopEdit={() => { editingId = null; board.touch(); }} />
    {/each}
    {#if groupBox && !marquee}
      <div class="groupbox" style="transform: translate({groupBox.x}px, {groupBox.y}px); width: {groupBox.w}px; height: {groupBox.h}px; border-width: {1 / board.camera.zoom}px">
        <div class="ghandle" data-group-resize title="Drag to resize the selection together" style="width: {16 / board.camera.zoom}px; height: {16 / board.camera.zoom}px; right: {-9 / board.camera.zoom}px; bottom: {-9 / board.camera.zoom}px; border-width: {2 / board.camera.zoom}px; border-radius: {4 / board.camera.zoom}px"></div>
      </div>
    {/if}
    {#if marquee}
      <div class="marquee" style="transform: translate({marquee.x}px, {marquee.y}px); width: {marquee.w}px; height: {marquee.h}px; border-width: {1 / board.camera.zoom}px"></div>
    {/if}
  </div>
  {#if menu}
    <div class="ctxmenu" data-action role="menu" style="left: {menu.sx}px; top: {menu.sy}px;">
      <button role="menuitem" onclick={() => menuAdd("note")}>Add note <kbd>N</kbd></button>
      <button role="menuitem" onclick={() => menuAdd("frame")}>Add frame <kbd>F</kbd></button>
    </div>
  {/if}
  {#if board.cards.length === 0}
    <div class="empty-hint">
      <h2>Start your board</h2>
      <p>Type a prompt below to generate an image, drop image files here, or right-click to add a note or frame.</p>
      <p class="keys"><kbd>Space</kbd>/<kbd>Middle</kbd> drag to pan · <kbd>Wheel</kbd> zoom · <kbd>N</kbd> note · <kbd>F</kbd> frame</p>
    </div>
  {/if}
</div>

{#if viewing}
  <Lightbox startId={viewing} onClose={closeViewer} />
{/if}

<style>
  .canvas {
    position: absolute;
    inset: 0;
    overflow: hidden;
    background-color: var(--bg);
    background-image: radial-gradient(circle, var(--grid) 1px, transparent 1.2px);
    touch-action: none;
    cursor: default;
  }
  .canvas.pan-ready {
    cursor: grab;
  }
  .canvas.panning {
    cursor: grabbing;
  }
  .world {
    position: absolute;
    left: 0;
    top: 0;
    transform-origin: 0 0;
    will-change: transform;
  }
  .groupbox {
    position: absolute;
    left: 0;
    top: 0;
    border: 1px dashed color-mix(in srgb, var(--accent) 70%, transparent);
    z-index: 99999;
    pointer-events: none;
    box-sizing: border-box;
  }
  .ghandle {
    position: absolute;
    background: var(--bg);
    border-style: solid;
    border-color: var(--accent);
    box-sizing: border-box;
    cursor: nwse-resize;
    pointer-events: auto;
  }
  .marquee {
    position: absolute;
    left: 0;
    top: 0;
    border: 1px solid var(--accent);
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    z-index: 100000;
    pointer-events: none;
  }
  .ctxmenu {
    position: absolute;
    z-index: 200000;
    min-width: 160px;
    padding: 4px;
    display: flex;
    flex-direction: column;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: 0 8px 28px rgba(0, 0, 0, 0.45);
  }
  .ctxmenu button {
    font: inherit;
    font-size: 13px;
    color: var(--text);
    background: transparent;
    border: 0;
    border-radius: 6px;
    padding: 7px 10px;
    text-align: left;
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 16px;
    cursor: pointer;
  }
  .ctxmenu button:hover,
  .ctxmenu button:focus-visible {
    background: var(--surface-2);
    outline: none;
  }
  .empty-hint {
    position: absolute;
    left: 50%;
    top: 42%;
    transform: translate(-50%, -50%);
    text-align: center;
    color: var(--muted);
    pointer-events: none;
    max-width: 440px;
  }
  .empty-hint h2 {
    color: var(--text);
    font-weight: 600;
    margin: 0 0 8px;
  }
  .keys {
    font-size: 12px;
  }
  kbd {
    font-family: inherit;
    font-size: 11px;
    padding: 1px 5px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--surface);
  }
</style>
