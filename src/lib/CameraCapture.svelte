<script lang="ts">
  // Webcam snapshot → image card. Uses the WebView's getUserMedia; if the
  // camera is missing or blocked, says so instead of failing silently.
  import { onMount } from "svelte";

  let { onCapture, onClose }: { onCapture: (png: Uint8Array) => Promise<void>; onClose: () => void } = $props();

  let video: HTMLVideoElement;
  let stream: MediaStream | null = null;
  let status = $state<"starting" | "live" | "error">("starting");
  let error = $state("");
  let busy = $state(false);
  let mirrored = $state(true);

  function explain(e: unknown): string {
    const name = (e as { name?: string })?.name ?? "";
    if (name === "NotAllowedError" || name === "SecurityError") return "Camera access was blocked (by Windows privacy settings or the app's WebView). Allow camera access for desktop apps in Windows Settings > Privacy & security > Camera, then try again.";
    if (name === "NotFoundError" || name === "OverconstrainedError") return "No camera was found.";
    if (name === "NotReadableError") return "The camera is in use by another app.";
    return `The camera couldn't start: ${String(e)}`;
  }

  async function capture() {
    if (!stream || busy) return;
    busy = true;
    const c = document.createElement("canvas");
    c.width = video.videoWidth;
    c.height = video.videoHeight;
    const g = c.getContext("2d")!;
    if (mirrored) { g.translate(c.width, 0); g.scale(-1, 1); }
    g.drawImage(video, 0, 0);
    try {
      const blob: Blob = await new Promise((res, rej) => c.toBlob((b) => (b ? res(b) : rej(new Error("capture failed"))), "image/png"));
      await onCapture(new Uint8Array(await blob.arrayBuffer()));
      onClose();
    } catch (e) {
      error = String(e);
      status = "error";
      busy = false;
    }
  }

  function onKey(e: KeyboardEvent) {
    e.stopImmediatePropagation();
    if (e.key === "Escape") { e.preventDefault(); onClose(); }
    else if (e.key === " " || e.key === "Enter") { e.preventDefault(); capture(); }
  }

  onMount(() => {
    window.addEventListener("keydown", onKey, true);
    (async () => {
      try {
        if (!navigator.mediaDevices?.getUserMedia) throw Object.assign(new Error("getUserMedia is not available"), { name: "NotFoundError" });
        stream = await navigator.mediaDevices.getUserMedia({ video: { width: { ideal: 1920 }, height: { ideal: 1080 } }, audio: false });
        video.srcObject = stream;
        await video.play();
        status = "live";
      } catch (e) {
        error = explain(e);
        status = "error";
      }
    })();
    return () => {
      window.removeEventListener("keydown", onKey, true);
      stream?.getTracks().forEach((t) => t.stop());
    };
  });
</script>

<div class="cam" role="dialog" aria-modal="true" aria-label="Camera">
  <div class="panel">
    <div class="view">
      <!-- svelte-ignore a11y_media_has_caption -->
      <video bind:this={video} playsinline muted class:mirrored class:hidden={status !== "live"}></video>
      {#if status === "starting"}<p class="msg">Starting the camera…</p>{/if}
      {#if status === "error"}<p class="msg err">{error}</p>{/if}
    </div>
    <div class="row">
      <label class="mirror"><input type="checkbox" bind:checked={mirrored} /> Mirror</label>
      <span class="spacer"></span>
      <button class="btn ghost" onclick={onClose}>Cancel</button>
      <button class="btn primary" onclick={capture} disabled={status !== "live" || busy}>{busy ? "Adding…" : "Capture"}</button>
    </div>
  </div>
</div>

<style>
  .cam {
    position: fixed;
    inset: 0;
    z-index: 1000;
    background: rgba(8, 9, 11, 0.85);
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .panel {
    width: min(860px, 92vw);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 14px;
    padding: 12px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.5);
  }
  .view {
    position: relative;
    aspect-ratio: 16 / 9;
    background: #000;
    border-radius: 10px;
    overflow: hidden;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  video {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }
  video.mirrored {
    transform: scaleX(-1);
  }
  video.hidden {
    display: none;
  }
  .msg {
    color: var(--muted);
    font-size: 14px;
    padding: 0 24px;
    text-align: center;
  }
  .err {
    color: #f0b440;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    color: var(--muted);
  }
  .spacer {
    flex: 1;
  }
  .mirror {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .btn {
    font: inherit;
    color: var(--text);
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 6px 14px;
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
</style>
