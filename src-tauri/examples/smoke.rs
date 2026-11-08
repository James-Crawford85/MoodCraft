//! End-to-end check against a live ComfyUI without the GUI:
//!   cargo run --example smoke -- [board_dir] [edit_cfg] [edit_prompt]
//! Runs text-to-image → edit of that image → text-to-image, printing timings.
//! The second text-to-image should not reload models (shared loader nodes).
use std::sync::Arc;
use std::time::Instant;

use moodcraft_lib::comfy::{self, Comfy, GenRequest, Sink};
use serde_json::Value;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = args.get(1).cloned().unwrap_or_else(|| std::env::temp_dir().join("moodcraft-smoke").to_string_lossy().into_owned());
    let edit_cfg: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let edit_prompt = args.get(3).cloned().unwrap_or_else(|| "Make it night time, lit by warm glowing lanterns".into());
    std::fs::create_dir_all(&dir).unwrap();

    let c = Arc::new(Comfy::new(moodcraft_lib::models::ModelConfig::default(), None));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, Value)>();
    let sink = Sink(Arc::new(move |e, p| {
        let _ = tx.send((e.to_string(), p));
    }));
    tokio::spawn(comfy::ws_loop(sink.clone(), c.clone()));
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;

    let report = comfy::check(&c).await;
    println!("check: connected={} device={:?} problems={:?}", report.connected, report.device, report.problems);
    if !report.connected {
        std::process::exit(1);
    }

    let mut source: Option<String> = None;
    let base_prompt = "concept art of a small mossy stone shrine in a misty forest, soft light";
    for (i, kind) in ["t2i", "edit", "variation", "t2i"].into_iter().enumerate() {
        let card_id = format!("smoke-{i}-{kind}");
        let edit = kind != "t2i";
        let req = GenRequest {
            card_id: card_id.clone(),
            kind: kind.into(),
            prompt: match kind {
                "edit" => edit_prompt.clone(),
                "variation" => base_prompt.into(),
                _ => format!("{base_prompt}, variant {i}"),
            },
            negative: String::new(),
            seed: 1234 + i as u64,
            width: 768,
            height: 768,
            steps: 20,
            cfg: if kind == "edit" { edit_cfg } else { 3.5 },
            source: if edit { source.clone() } else { None },
            board_dir: dir.clone(),
            resume_prompt_id: None,
            denoise: Some(0.7),
            extra_sources: vec![], mask: None,
        };
        let t = Instant::now();
        let mut first_sample: Option<f32> = None;
        tokio::spawn(comfy::run_job(sink.clone(), c.clone(), req));
        loop {
            let (e, p) = rx.recv().await.unwrap();
            if p["cardId"] != card_id.as_str() {
                continue;
            }
            match e.as_str() {
                "gen://progress" if p["stage"] == "sampling" && first_sample.is_none() => first_sample = Some(t.elapsed().as_secs_f32()),
                "gen://done" => {
                    println!("[{i} {kind}] {:.1}s total (sampling began at {:.1}s) -> {}", t.elapsed().as_secs_f32(), first_sample.unwrap_or(-1.0), p["asset"]);
                    if !edit {
                        source = p["asset"].as_str().map(str::to_string);
                    }
                    break;
                }
                "gen://error" => {
                    println!("[{i} {kind}] ERROR: {}", p["message"]);
                    std::process::exit(2);
                }
                _ => {}
            }
        }
    }
    println!("assets in {dir}");
}
