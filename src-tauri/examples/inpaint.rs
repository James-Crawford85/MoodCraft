//! Try brush inpainting on a board image without the GUI:
//!   cargo run --example inpaint -- <board_dir> <source asset> <mask asset> <cfg> <prompt> [override_dir]
//! `override_dir` lets you compare a different `flux_klein/inpaint.json`.
use std::path::PathBuf;
use std::sync::Arc;

use boardcreate_lib::comfy::{self, Comfy, GenRequest, Sink};
use serde_json::Value;

#[tokio::main]
async fn main() {
    let a: Vec<String> = std::env::args().collect();
    let c = Arc::new(Comfy::new(boardcreate_lib::models::ModelConfig::default(), a.get(6).map(PathBuf::from)));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, Value)>();
    let sink = Sink(Arc::new(move |e, p| {
        let _ = tx.send((e.to_string(), p));
    }));
    tokio::spawn(comfy::ws_loop(sink.clone(), c.clone()));
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    let req = GenRequest {
        card_id: "inpaint".into(), kind: "inpaint".into(), prompt: a[5].clone(), negative: String::new(), seed: 777,
        width: 1024, height: 576, steps: 20, cfg: a[4].parse().unwrap(), source: Some(a[2].clone()), board_dir: a[1].clone(),
        resume_prompt_id: None, denoise: None, extra_sources: vec![], mask: Some(a[3].clone()),
    };
    let t = std::time::Instant::now();
    tokio::spawn(comfy::run_job(sink.clone(), c.clone(), req));
    loop {
        let (e, p) = rx.recv().await.unwrap();
        if e == "gen://done" { println!("{:.1}s -> {}", t.elapsed().as_secs_f32(), p["asset"]); break; }
        if e == "gen://error" { println!("ERROR {}", p["message"]); break; }
    }
}
