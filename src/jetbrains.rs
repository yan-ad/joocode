use crate::provider::Registry;

/// Configuration summary for JetBrains AI Assistant's OpenAI-compatible provider.
///
/// JetBrains persists provider keys in its credential store, so Joocode deliberately
/// does not write IDE configuration or credentials on the user's behalf.
pub fn setup_instructions(registry: &Registry, base_url: &str) -> String {
    let model = registry
        .models()
        .first()
        .map(|model| model.id.as_str())
        .unwrap_or("provider/model");

    format!(
        "JetBrains AI Assistant setup:\n\
         1. Open Settings | Tools | AI Assistant | Providers & API keys.\n\
         2. Add an OpenAI-compatible provider.\n\
         3. Set Base URL to: {base_url}\n\
         4. Set API key to any non-empty local value (for example: joocode).\n\
         5. Select a discovered model, for example: {model}\n\
          Joocode exposes {} discovered model(s) at GET {base_url}/models.",
        registry.models().len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::ConfigPaths, provider::Registry};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn instructions_include_the_endpoint_and_qualified_model() {
        let dir = tempdir().unwrap();
        let config = dir.path().join("opencode.jsonc");
        let auth = dir.path().join("auth.json");
        fs::write(
            &config,
            r#"{ provider: { demo: { options: { baseURL: "https://upstream.test/v1" }, models: { fast: {} } } } }"#,
        )
        .unwrap();
        fs::write(&auth, "{}").unwrap();
        let registry = Registry::load(&ConfigPaths { config, auth }).unwrap();

        let text = setup_instructions(&registry, "http://127.0.0.1:10100/v1");

        assert!(text.contains("http://127.0.0.1:10100/v1"));
        assert!(text.contains("demo/fast"));
    }
}
