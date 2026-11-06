// "Present": turn the board into a self-contained HTML slide deck.
// One slide per frame (reading order), a title slide and a "loose items"
// slide for everything outside frames. No AI involved. Images are referenced
// as __ASSET_n__ tokens that Rust replaces with base64 data URIs.
import { board, contains } from "./board.svelte";
import type { Card, FrameCard, ImageCard, NoteCard } from "./types";

const esc = (s: string) => s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);

/** Rows top to bottom, left to right within a row (a card starts a new row
 *  when its top is below the middle of the row's first card). */
function readingOrder<T extends Card>(cards: T[]): T[] {
  const sorted = [...cards].sort((a, b) => a.y - b.y || a.x - b.x);
  const rows: T[][] = [];
  for (const c of sorted) {
    const row = rows[rows.length - 1];
    if (row && c.y < row[0].y + row[0].h / 2) row.push(c);
    else rows.push([c]);
  }
  return rows.flatMap((r) => r.sort((a, b) => a.x - b.x));
}

interface Slide {
  title: string;
  images: ImageCard[];
  notes: NoteCard[];
  extra: string[];
}

export function buildDeck(): { html: string; assets: string[] } {
  const frames = readingOrder(board.cards.filter((c): c is FrameCard => c.type === "frame"));
  const items = board.cards.filter((c) => c.type !== "frame");
  // Each item belongs to the smallest frame that fully contains it.
  const owner = new Map<string, FrameCard | null>();
  for (const c of items) {
    const fs = frames.filter((f) => contains(f, c)).sort((a, b) => a.w * a.h - b.w * b.h);
    owner.set(c.id, fs[0] ?? null);
  }
  const slideFor = (title: string, cards: Card[]): Slide => {
    const ordered = readingOrder(cards);
    return {
      title,
      images: ordered.filter((c): c is ImageCard => c.type === "image" && c.status === "ready" && !!c.asset),
      notes: ordered.filter((c): c is NoteCard => c.type === "note" && !!c.text.trim()),
      extra: ordered.filter((c) => c.type === "board").map((c) => `Subboard: ${(c as { label: string }).label || "Board"}`),
    };
  };
  const slides: Slide[] = frames
    .map((f) => slideFor(f.label || "Frame", items.filter((c) => owner.get(c.id) === f)))
    .filter((s) => s.images.length || s.notes.length || s.extra.length);
  const loose = slideFor("Loose items", items.filter((c) => !owner.get(c.id)));
  // Split big loose sets so images stay readable.
  const PER = 12;
  for (let i = 0; i < Math.max(loose.images.length, 1); i += PER) {
    const part: Slide = { ...loose, images: loose.images.slice(i, i + PER), notes: i === 0 ? loose.notes : [], extra: i === 0 ? loose.extra : [] };
    if (!part.images.length && !part.notes.length && !part.extra.length) continue;
    if (loose.images.length > PER) part.title = `Loose items (${i / PER + 1}/${Math.ceil(loose.images.length / PER)})`;
    slides.push(part);
  }

  const assets: string[] = [];
  const token = (asset: string) => {
    let i = assets.indexOf(asset);
    if (i < 0) { i = assets.length; assets.push(asset); }
    return `__ASSET_${i}__`;
  };

  const imageCount = items.filter((c) => c.type === "image" && c.status === "ready").length;
  const date = new Date().toLocaleDateString(undefined, { year: "numeric", month: "long", day: "numeric" });
  const parts: string[] = [];
  parts.push(`<section class="slide title"><div><h1>${esc(board.name)}</h1><p>${date} · ${frames.length} frame${frames.length === 1 ? "" : "s"} · ${imageCount} image${imageCount === 1 ? "" : "s"}</p></div></section>`);
  for (const s of slides) {
    const n = s.images.length;
    const cols = n <= 1 ? 1 : n <= 4 ? 2 : n <= 9 ? 3 : 4;
    const imgs = s.images
      .map((c) => `<figure><img src="${token(c.asset!)}" alt="${esc(c.prompt ?? "")}">${c.prompt ? `<figcaption>${esc(c.prompt)}</figcaption>` : ""}</figure>`)
      .join("");
    const side = [
      ...s.notes.map((c) => `<div class="note" style="--note:${esc(c.color)}">${esc(c.text)}</div>`),
      ...s.extra.map((t) => `<div class="note sub">${esc(t)}</div>`),
    ].join("");
    parts.push(
      `<section class="slide"><h2>${esc(s.title)}</h2><div class="body${side ? " with-side" : ""}">` +
        (n ? `<div class="grid" style="--cols:${cols};--rows:${Math.ceil(n / cols)}">${imgs}</div>` : "") +
        (side ? `<aside>${side}</aside>` : "") +
        `</div></section>`,
    );
  }

  const html = `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>${esc(board.name)}</title>
<style>
:root{--bg:#121316;--surface:#1c1e23;--text:#e8e9ec;--muted:#9aa0ab;--accent:#7c9cff}
*{box-sizing:border-box}html,body{margin:0;height:100%;background:var(--bg);color:var(--text);font:15px/1.45 system-ui,-apple-system,"Segoe UI",sans-serif}
.slide{position:absolute;inset:0;display:none;flex-direction:column;padding:4vh 4vw 6vh}
.slide.on{display:flex}
.slide.title{align-items:center;justify-content:center;text-align:center}
.title h1{font-size:clamp(32px,6vw,72px);margin:0 0 12px;letter-spacing:-.01em}
.title p{color:var(--muted);margin:0}
h2{margin:0 0 2vh;font-size:clamp(20px,2.6vw,34px);font-weight:650}
.body{flex:1;min-height:0;display:flex;gap:2vw}
.grid{flex:1;min-height:0;display:grid;grid-template-columns:repeat(var(--cols),minmax(0,1fr));grid-template-rows:repeat(var(--rows),minmax(0,1fr));gap:1.4vw}
figure{margin:0;min-height:0;display:flex;flex-direction:column;gap:6px}
figure img{flex:1;min-height:0;width:100%;object-fit:contain;border-radius:6px;background:#0b0c0e}
figcaption{font-size:12px;color:var(--muted);overflow:hidden;display:-webkit-box;-webkit-line-clamp:2;-webkit-box-orient:vertical}
aside{width:min(28vw,380px);overflow:auto;display:flex;flex-direction:column;gap:12px}
.note{background:var(--note,#f7e27a);color:#1b1c1f;border-radius:10px;padding:14px 16px;white-space:pre-wrap;font-size:16px}
.note.sub{background:var(--surface);color:var(--text);border:1px solid #343842}
.body:not(.with-side) aside{display:none}
.hud{position:fixed;right:16px;bottom:12px;display:flex;gap:8px;align-items:center;color:var(--muted);font-size:13px}
.hud button{font:inherit;color:var(--text);background:var(--surface);border:1px solid #343842;border-radius:8px;padding:5px 10px;cursor:pointer}
@media print{@page{size:landscape;margin:0}html,body{height:auto}.slide{position:relative;display:flex!important;height:100vh;page-break-after:always;break-after:page}.hud{display:none}}
</style></head><body>
${parts.join("\n")}
<div class="hud"><button id="prev" aria-label="Previous slide">‹</button><span id="n"></span><button id="next" aria-label="Next slide">›</button><button id="print" title="Print or save as PDF">Print / PDF</button></div>
<script>
const s=[...document.querySelectorAll('.slide')];let i=0;
function show(k){i=Math.max(0,Math.min(s.length-1,k));s.forEach((e,j)=>e.classList.toggle('on',j===i));document.getElementById('n').textContent=(i+1)+' / '+s.length;}
document.getElementById('prev').onclick=()=>show(i-1);document.getElementById('next').onclick=()=>show(i+1);
document.getElementById('print').onclick=()=>print();
addEventListener('keydown',e=>{if(['ArrowRight','PageDown',' '].includes(e.key)){e.preventDefault();show(i+1)}else if(['ArrowLeft','PageUp','Backspace'].includes(e.key)){e.preventDefault();show(i-1)}else if(e.key==='Home')show(0);else if(e.key==='End')show(s.length-1);});
show(0);
</script></body></html>`;
  return { html, assets };
}
