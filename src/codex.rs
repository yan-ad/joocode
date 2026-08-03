use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, bail};
use serde_json::{Value, json};
use toml_edit::{DocumentMut, Item, Table, value};

use crate::provider::{ModelInfo, Registry};

const PROVIDER_ID: &str = "joc";
const CRABCODEX_PROVIDER_ID: &str = "crabcodex";
const LEGACY_PROVIDER_ID: &str = "open_initiative";

#[derive(Debug)]
pub struct InstallResult {
    pub config: PathBuf,
    pub catalog: PathBuf,
    pub added_model_count: usize,
    pub total_model_count: usize,
}

pub fn install(registry: &Registry, base_url: &str) -> anyhow::Result<InstallResult> {
    if registry.models().is_empty() {
        bail!("no compatible OpenCode models were discovered");
    }

    let home = codex_home()?;
    fs::create_dir_all(&home)
        .with_context(|| format!("failed to create Codex directory {}", home.display()))?;

    let catalog_path = home.join("joc-models.json");
    let config_path = home.join("config.toml");
    let existing = match fs::read_to_string(&config_path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).context("failed to read Codex configuration"),
    };
    let mut document = existing
        .parse::<DocumentMut>()
        .context("Codex config.toml is not valid TOML")?;

    let previous_catalog = document
        .get("model_catalog_json")
        .and_then(Item::as_str)
        .map(PathBuf::from);
    let bundled_catalog = bundled_catalog()?;
    let existing_catalog = previous_catalog
        .as_deref()
        .filter(|path| *path != catalog_path)
        .map(read_catalog)
        .transpose()?
        .unwrap_or_else(|| json!({ "models": [] }));
    let catalog = merged_catalog(&bundled_catalog, &existing_catalog, registry.models())?;
    fs::write(&catalog_path, serde_json::to_vec_pretty(&catalog)?)
        .with_context(|| format!("failed to write {}", catalog_path.display()))?;

    // Codex selects one provider globally. JustOpenCode is an aggregate provider:
    // native OpenAI model slugs are passed through with Codex's existing auth,
    // while provider/model slugs are routed through OpenCode.
    document["model_provider"] = value(PROVIDER_ID);
    document["model_catalog_json"] = value(catalog_path.to_string_lossy().as_ref());

    if document.get("model_providers").is_none() {
        document.insert("model_providers", Item::Table(Table::new()));
    } else if !document["model_providers"].is_table() {
        bail!("model_providers must be a TOML table");
    }
    let providers = document["model_providers"]
        .as_table_mut()
        .context("model_providers must be a TOML table")?;
    let mut provider = Table::new();
    provider["name"] = value("JustOpenCode");
    provider["base_url"] = value(base_url.trim_end_matches('/'));
    provider["wire_api"] = value("responses");
    provider["requires_openai_auth"] = value(true);
    providers[PROVIDER_ID] = Item::Table(provider);
    providers.remove(CRABCODEX_PROVIDER_ID);
    providers.remove(LEGACY_PROVIDER_ID);

    fs::write(&config_path, document.to_string())
        .with_context(|| format!("failed to write {}", config_path.display()))?;

    for legacy_catalog in [
        home.join("crabcodex-models.json"),
        home.join("open-initiative-models.json"),
    ] {
        match fs::remove_file(&legacy_catalog) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("failed to remove legacy model catalog"),
        }
    }

    Ok(InstallResult {
        config: config_path,
        catalog: catalog_path,
        added_model_count: registry.models().len(),
        total_model_count: catalog["models"].as_array().map_or(0, Vec::len),
    })
}

fn bundled_catalog() -> anyhow::Result<Value> {
    let output = Command::new("codex")
        .args(["debug", "models", "--bundled"])
        .output()
        .context("failed to run `codex debug models --bundled`; is Codex installed?")?;
    if !output.status.success() {
        bail!(
            "failed to read Codex bundled models: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice(&output.stdout).context("Codex returned an invalid bundled catalog")
}

fn read_catalog(path: &Path) -> anyhow::Result<Value> {
    match fs::read(path) {
        Ok(content) => serde_json::from_slice(&content)
            .with_context(|| format!("{} is not valid JSON", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(json!({ "models": [] })),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn merged_catalog(
    bundled: &Value,
    existing: &Value,
    opencode_models: &[ModelInfo],
) -> anyhow::Result<Value> {
    let mut merged = Vec::new();
    let mut slugs = HashSet::new();
    for catalog in [bundled, existing] {
        let models = catalog
            .get("models")
            .and_then(Value::as_array)
            .context("model catalog must contain a models array")?;
        for model in models {
            let Some(slug) = model.get("slug").and_then(Value::as_str) else {
                continue;
            };
            if slugs.insert(slug.to_owned()) {
                merged.push(model.clone());
            }
        }
    }
    for model in opencode_models {
        let preset = model_preset(model, false);
        if slugs.insert(model.id.clone()) {
            merged.push(preset);
        }
    }
    Ok(json!({ "models": merged }))
}

fn codex_home() -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let home = dirs::home_dir().context("could not determine home directory")?;
    Ok(home.join(".codex"))
}

fn model_preset(model: &ModelInfo, is_default: bool) -> Value {
    let reasoning_efforts = if model.reasoning {
        vec![
            json!({"effort": "low", "description": "Fast responses with light reasoning"}),
            json!({"effort": "medium", "description": "Balanced reasoning depth and latency"}),
            json!({"effort": "high", "description": "Deeper reasoning for complex tasks"}),
        ]
    } else {
        Vec::new()
    };

    json!({
        "slug": model.id,
        // Codex Desktop primarily renders display_name in the picker. Keep it
        // identical to the routable ID so models never collapse into an
        // ambiguous "Custom" label and users see provider/model explicitly.
        "display_name": model.id,
        "description": format!("{} ({})", model.name, model.provider),
        "default_reasoning_level": if model.reasoning { "medium" } else { "none" },
        "supported_reasoning_levels": reasoning_efforts,
        "shell_type": "shell_command",
        "visibility": "list",
        "supported_in_api": true,
        "priority": if is_default { 0 } else { 1 },
        "additional_speed_tiers": [],
        "service_tiers": [],
        "default_service_tier": null,
        "availability_nux": null,
        "upgrade": null,
        "base_instructions": "You are Codex, a coding agent. Follow the user's instructions and use the supplied tools to work in their repository.",
        "model_messages": null,
        "include_skills_usage_instructions": false,
        "supports_reasoning_summaries": model.reasoning,
        "supports_reasoning_summary_parameter": false,
        "default_reasoning_summary": "auto",
        "support_verbosity": false,
        "default_verbosity": null,
        "apply_patch_tool_type": "freeform",
        "web_search_tool_type": "text",
        "truncation_policy": { "mode": "tokens", "limit": 10000 },
        "supports_parallel_tool_calls": true,
        "supports_image_detail_original": false,
        "context_window": model.context_window,
        "max_context_window": model.context_window,
        "auto_compact_token_limit": null,
        "comp_hash": null,
        "effective_context_window_percent": 95,
        "experimental_supported_tools": [],
        "input_modalities": ["text", "image"],
        "prefer_websockets": false,
        "supports_search_tool": false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_models_are_visible_and_keep_qualified_ids() {
        let models = vec![ModelInfo {
            id: "demo/model-a".into(),
            provider: "demo".into(),
            upstream_id: "model-a".into(),
            name: "Model A".into(),
            reasoning: true,
            context_window: Some(1000),
            max_output_tokens: Some(100),
        }];
        let catalog =
            merged_catalog(&json!({ "models": [] }), &json!({ "models": [] }), &models).unwrap();
        let model = &catalog["models"][0];
        assert_eq!(model["slug"], "demo/model-a");
        assert_eq!(model["display_name"], "demo/model-a");
        assert_eq!(model["description"], "Model A (demo)");
        assert_eq!(model["visibility"], "list");
        assert_eq!(model["default_reasoning_level"], "medium");
    }

    #[test]
    fn merges_bundled_existing_and_opencode_models_without_duplicates() {
        let bundled = json!({ "models": [
            { "slug": "gpt-5.4", "display_name": "GPT-5.4" }
        ]});
        let existing = json!({ "models": [
            { "slug": "gpt-5.4", "display_name": "duplicate" },
            { "slug": "local/model", "display_name": "Local" }
        ]});
        let models = vec![ModelInfo {
            id: "demo/model-a".into(),
            provider: "demo".into(),
            upstream_id: "model-a".into(),
            name: "Model A".into(),
            reasoning: false,
            context_window: None,
            max_output_tokens: None,
        }];
        let merged = merged_catalog(&bundled, &existing, &models).unwrap();
        let slugs = merged["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["slug"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(slugs, ["gpt-5.4", "local/model", "demo/model-a"]);
    }
}
