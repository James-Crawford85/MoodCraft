//! Try "keep identity" edit instructions on an existing board image:
//!   cargo run --example identity -- <board_dir> <asset> <cfg> <prompt>...
//! Prints the result asset for each prompt (same seed for all).
use std::sync::Arc;

use boardcreate_lib::comfy::{self, Comfy, GenRequest, Sink};
use serde_json::Value;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (dir, asset, cfg) = (args[1].clone(), args[2].clone(), args[3].parse::<f64>().unwrap());
    let c = Arc::new(Comfy::new(boardcreate_lib::models::ModelConfig::default(), None));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, Value)>();
    let sink = Sink(Arc::new(move |e, p| {
        let _ = tx.send((e.to_string(), p));
    }));
    tokio::spawn(comfy::ws_loop(sink.clone(), c.clone()));
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    for (i, prompt) in args[4..].iter().enumerate() {
        let card_id = format!("identity-{i}");
        let req = GenRequest {
            card_id: card_id.clone(), kind: "edit".into(), prompt: prompt.clone(), negative: String::new(), seed: 4242,
            width: 1024, height: 576, steps: 20, cfg, source: Some(asset.clone()), board_dir: dir.clone(),
            resume_prompt_id: None, denoise: None, extra_sources: vec![], mask: None,
        };
        let t = std::time::Instant::now();
        tokio::spawn(comfy::run_job(sink.clone(), c.clone(), req));
        loop {
            let (e, p) = rx.recv().await.unwrap();
            if p["cardId"] != card_id.as_str() { continue; }
            if e == "gen://done" { println!("{:.1}s cfg {cfg} | {prompt} -> {}", t.elapsed().as_secs_f32(), p["asset"]); break; }
            if e == "gen://error" { println!("ERROR {prompt}: {}", p["message"]); break; }
        }
    }
}
