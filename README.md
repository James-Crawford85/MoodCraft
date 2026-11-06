# BoardCreate

A local infinite inspiration board (in the spirit of Google Mixboard) for
concept layouts and project planning. Images are generated on the fly through
your local **ComfyUI**. Built with **Tauri v2** (Rust backend) and **SvelteKit + Svelte 5** (frontend).

## Run

1. Start ComfyUI from the **Comfy Desktop** app (it should listen on `http://127.0.0.1:8188`).
2. In this folder:

```bash
npm install
npm run tauri dev
```

The status pill in the top bar turns green when ComfyUI is reachable. It turns
amber if a required node or model is missing; hover it or open ⚙ to see which.

## Using it

| Action | How |
| --- | --- |
| Generate | Type in the bottom prompt bar, press **Enter**. Pick the aspect ratio and ×1–×4. |
| Edit an image | Select an image card, then type the change in the prompt bar, or use **Edit** in the floating toolbar |
| Variations | Select an image card → **✦ Variations** |
| Re-roll | Select a generated image → **↻** (same prompt, new seed) |
| Add images | Drag files from Explorer onto the canvas, or paste with **Ctrl+V** |
| Note | **N**, or double-click empty canvas. Double-click a note to edit it. |
| Frame | **F** (wraps the selection). Moving a frame moves the cards inside it. |
| Pan / zoom | Space-drag, middle-drag or right-drag to pan · mouse wheel zooms · trackpad scroll pans, pinch zooms |
| Select | Click, Shift/Ctrl-click, drag a marquee on empty canvas, Ctrl+A |
| Other | Del delete · Ctrl+D duplicate · Ctrl+Z / Ctrl+Y undo/redo · Ctrl+0 fit · 1 = 100% · G snap to grid |

**Free GPU** (top bar) asks ComfyUI to unload its models so Blender or other
tools get the VRAM back. The app does the same automatically when it closes;
between generations, models stay loaded so generation stays fast. Generated
files land in ComfyUI's output under `board/<board folder name>/`, and sources
for edits are uploaded to `input/board/`, kept apart from your other ComfyUI work.

Cards keep generating in the background. A pending card shows the queue and
sampling progress and has a **Cancel** button. ComfyUI errors (including GPU
out-of-memory) show on the card with a **Retry** button.

## Boards on disk

A board is a folder:

```
MyBoard/
  board.json      # cards, positions, prompts, seeds, camera
  assets/*.png    # generated, dropped and pasted images
```

Every change autosaves. A new board starts as a scratch board under
`%APPDATA%\com.sunsh.boardcreate\boards\`. **Save to folder…** copies it
somewhere permanent, and **Open…** opens any board folder. The last board
reopens on launch. Cards that were still generating re-attach to their ComfyUI
job after a restart or reload.

## ComfyUI workflows

The Rust side does all ComfyUI traffic: `POST /prompt` and `/upload/image`,
progress over `/ws` with a `/history` polling fallback, and results through
`/view`. It sends API-format templates with `{{placeholder}}` values:

- `src-tauri/workflows/t2i_flux3_klein.json`: Flux3 Image Klein base (int8) +
  Qwen3-VL 4B text encoder (`flux3`) + `flux2-vae`, `EmptyFlux2LatentImage`,
  euler/simple. Defaults: 768², 20 steps, CFG 3.5.
- `src-tauri/workflows/edit_qwen_2509.json`: Qwen-Image-Edit 2509 (int8) +
  `qwen_2.5_vl_7b_fp8_scaled` (`qwen_image`) + `qwen_image_vae`,
  `TextEncodeQwenImageEditPlus`, `FluxKontextImageScale`, AuraFlow shift 3,
  CFGNorm. 20 steps, CFG 4. This mirrors the official 2509 template without the Lightning LoRA.

To change a graph without rebuilding, put `t2i.json` / `edit.json` (same
placeholders) in `%APPDATA%\com.sunsh.boardcreate\workflows\`. The ⚙ panel's
**Re-check** compares the templates' nodes and model files with ComfyUI's
`/object_info`.

Headless end-to-end check against a live ComfyUI:

```bash
cd src-tauri && cargo run --example smoke
```

## Layout

```
src-tauri/src/comfy.rs   ComfyUI client: WS listener, job runner, cancel, health check
src-tauri/src/board.rs   board folder load/save, asset import
src-tauri/src/lib.rs     Tauri commands
src/lib/board.svelte.ts  board state, camera, undo/redo, placement
src/lib/gen.svelte.ts    generation intents → cards/jobs, backend events
src/lib/Canvas.svelte    pan/zoom/marquee/drag/resize
src/lib/CardView.svelte  image/note/frame card rendering
src/routes/+page.svelte  app shell, toolbars, settings, shortcuts, drop/paste
```
