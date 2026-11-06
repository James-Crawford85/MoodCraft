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
| Edit an image | Select an image card, then type the change in the prompt bar, or use **Edit** in the floating toolbar (Flux Klein reference edit) |
| Remix several images | Shift/Ctrl-click 2+ images (the click order numbers them; badges show **1**, **2**, …), then type in the prompt bar, e.g. "put the patch from image 2 on the jacket in image 1". Flux Klein feeds every image to `ReferenceMultiLatent`; image 1 sets the output shape. Results land right of the selection. (Not available with the Checkpoint family.) Right after a generation the new cards are selected but don't trigger a remix; select images yourself to remix them. |
| Variations | Select an image card → **✦ Variations** (Flux img2img with the card's prompt) |
| Re-roll | Select a generated image → **↻** (same prompt, new seed) |
| Add images | Drag files from Explorer onto the canvas, or paste with **Ctrl+V** |
| Note | **N**, or right-click empty canvas → **Add note** (placed where you clicked). Double-click a note to edit it. |
| Frame | **F** (wraps the selection), or right-click empty canvas → **Add frame**. Moving a frame moves the cards inside it. |
| View large | Double-click an image: full-window viewer. **←/→** (or the arrows, or the wheel) step through the board's images in reading order (top to bottom, left to right); **Esc** or a click on the backdrop closes it and selects the last image shown. |
| Pan / zoom | Space-drag, middle-drag or right-drag to pan (a right-click without dragging opens the menu) · mouse wheel zooms · trackpad scroll pans, pinch zooms |
| Select | Click, Shift/Ctrl-click, drag a marquee on empty canvas, Ctrl+A |
| Resize | Drag the corner handle of a selected card (images keep their aspect ratio). With several items selected, dragging any of their handles, or the handle of the dashed selection box, scales the whole group together about its top-left corner, so the arrangement keeps its shape. One Ctrl+Z undoes it. |
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

### Choosing models (⚙ → Models)

Pick a **model family**, then the files it needs. The dropdowns are filled
live from ComfyUI (`/models/diffusion_models`, `/models/text_encoders`,
`/models/vae`, `/models/checkpoints`, and CLIPLoader's `type` options from
`/object_info`). Changing the family preselects installed files that match it
by name; you can override any of them. **Test** renders a 4-step 256² image with
the selection before you save, so a mismatched encoder shows up in settings
rather than on a card. **Save** writes `%APPDATA%\com.sunsh.boardcreate\models.json`
(not `board.json`). It frees and loads nothing; the next image loads the new
models. Switching family also resets size, steps and CFG to that family's defaults.

| Family | Loaders | Generate | Edit | Variations | Defaults |
| --- | --- | --- | --- | --- | --- |
| Flux Klein | diffusion model + text encoder (`flux3`) + VAE | ✓ | ✓ reference edit | ✓ img2img | 768², 20 steps, CFG 3.5, euler/simple; edit CFG 1; strength 0.7 |
| Checkpoint (SD / SDXL) | one checkpoint file | ✓ | – (buttons disabled) | ✓ img2img | 1024², 25 steps, CFG 6.5, dpmpp_2m/karras; strength 0.6 |

Templates live in `src-tauri/workflows/<family>/{t2i,edit,variation}.json`.
Loader inputs are placeholders (`{{unet}}`, `{{clip}}`, `{{clip_type}}`,
`{{vae}}`, `{{ckpt}}`, plus `{{sampler}}`/`{{scheduler}}` from the family
preset). Within a family, every template uses the same loader node ids and
inputs, enforced by a unit test, so ComfyUI's cache keeps the models loaded
when you switch between generating, editing and variations. Measured warm with
Flux Klein: about 6 s each at 768².

- **Flux Klein**: `t2i` uses `EmptyFlux2LatentImage`. `edit` is the core path of
  the official `image_flux3_klein_image_edit` template (`ReferenceMultiLatent`,
  without its prompt upsampler). `variation` is img2img: the source is scaled,
  VAE-encoded and resampled with the card's original prompt. A reference edit
  asked for "a variation" barely changed the image.
- **Checkpoint**: `CheckpointLoaderSimple` → `EmptyLatentImage` / img2img.

**Adding a family:** add a folder of templates and one entry in `FAMILIES` in
`src-tauri/src/models.rs`. The entry holds the loader type, default encoder
type, file-name hints, default steps/CFG/sampler/scheduler, and whether it
supports edit and variations.

To change a graph without rebuilding, put `<kind>.json` (same placeholders) in
`%APPDATA%\com.sunsh.boardcreate\workflows\<family>\`. The ⚙ panel's
**Re-check** compares the active templates' nodes and model files with
ComfyUI's `/object_info`.

On launch, when ComfyUI is idle, the app queues a 1-step 256² image with the
selected models (saved as a temp preview only), so they are loaded before your
first real image.

Headless end-to-end check against a live ComfyUI:

```bash
cd src-tauri && cargo run --example smoke
```

```bash
cd src-tauri && cargo run --example models
```

`smoke` runs t2i → edit → variation → t2i with timings. `models` lists the installed
models, runs Test for Flux, SDXL and a deliberately mismatched combination, then
an SDXL generate and variation. The SDXL steps unload Flux.

## Layout

```
src-tauri/src/comfy.rs   ComfyUI client: WS listener, job runner, cancel, health check
src-tauri/src/board.rs   board folder load/save, asset import
src-tauri/src/models.rs  model families, model selection (models.json)
src-tauri/src/lib.rs     Tauri commands
src/lib/board.svelte.ts  board state, camera, undo/redo, placement
src/lib/gen.svelte.ts    generation intents → cards/jobs, backend events
src/lib/ModelSetup.svelte, models.svelte.ts  ⚙ → Models panel and state
src/lib/Canvas.svelte    pan/zoom/marquee/drag/resize
src/lib/CardView.svelte  image/note/frame card rendering
src/routes/+page.svelte  app shell, toolbars, settings, shortcuts, drop/paste
```
