//! End-to-end check against a live ComfyUI without the GUI:
//!   cargo run --example smoke -- [board_dir]
//! Runs a text-to-image job, then a Qwen edit (variation) of its result.
use std::sync::Arc;
use std::time::Instant;

use boardcreate_lib::comfy::{self, Comfy, GenRequest, Sink};
use serde_json::Value;

#[tokio::main]
async fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| std::env::temp_dir().join("boardcreate-smoke").to_string_lossy().into_owned());
    std::fs::create_dir_all(&dir).unwrap();
    let c = Arc::new(Comfy::new());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, Value)>();
    let sink = Sink(Arc::new(move |e, p| { let _ = tx.send((e.to_string(), p)); }));
    tokio::spawn(comfy::ws_loop(sink.clone(), c.clone()));
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;

    let report = comfy::check(&c, None).await;
    println!("check: connected={} device={:?} problems={:#?}", report.connected, report.device, report.problems);
    if !report.connected { std::process::exit(1); }

    let mut source: Option<String> = None;
    for kind in ["t2i", "edit"] {
        let req = GenRequest {
            card_id: format!("smoke-{kind}"),
            kind: kind.into(),
            prompt: if kind == "t2i" { "concept art of a small mossy stone shrine in a misty forest, soft light".into() } else { String::new() },
            negative: String::new(),
            seed: 1234,
            width: 512,
            height: 512,
            steps: if kind == "t2i" { 20 } else { 20 },
            cfg: if kind == "t2i" { 3.5 } else { 4.0 },
            source: source.clone(),
            board_dir: dir.clone(),
            resume_prompt_id: None,
        };
        let mut req = req;
        if kind == "edit" { req.prompt = "Create a variation of this image at night with glowing lanterns".into(); }
        let t = Instant::now();
        tokio::spawn(comfy::run_job(sink.clone(), c.clone(), req, None));
        loop {
            let (e, p) = rx.recv().await.unwrap();
            if p["cardId"] != format!("smoke-{kind}") { continue; }
            match e.as_str() {
                "gen://progress" => println!("  [{kind}] {} {}/{}", p["stage"], p["value"], p["max"]),
                "gen://preview" => {}
                "gen://done" => { println!("[{kind}] DONE in {:.1}s: {p}", t.elapsed().as_secs_f32()); source = p["asset"].as_str().map(str::to_string); break; }
                "gen://error" => { println!("[{kind}] ERROR: {p}"); std::process::exit(2); }
                _ => {}
            }
        }
    }
    println!("assets in {dir}");
}
