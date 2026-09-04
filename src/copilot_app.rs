use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, ensure};
use keyring::Entry;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;

use crate::provider::Registry;

const PROVIDER_ID: &str = "6a6f6f63-6f64-4565-8000-000000000001";
const PROVIDER_NAME: &str = "Joocode";
const KEYRING_SERVICE: &str = "github-copilot-app";
const LOCAL_API_KEY: &str = "joocode-local";
const MANAGED_PROVIDER_KEY: &str = "joocode-managed-provider-id";

pub fn installed() -> bool {
    database_path().is_some_and(|path| path.is_file()) || application_installed()
}

pub fn install(registry: &Registry, base_url: &str) -> anyhow::Result<()> {
    let path = database_path().context("cannot determine GitHub Copilot app data directory")?;
    let provider_id = install_at(registry, base_url, &path)?;
    store_credential(&provider_id)?;
    Ok(())
}

pub fn uninstall() -> anyhow::Result<()> {
    let Some(path) = database_path().filter(|path| path.is_file()) else {
        delete_credential(PROVIDER_ID);
        return Ok(());
    };
    let mut connection = open_database(&path)?;
    ensure_schema(&connection)?;
    let transaction = connection.transaction()?;
    let provider_ids = managed_provider_ids(&transaction)?;
    for provider_id in &provider_ids {
        transaction.execute(
            "DELETE FROM model_providers WHERE id = ?1",
            params![provider_id],
        )?;
    }

    invalidate_model_cache(&transaction)?;
    for provider_id in &provider_ids {
        transaction.execute(
            "DELETE FROM app_state WHERE key = 'copilot-selected-model' AND value LIKE ?1",
            params![format!("{provider_id}/%")],
        )?;
    }
    transaction.execute(
        "DELETE FROM app_state WHERE key = ?1",
        params![MANAGED_PROVIDER_KEY],
    )?;
    transaction.commit()?;
    for provider_id in provider_ids {
        delete_credential(&provider_id);
    }
    Ok(())
}

fn install_at(registry: &Registry, base_url: &str, path: &Path) -> anyhow::Result<String> {
    ensure!(
        path.is_file(),
        "GitHub Copilot app database was not found at {}; open the app once, then retry",
        path.display()
    );
    let mut connection = open_database(path)?;
    ensure_schema(&connection)?;
    let transaction = connection.transaction()?;
    let provider_id = resolve_provider_id(&transaction, base_url)?;
    let settings = json!({
        "authKind": "api_key",
        "baseUrl": base_url.trim_end_matches('/'),
        "headersJson": "{}",
        "wireApi": "completions"
    })
    .to_string();
    transaction.execute(
        "INSERT INTO model_providers (id, name, type, settings_json, account_id, updated_at)
         VALUES (?1, ?2, 'custom', ?3, NULL, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
         ON CONFLICT(id) DO UPDATE SET
           name = excluded.name,
           type = excluded.type,
           settings_json = excluded.settings_json,
           account_id = NULL,
           updated_at = excluded.updated_at",
        params![provider_id, PROVIDER_NAME, settings],
    )?;
    transaction.execute(
        "INSERT INTO app_state (key, value, updated_at)
         VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        params![MANAGED_PROVIDER_KEY, provider_id],
    )?;

    let model_ids = registry
        .models()
        .iter()
        .map(|model| model.id.as_str())
        .collect::<Vec<_>>();
    for model in registry.models() {
        let existing_id = transaction
            .query_row(
                "SELECT id FROM provider_models WHERE provider_id = ?1 AND model_id = ?2",
                params![provider_id, model.id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let id = existing_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        transaction.execute(
            "INSERT INTO provider_models (
                id, provider_id, model_id, wire_model, display_name,
                max_prompt_tokens, max_output_tokens, wire_api_override,
                supported_reasoning_efforts, updated_at
             ) VALUES (
                ?1, ?2, ?3, NULL, ?4, ?5, ?6, NULL, ?7,
                strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             )
             ON CONFLICT(provider_id, model_id) DO UPDATE SET
                display_name = excluded.display_name,
                max_prompt_tokens = excluded.max_prompt_tokens,
                max_output_tokens = excluded.max_output_tokens,
                wire_api_override = excluded.wire_api_override,
                supported_reasoning_efforts = excluded.supported_reasoning_efforts,
                updated_at = excluded.updated_at",
            params![
                id,
                provider_id,
                model.id,
                model.name,
                model
                    .context_window
                    .and_then(|value| i64::try_from(value).ok()),
                model
                    .max_output_tokens
                    .and_then(|value| i64::try_from(value).ok()),
                model.reasoning.then_some(r#"["low","medium","high"]"#),
            ],
        )?;
    }

    if model_ids.is_empty() {
        transaction.execute(
            "DELETE FROM provider_models WHERE provider_id = ?1",
            params![provider_id],
        )?;
    } else {
        let placeholders = std::iter::repeat_n("?", model_ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "DELETE FROM provider_models WHERE provider_id = ? AND model_id NOT IN ({placeholders})"
        );
        let mut values = Vec::<&dyn rusqlite::ToSql>::with_capacity(model_ids.len() + 1);
        values.push(&provider_id);
        values.extend(model_ids.iter().map(|id| id as &dyn rusqlite::ToSql));
        transaction.execute(&sql, values.as_slice())?;
    }
    invalidate_model_cache(&transaction)?;
    transaction.commit()?;
    Ok(provider_id)
}

fn resolve_provider_id(
    transaction: &rusqlite::Transaction<'_>,
    base_url: &str,
) -> anyhow::Result<String> {
    if let Some(provider_id) = transaction
        .query_row(
            "SELECT value FROM app_state WHERE key = ?1",
            params![MANAGED_PROVIDER_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .filter(|provider_id| provider_exists(transaction, provider_id).unwrap_or(false))
    {
        return Ok(provider_id);
    }
    if provider_exists(transaction, PROVIDER_ID)? {
        return Ok(PROVIDER_ID.to_owned());
    }

    let normalized = base_url.trim_end_matches('/');
    let mut statement = transaction.prepare(
        "SELECT id, settings_json FROM model_providers WHERE type = 'custom' ORDER BY created_at",
    )?;
    let candidates = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for candidate in candidates {
        let (provider_id, settings) = candidate?;
        let matches = serde_json::from_str::<serde_json::Value>(&settings)
            .ok()
            .and_then(|settings| settings.get("baseUrl")?.as_str().map(str::to_owned))
            .is_some_and(|existing| existing.trim_end_matches('/') == normalized);
        if matches {
            return Ok(provider_id);
        }
    }
    Ok(PROVIDER_ID.to_owned())
}

fn provider_exists(
    transaction: &rusqlite::Transaction<'_>,
    provider_id: &str,
) -> anyhow::Result<bool> {
    Ok(transaction
        .query_row(
            "SELECT 1 FROM model_providers WHERE id = ?1",
            params![provider_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn managed_provider_ids(transaction: &rusqlite::Transaction<'_>) -> anyhow::Result<Vec<String>> {
    let mut ids = vec![PROVIDER_ID.to_owned()];
    if let Some(provider_id) = transaction
        .query_row(
            "SELECT value FROM app_state WHERE key = ?1",
            params![MANAGED_PROVIDER_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        && !ids.contains(&provider_id)
    {
        ids.push(provider_id);
    }
    Ok(ids)
}

fn open_database(path: &Path) -> anyhow::Result<Connection> {
    let connection = Connection::open(path).with_context(|| {
        format!(
            "failed opening GitHub Copilot app database at {}",
            path.display()
        )
    })?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    Ok(connection)
}

fn ensure_schema(connection: &Connection) -> anyhow::Result<()> {
    for table in ["model_providers", "provider_models", "app_state"] {
        let found = connection
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                params![table],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        ensure!(
            found,
            "GitHub Copilot app database is missing the {table} table; update or open the app once"
        );
    }
    Ok(())
}

fn invalidate_model_cache(transaction: &rusqlite::Transaction<'_>) -> anyhow::Result<()> {
    transaction.execute(
        "DELETE FROM app_state WHERE key = 'copilot-available-models-v2'",
        [],
    )?;
    Ok(())
}

fn database_path() -> Option<PathBuf> {
    std::env::var_os("COPILOT_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".copilot")))
        .map(|root| root.join("data.db"))
}

fn credential_account(provider_id: &str) -> String {
    format!("byok:{provider_id}:apiKey")
}

fn store_credential(provider_id: &str) -> anyhow::Result<()> {
    Entry::new(KEYRING_SERVICE, &credential_account(provider_id))
        .context("failed opening the GitHub Copilot app credential store")?
        .set_password(LOCAL_API_KEY)
        .context("failed storing Joocode's local GitHub Copilot app credential")
}

fn delete_credential(provider_id: &str) {
    if let Ok(entry) = Entry::new(KEYRING_SERVICE, &credential_account(provider_id)) {
        let _ = entry.delete_credential();
    }
}

fn application_installed() -> bool {
    #[cfg(target_os = "macos")]
    {
        return [
            "/Applications/GitHub Copilot.app",
            "/Applications/GitHub.app",
        ]
        .iter()
        .any(|path| Path::new(path).exists())
            || dirs::home_dir().is_some_and(|home| {
                home.join("Applications/GitHub Copilot.app").exists()
                    || home.join("Applications/GitHub.app").exists()
            });
    }
    #[cfg(target_os = "windows")]
    {
        return ["LOCALAPPDATA", "PROGRAMFILES"]
            .into_iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from)
            .any(|root| {
                [
                    "Programs/GitHub Copilot/GitHub Copilot.exe",
                    "GitHub Copilot/GitHub Copilot.exe",
                    "GitHub Copilot/GitHub.exe",
                ]
                .iter()
                .any(|path| root.join(path).exists())
            });
    }
    #[cfg(target_os = "linux")]
    {
        return dirs::home_dir().is_some_and(|home| {
            home.join(".local/share/applications/github-copilot.desktop")
                .exists()
        }) || Path::new("/usr/share/applications/github-copilot.desktop").exists();
    }
    #[allow(unreachable_code)]
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::ConfigPaths, provider::Registry};

    fn create_schema(path: &Path) {
        let connection = Connection::open(path).unwrap();
        connection.execute_batch(
            "CREATE TABLE model_providers (
                id TEXT PRIMARY KEY NOT NULL,
                name TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                type TEXT NOT NULL DEFAULT 'openai',
                settings_json TEXT NOT NULL DEFAULT '{}',
                account_id TEXT
             );
             CREATE TABLE provider_models (
                id TEXT PRIMARY KEY NOT NULL,
                provider_id TEXT NOT NULL REFERENCES model_providers(id) ON DELETE CASCADE,
                model_id TEXT NOT NULL,
                wire_model TEXT,
                display_name TEXT NOT NULL,
                max_prompt_tokens INTEGER,
                max_output_tokens INTEGER,
                wire_api_override TEXT,
                created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                supported_reasoning_efforts TEXT,
                UNIQUE (provider_id, model_id)
             );
             CREATE TABLE app_state (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL, updated_at TEXT);
             INSERT INTO model_providers (id, name, type, settings_json) VALUES ('existing', 'GitHub Copilot', 'github_copilot', '{}');
             INSERT INTO app_state (key, value) VALUES ('copilot-available-models-v2', 'stale');"
        ).unwrap();
    }

    fn registry() -> Registry {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("opencode.jsonc");
        let auth = directory.path().join("auth.json");
        std::fs::write(
            &config,
            r#"{"provider":{"clip":{"npm":"@ai-sdk/openai-compatible","options":{"baseURL":"https://example.com/v1"},"models":{"gpt-5.4":{"name":"GPT 5.4","limit":{"context":200000,"output":64000}}}}}}"#,
        ).unwrap();
        std::fs::write(&auth, r#"{"clip":{"type":"api","key":"secret"}}"#).unwrap();
        Registry::load(&ConfigPaths { config, auth }).unwrap()
    }

    #[test]
    fn install_preserves_existing_provider_and_syncs_models() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("data.db");
        create_schema(&path);

        install_at(&registry(), "http://127.0.0.1:10100/v1", &path).unwrap();

        let connection = Connection::open(path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM model_providers WHERE id = 'existing'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        let settings: String = connection
            .query_row(
                "SELECT settings_json FROM model_providers WHERE id = ?1",
                params![PROVIDER_ID],
                |row| row.get(0),
            )
            .unwrap();
        assert!(settings.contains("http://127.0.0.1:10100/v1"));
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM provider_models WHERE provider_id = ?1",
                    params![PROVIDER_ID],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM app_state WHERE key = 'copilot-available-models-v2'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn reinstall_removes_stale_joocode_models() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("data.db");
        create_schema(&path);
        let connection = Connection::open(&path).unwrap();
        connection.execute("INSERT INTO model_providers (id,name,type,settings_json) VALUES (?1,'Joocode','custom','{}')", params![PROVIDER_ID]).unwrap();
        connection.execute("INSERT INTO provider_models (id,provider_id,model_id,display_name) VALUES ('stale',?1,'stale/model','stale/model')", params![PROVIDER_ID]).unwrap();
        drop(connection);

        install_at(&registry(), "http://127.0.0.1:10100/v1", &path).unwrap();
        let connection = Connection::open(path).unwrap();
        assert_eq!(
            connection.query_row("SELECT COUNT(*) FROM provider_models WHERE provider_id = ?1 AND model_id = 'stale/model'", params![PROVIDER_ID], |row| row.get::<_, i64>(0)).unwrap(),
            0
        );
    }

    #[test]
    fn uninstall_preserves_other_providers_and_clears_joocode_selection() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("data.db");
        create_schema(&path);
        let mut connection = open_database(&path).unwrap();
        let transaction = connection.transaction().unwrap();
        transaction
            .execute(
                "INSERT INTO model_providers (id,name,type,settings_json) VALUES (?1,'Joocode','custom','{}')",
                params![PROVIDER_ID],
            )
            .unwrap();
        transaction
            .execute(
                "INSERT INTO app_state (key,value) VALUES ('copilot-selected-model', ?1)",
                params![format!("{PROVIDER_ID}/clip/gpt-5.4")],
            )
            .unwrap();
        transaction
            .execute(
                "DELETE FROM model_providers WHERE id = ?1",
                params![PROVIDER_ID],
            )
            .unwrap();
        invalidate_model_cache(&transaction).unwrap();
        transaction
            .execute(
                "DELETE FROM app_state WHERE key = 'copilot-selected-model' AND value LIKE ?1",
                params![format!("{PROVIDER_ID}/%")],
            )
            .unwrap();
        transaction.commit().unwrap();

        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM model_providers WHERE id = 'existing'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM app_state WHERE key = 'copilot-selected-model'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
    }
    #[test]
    fn install_adopts_existing_manual_local_endpoint() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("data.db");
        create_schema(&path);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO model_providers (id,name,type,settings_json) VALUES ('manual','Custom endpoint','custom',?1)",
                params![r#"{"authKind":"api_key","baseUrl":"http://127.0.0.1:10100/v1","headersJson":"{}","wireApi":"completions"}"#],
            )
            .unwrap();
        drop(connection);

        let provider_id = install_at(&registry(), "http://127.0.0.1:10100/v1", &path).unwrap();

        assert_eq!(provider_id, "manual");
        let connection = Connection::open(path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM model_providers WHERE id = ?1",
                    params![PROVIDER_ID],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT name FROM model_providers WHERE id = 'manual'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            PROVIDER_NAME
        );
    }
}
