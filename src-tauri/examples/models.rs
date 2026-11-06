//! Live check of the model-setup backend against a running ComfyUI:
//!   cargo run --example models -- [board_dir]
//! Lists installed models, runs the settings "Test" for Flux Klein, SDXL and a
//! deliberately mismatched combo, then a real SDXL generate + variation.
use std::sync::Arc;
use std::time::Instant;

use boardcreate_lib::comfy::{self, Comfy, GenRequest, Sink};
use boardcreate_lib::models::ModelConfig;
use serde_json::Value;

#[tokio::main]
async fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| std::env::temp_dir().join("boardcreate-models").to_string_lossy().into_owned());
    std::fs::create_dir_all(&dir).unwrap();
    let c = Arc::new(Comfy::new(ModelConfig::default(), None));

    let lists = comfy::list_models(&c).await.expect("list models");
    println!("diffusion_models={:?}\ntext_encoders={:?}\nvae={:?}\ncheckpoints={:?}\nclip_types has flux3: {}",
        lists.diffusion_models, lists.text_encoders, lists.vae, lists.checkpoints, lists.clip_types.iter().any(|t| t == "flux3"));

    let flux = ModelConfig::default();
    let sdxl = ModelConfig { family: "checkpoint".into(), ckpt: "sd_xl_base_1.0.safetensors".into(), ..Default::default() };
    let mismatched = ModelConfig { clip: "t5gemma_b_b_ul2.safetensors".into(), ..Default::default() };
    for (name, cfg) in [("flux", &flux), ("sdxl", &sdxl), ("mismatched", &mismatched)] {
        match comfy::test_models(&c, cfg).await {
            Ok(t) => println!("TEST {name}: ok in {:.1}s", t.seconds),
            Err(e) => println!("TEST {name}: error: {}", e.lines().next().unwrap_or("")),
        }
    }

    // Real SDXL generation + variation through the normal job path.
    *c.models.write().await = sdxl;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, Value)>();
    let sink = Sink(Arc::new(move |e, p| { let _ = tx.send((e.to_string(), p)); }));
    tokio::spawn(comfy::ws_loop(sink.clone(), c.clone()));
    let mut source = None;
    for kind in ["t2i", "variation", "edit"] {
        let req = GenRequest {
            card_id: kind.into(), kind: kind.into(),
            prompt: "concept art of a small mossy stone shrine in a misty forest, soft light".into(),
            negative: String::new(), seed: 7, width: 1024, height: 1024, steps: 25, cfg: 6.5,
            source: source.clone(), board_dir: dir.clone(), resume_prompt_id: None, denoise: Some(0.6), extra_sources: vec![],
        };
        let t = Instant::now();
        tokio::spawn(comfy::run_job(sink.clone(), c.clone(), req));
        loop {
            let (e, p) = rx.recv().await.unwrap();
            if p["cardId"] != kind { continue; }
            if e == "gen://done" { println!("SDXL {kind}: done in {:.1}s -> {}", t.elapsed().as_secs_f32(), p["asset"]); source = p["asset"].as_str().map(str::to_string); break; }
            if e == "gen://error" { println!("SDXL {kind}: error: {}", p["message"]); break; }
        }
    }
    println!("assets in {dir}");
}
