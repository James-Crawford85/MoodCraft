//! Model families and the user's model selection.
//!
//! A family is a folder of workflow templates (`workflows/<id>/<kind>.json`)
//! plus a small preset: which loader inputs it needs, default sampling
//! settings, file-name hints for auto-selecting installed models, and which
//! generation kinds it supports. Adding a family = a template folder + one
//! entry in [`FAMILIES`].
//!
//! Every template of a family must use the same loader node ids and inputs
//! (only `{{unet}}`/`{{clip}}`/`{{clip_type}}`/`{{vae}}`/`{{ckpt}}`), so
//! ComfyUI's execution cache keeps the models loaded across kinds.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum Loaders {
    /// Separate diffusion model + text encoder (+ type) + VAE files.
    Split,
    /// One checkpoint file containing model, CLIP and VAE.
    Checkpoint,
}

#[derive(Serialize, Clone, Copy, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Defaults {
    pub size: u32,
    pub steps: u32,
    pub cfg: f64,
    pub edit_steps: u32,
    pub edit_cfg: f64,
    pub variation_strength: f64,
    pub sampler: &'static str,
    pub scheduler: &'static str,
    /// Steps/CFG for the quick 256² "Test" render.
    pub test_steps: u32,
}

/// File-name hints: a file matches a hint when its lowercase name contains
/// every token. Hints are tried in order.
#[derive(Serialize, Clone, Copy, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Hints {
    pub unet: &'static [&'static [&'static str]],
    pub clip: &'static [&'static [&'static str]],
    pub vae: &'static [&'static [&'static str]],
    pub ckpt: &'static [&'static [&'static str]],
}

#[derive(Serialize, Clone, Copy, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Family {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub loaders: Loaders,
    /// Default CLIPLoader `type` (split loaders only).
    pub clip_type: &'static str,
    pub hints: Hints,
    pub defaults: Defaults,
    pub supports_edit: bool,
    pub supports_variation: bool,
    #[serde(skip)]
    templates: &'static [(&'static str, &'static str)],
}

pub static FAMILIES: &[Family] = &[
    Family {
        id: "flux_klein",
        label: "Flux Klein",
        description: "Flux3 Image Klein: diffusion model + Qwen3-VL text encoder (type flux3) + Flux2 VAE. Supports generate, edit and variations.",
        loaders: Loaders::Split,
        clip_type: "flux3",
        hints: Hints {
            unet: &[&["flux3", "klein"], &["klein"], &["flux"]],
            clip: &[&["qwen3vl", "4b", "int8"], &["qwen3vl", "4b"], &["qwen3vl"]],
            vae: &[&["flux2"], &["flux"]],
            ckpt: &[],
        },
        defaults: Defaults {
            size: 768, steps: 20, cfg: 3.5, edit_steps: 20, edit_cfg: 1.0, variation_strength: 0.7,
            sampler: "euler", scheduler: "simple", test_steps: 4,
        },
        supports_edit: true,
        supports_variation: true,
        templates: &[
            ("t2i", include_str!("../workflows/flux_klein/t2i.json")),
            ("edit", include_str!("../workflows/flux_klein/edit.json")),
            ("variation", include_str!("../workflows/flux_klein/variation.json")),
        ],
    },
    Family {
        id: "checkpoint",
        label: "Checkpoint (SD / SDXL)",
        description: "A single checkpoint file (model + CLIP + VAE), e.g. SDXL base. Generate and variations; no instruction edits.",
        loaders: Loaders::Checkpoint,
        clip_type: "",
        hints: Hints { unet: &[], clip: &[], vae: &[], ckpt: &[&["sd_xl"], &["sdxl"], &["xl"], &["sd"]] },
        defaults: Defaults {
            size: 1024, steps: 25, cfg: 6.5, edit_steps: 20, edit_cfg: 1.0, variation_strength: 0.6,
            sampler: "dpmpp_2m", scheduler: "karras", test_steps: 4,
        },
        supports_edit: false,
        supports_variation: true,
        templates: &[
            ("t2i", include_str!("../workflows/checkpoint/t2i.json")),
            ("variation", include_str!("../workflows/checkpoint/variation.json")),
        ],
    },
];

pub fn family(id: &str) -> Option<&'static Family> {
    FAMILIES.iter().find(|f| f.id == id)
}

impl Family {
    pub fn supports(&self, kind: &str) -> bool {
        match kind {
            "edit" => self.supports_edit,
            "variation" => self.supports_variation,
            _ => true,
        }
    }

    pub fn kinds(&self) -> impl Iterator<Item = &'static str> {
        self.templates.iter().map(|(k, _)| *k)
    }

    /// The template for `kind`, preferring `<override_dir>/<family>/<kind>.json`.
    pub fn template(&self, kind: &str, override_dir: Option<&Path>) -> Result<Value, String> {
        if !self.supports(kind) {
            return Err(format!("The {} model family doesn't support {kind}.", self.label));
        }
        if let Some(dir) = override_dir {
            let p = dir.join(self.id).join(format!("{kind}.json"));
            if let Ok(s) = std::fs::read_to_string(&p) {
                return serde_json::from_str(&s).map_err(|e| format!("Bad workflow override {}: {e}", p.display()));
            }
        }
        let (_, src) = self
            .templates
            .iter()
            .find(|(k, _)| *k == kind)
            .ok_or_else(|| format!("{} has no {kind} template", self.label))?;
        serde_json::from_str(src).map_err(|e| format!("Bad builtin workflow {}/{kind}: {e}", self.id))
    }
}

/// The user's model selection, persisted in `<app_config>/models.json`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelConfig {
    pub family: String,
    pub unet: String,
    pub clip: String,
    pub clip_type: String,
    pub vae: String,
    pub ckpt: String,
}

impl Default for ModelConfig {
    fn default() -> Self {
        ModelConfig {
            family: "flux_klein".into(),
            unet: "flux3-image-klein-base-int8-convrot.safetensors".into(),
            clip: "qwen3vl_4b_int8_convrot.safetensors".into(),
            clip_type: "flux3".into(),
            vae: "flux2-vae.safetensors".into(),
            ckpt: String::new(),
        }
    }
}

impl ModelConfig {
    pub fn family(&self) -> Result<&'static Family, String> {
        family(&self.family).ok_or_else(|| format!("Unknown model family '{}'", self.family))
    }

    /// Template variables for loaders and family sampling defaults.
    pub fn vars(&self) -> Result<Vec<(&'static str, Value)>, String> {
        let f = self.family()?;
        let need = |v: &str, what: &str| if v.is_empty() { Err(format!("No {what} selected (⚙ → Models)")) } else { Ok(json!(v)) };
        let mut out = vec![("sampler", json!(f.defaults.sampler)), ("scheduler", json!(f.defaults.scheduler))];
        match f.loaders {
            Loaders::Split => {
                out.push(("unet", need(&self.unet, "diffusion model")?));
                out.push(("clip", need(&self.clip, "text encoder")?));
                out.push(("clip_type", need(&self.clip_type, "text encoder type")?));
                out.push(("vae", need(&self.vae, "VAE")?));
            }
            Loaders::Checkpoint => out.push(("ckpt", need(&self.ckpt, "checkpoint")?)),
        }
        Ok(out)
    }
}

pub fn config_path(config_dir: &Path) -> PathBuf {
    config_dir.join("models.json")
}

pub fn load_config(config_dir: Option<&Path>) -> ModelConfig {
    config_dir
        .and_then(|d| std::fs::read_to_string(config_path(d)).ok())
        .and_then(|s| serde_json::from_str::<ModelConfig>(&s).ok())
        .filter(|c| family(&c.family).is_some())
        .unwrap_or_default()
}

pub fn save_config(config_dir: &Path, cfg: &ModelConfig) -> Result<(), String> {
    std::fs::create_dir_all(config_dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(config_path(config_dir), json).map_err(|e| format!("Save model settings: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Within a family, loader nodes must match across kinds so ComfyUI's
    /// cache keeps models loaded when switching generate/edit/variation.
    #[test]
    fn families_share_loaders() {
        const LOADERS: &[&str] = &["UNETLoader", "CLIPLoader", "VAELoader", "CheckpointLoaderSimple"];
        for f in FAMILIES {
            let t2i = f.template("t2i", None).unwrap();
            let loader_ids: Vec<&String> = t2i
                .as_object()
                .unwrap()
                .iter()
                .filter(|(_, n)| LOADERS.contains(&n["class_type"].as_str().unwrap_or("")))
                .map(|(id, _)| id)
                .collect();
            assert!(!loader_ids.is_empty(), "{} has no loaders", f.id);
            for kind in f.kinds() {
                let wf = f.template(kind, None).unwrap();
                for id in &loader_ids {
                    assert_eq!(wf[*id], t2i[*id], "{}/{kind} loader node {id} differs from t2i", f.id);
                }
                for (id, n) in wf.as_object().unwrap() {
                    if LOADERS.contains(&n["class_type"].as_str().unwrap_or("")) {
                        assert!(loader_ids.contains(&id), "{}/{kind} has an extra loader {id}", f.id);
                    }
                }
            }
        }
    }

    #[test]
    fn unsupported_kind_is_a_clear_error() {
        let f = family("checkpoint").unwrap();
        assert!(f.template("edit", None).unwrap_err().contains("doesn't support edit"));
    }

    #[test]
    fn vars_require_selection() {
        let cfg = ModelConfig { family: "checkpoint".into(), ..Default::default() };
        assert!(cfg.vars().unwrap_err().contains("checkpoint"));
        assert!(ModelConfig::default().vars().is_ok());
    }
}
