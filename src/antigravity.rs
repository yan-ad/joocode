use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(target_os = "macos")]
use std::process::Stdio;

use anyhow::{Context, ensure};
use axum::{
    body::Body,
    http::{HeaderMap, Method, StatusCode, Uri, header},
    response::Response,
};
use bytes::Bytes;
use futures_util::TryStreamExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{error::ApiError, provider::Registry};

const GOOGLE_API: &str = "https://generativelanguage.googleapis.com";
const CLOUD_CODE_API: &str = "https://daily-cloudcode-pa.googleapis.com";
const OFFICIAL_GOOGLE_LITERAL: &str = "'https://generativelanguage.googleapis.com'";
const OFFICIAL_CLOUD_LITERAL: &str = "'https://daily-cloudcode-pa.googleapis.com'";
const PATCHED_APP_NAME: &str = "Antigravity Joocode.app";
#[cfg(target_os = "macos")]
const OFFICIAL_APP_NAME: &str = "Antigravity.app";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PatchStatus {
    Missing,
    Available { version: String },
    Installed { version: String, app: PathBuf },
    Stale { version: String, app: PathBuf },
}

fn model_from_uri(uri: &Uri) -> Option<&str> {
    let path = uri.path();
    let models = path.split("/models/").nth(1)?;
    models
        .split_once(':')
        .map(|(model, _)| model)
        .filter(|model| !model.is_empty())
}

impl PatchStatus {
    pub fn render(&self) -> String {
        match self {
            Self::Missing => "Antigravity is not installed.".into(),
            Self::Available { version } => {
                format!("Antigravity {version} is available for the Joocode bridge patch.")
            }
            Self::Installed { version, app } => format!(
                "Antigravity Joocode {version} is installed at {}.",
                app.display()
            ),
            Self::Stale { version, app } => format!(
                "Antigravity Joocode at {} is stale; rebuild it from Antigravity {version}.",
                app.display()
            ),
        }
    }
}

pub fn patch_installed() -> bool {
    patched_app().is_some_and(|path| path.exists())
}

pub fn status(base_url: &str) -> anyhow::Result<PatchStatus> {
    let Some(official) = official_app() else {
        return Ok(PatchStatus::Missing);
    };
    let version = app_version(&official).unwrap_or_else(|| "unknown".into());
    let Some(patched) = patched_app() else {
        return Ok(PatchStatus::Available { version });
    };
    if !patched.exists() {
        return Ok(PatchStatus::Available { version });
    }
    let archive = patched.join("Contents/Resources/app.asar");
    let expected = bridge_origin(base_url)?;
    let bytes =
        fs::read(&archive).with_context(|| format!("failed to read {}", archive.display()))?;
    let marker = fixed_js_literal(&expected)?;
    let patched_version = app_version(&patched).unwrap_or_default();
    if patched_version == version && count_bytes(&bytes, marker.as_bytes()) >= 2 {
        Ok(PatchStatus::Installed {
            version,
            app: patched,
        })
    } else {
        Ok(PatchStatus::Stale {
            version,
            app: patched,
        })
    }
}

pub fn install(base_url: &str) -> anyhow::Result<PathBuf> {
    ensure!(
        cfg!(target_os = "macos"),
        "the Antigravity patched-app integration currently supports macOS"
    );
    if let PatchStatus::Installed { app, .. } = status(base_url)? {
        return Ok(app);
    }
    let official = official_app().context("Antigravity.app is not installed")?;
    let destination = patched_app().context("cannot determine the user Applications directory")?;
    let parent = destination
        .parent()
        .context("invalid Antigravity destination")?;
    fs::create_dir_all(parent)?;

    let staging = parent.join(format!(
        ".{PATCHED_APP_NAME}.staging-{}",
        std::process::id()
    ));
    remove_path(&staging)?;
    remove_path(&destination)?;

    let mut clone = Command::new("/bin/cp");
    clone.args(["-cR"]).arg(&official).arg(&staging);
    let mut status = clone.status().context("failed to clone Antigravity.app")?;
    if !status.success() {
        remove_path(&staging)?;
        status = Command::new("/bin/cp")
            .args(["-R"])
            .arg(&official)
            .arg(&staging)
            .status()
            .context("failed to copy Antigravity.app")?;
    }
    ensure!(status.success(), "failed to clone Antigravity.app");

    let result = (|| {
        let archive = staging.join("Contents/Resources/app.asar");
        patch_archive(&archive, &bridge_origin(base_url)?)?;
        update_bundle_metadata(&staging, &archive)?;
        sign_bundle(&staging)?;
        fs::rename(&staging, &destination)
            .with_context(|| format!("failed to install {}", destination.display()))?;
        Ok::<_, anyhow::Error>(())
    })();
    if result.is_err() {
        let _ = remove_path(&staging);
    }
    result?;
    Ok(destination)
}

pub fn restore() -> anyhow::Result<()> {
    if let Some(path) = patched_app() {
        remove_path(&path)?;
    }
    Ok(())
}

fn official_app() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("JOOCODE_ANTIGRAVITY_APP").map(PathBuf::from) {
        return path.exists().then_some(path);
    }
    #[cfg(target_os = "macos")]
    {
        let system = PathBuf::from("/Applications").join(OFFICIAL_APP_NAME);
        if system.exists() {
            return Some(system);
        }
        dirs::home_dir()
            .map(|home| home.join("Applications").join(OFFICIAL_APP_NAME))
            .filter(|path| path.exists())
    }
    #[cfg(target_os = "windows")]
    {
        let root = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
        [
            root.join("Programs/Antigravity/Antigravity.exe"),
            root.join("Programs/Antigravity IDE/Antigravity.exe"),
        ]
        .into_iter()
        .find(|path| path.exists())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

fn patched_app() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("JOOCODE_ANTIGRAVITY_PATCHED_APP").map(PathBuf::from) {
        return Some(path);
    }
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir().map(|home| home.join("Applications").join(PATCHED_APP_NAME))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|root| root.join("Programs/Antigravity Joocode"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

fn app_version(app: &Path) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("/usr/bin/defaults")
            .arg("read")
            .arg(app.join("Contents/Info"))
            .arg("CFBundleShortVersionString")
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        None
    }
}

fn bridge_origin(base_url: &str) -> anyhow::Result<String> {
    let parsed = reqwest::Url::parse(base_url).context("invalid Joocode base URL")?;
    ensure!(
        matches!(parsed.host_str(), Some("127.0.0.1" | "localhost")),
        "Antigravity patch currently requires a localhost Joocode URL"
    );
    let port = parsed
        .port_or_known_default()
        .context("Joocode base URL has no port")?;
    Ok(format!("http://127.0.0.1:{port}"))
}

fn fixed_js_literal(origin: &str) -> anyhow::Result<String> {
    let quoted = format!("'{origin}'");
    ensure!(
        quoted.len() <= OFFICIAL_GOOGLE_LITERAL.len() - 4,
        "Joocode URL is too long for the safe Antigravity patch"
    );
    let padding = OFFICIAL_GOOGLE_LITERAL.len() - quoted.len() - 4;
    Ok(format!("{quoted}/*{}*/", ".".repeat(padding)))
}

fn patch_archive(path: &Path, origin: &str) -> anyhow::Result<()> {
    let mut archive =
        fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    ensure!(archive.len() >= 16, "invalid Antigravity app.asar");
    let header_size = u32::from_le_bytes(archive[4..8].try_into().expect("four bytes")) as usize;
    let json_size = u32::from_le_bytes(archive[12..16].try_into().expect("four bytes")) as usize;
    ensure!(
        archive.len() >= 16 + json_size,
        "truncated Antigravity app.asar header"
    );
    let header: Value = serde_json::from_slice(&archive[16..16 + json_size])
        .context("invalid Antigravity app.asar header")?;
    let file = header
        .pointer("/files/dist/files/languageServer.js")
        .context("Antigravity launcher dist/languageServer.js is missing")?;
    let size = file
        .get("size")
        .and_then(Value::as_u64)
        .context("launcher size is missing")? as usize;
    let offset = file
        .get("offset")
        .and_then(Value::as_str)
        .context("launcher offset is missing")?
        .parse::<usize>()
        .context("invalid launcher offset")?;
    let data_start = header_size + 8 + offset;
    let data_end = data_start + size;
    ensure!(
        data_end <= archive.len(),
        "Antigravity launcher is truncated"
    );

    let old_hash = file
        .pointer("/integrity/hash")
        .and_then(Value::as_str)
        .context("launcher integrity hash is missing")?
        .to_owned();
    let replacement = fixed_js_literal(origin)?;
    let launcher = &mut archive[data_start..data_end];
    replace_once(
        launcher,
        OFFICIAL_GOOGLE_LITERAL.as_bytes(),
        replacement.as_bytes(),
    )?;
    replace_once(
        launcher,
        OFFICIAL_CLOUD_LITERAL.as_bytes(),
        replacement.as_bytes(),
    )?;
    let new_hash = format!("{:x}", Sha256::digest(launcher));
    replace_all_equal(
        &mut archive[16..16 + json_size],
        old_hash.as_bytes(),
        new_hash.as_bytes(),
    )?;

    let temporary = path.with_extension("asar.joocode.tmp");
    let mut output = fs::File::create(&temporary)?;
    output.write_all(&archive)?;
    output.sync_all()?;
    fs::rename(&temporary, path)?;
    Ok(())
}

fn replace_once(haystack: &mut [u8], needle: &[u8], replacement: &[u8]) -> anyhow::Result<()> {
    ensure!(
        needle.len() == replacement.len(),
        "safe patch replacement length mismatch"
    );
    let positions = find_positions(haystack, needle);
    ensure!(
        positions.len() == 1,
        "expected one Antigravity launcher patch point, found {}",
        positions.len()
    );
    let start = positions[0];
    haystack[start..start + needle.len()].copy_from_slice(replacement);
    Ok(())
}

fn replace_all_equal(haystack: &mut [u8], needle: &[u8], replacement: &[u8]) -> anyhow::Result<()> {
    ensure!(
        needle.len() == replacement.len(),
        "integrity replacement length mismatch"
    );
    let positions = find_positions(haystack, needle);
    ensure!(
        !positions.is_empty(),
        "launcher integrity hash was not found in the ASAR header"
    );
    for start in positions {
        haystack[start..start + needle.len()].copy_from_slice(replacement);
    }
    Ok(())
}

fn find_positions(haystack: &[u8], needle: &[u8]) -> Vec<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return Vec::new();
    }
    haystack
        .windows(needle.len())
        .enumerate()
        .filter_map(|(index, value)| (value == needle).then_some(index))
        .collect()
}

fn count_bytes(haystack: &[u8], needle: &[u8]) -> usize {
    find_positions(haystack, needle).len()
}

#[cfg(target_os = "macos")]
fn update_bundle_metadata(app: &Path, archive: &Path) -> anyhow::Result<()> {
    let plist = app.join("Contents/Info.plist");
    let archive_hash = format!("{:x}", Sha256::digest(fs::read(archive)?));
    for (key, value) in [
        (":CFBundleDisplayName", "Antigravity Joocode"),
        (":CFBundleName", "Antigravity Joocode"),
        (":CFBundleIdentifier", "com.google.antigravity.joocode"),
        (
            ":ElectronAsarIntegrity:Resources/app.asar:hash",
            archive_hash.as_str(),
        ),
    ] {
        let status = Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", &format!("Set '{key}' '{value}'")])
            .arg(&plist)
            .status()?;
        ensure!(
            status.success(),
            "failed to update Antigravity Joocode bundle metadata"
        );
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn update_bundle_metadata(_app: &Path, _archive: &Path) -> anyhow::Result<()> {
    anyhow::bail!("Antigravity patch installation is not implemented on this platform yet")
}

#[cfg(target_os = "macos")]
fn sign_bundle(app: &Path) -> anyhow::Result<()> {
    let status = Command::new("/usr/bin/codesign")
        .args(["--force", "--deep", "--sign", "-"])
        .arg(app)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status()
        .context("failed to ad-hoc sign Antigravity Joocode")?;
    ensure!(
        status.success(),
        "failed to ad-hoc sign Antigravity Joocode"
    );
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn sign_bundle(_app: &Path) -> anyhow::Result<()> {
    Ok(())
}

fn remove_path(path: &Path) -> anyhow::Result<()> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path)?,
        Ok(_) => fs::remove_file(path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

pub async fn handle(
    registry: &Registry,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let path = uri.path();
    if path.contains("fetchAvailableModels") {
        return available_models(registry, method, uri, headers, body).await;
    }
    if path.contains("generateContent") {
        let request: Value = serde_json::from_slice(&body).map_err(|error| {
            ApiError::bad_request(format!("invalid Antigravity request: {error}"))
        })?;
        if let Some(model) = requested_model(&request).or_else(|| model_from_uri(&uri))
            && let Some(model_info) = find_model(registry, model)
        {
            let stream = path.contains("streamGenerateContent")
                || uri.query().is_some_and(|query| query.contains("alt=sse"));
            return generate(registry, model_info.id.as_str(), request, stream).await;
        }
    }
    forward_google(registry, method, uri, headers, body).await
}

async fn available_models(
    registry: &Registry,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let custom = custom_models(registry);
    match forward_google(registry, method, uri, headers, body).await {
        Ok(response) if response.status().is_success() => {
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), 16 * 1024 * 1024)
                .await
                .map_err(|error| ApiError::upstream(StatusCode::BAD_GATEWAY, error.to_string()))?;
            let mut value: Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
            merge_custom_models(&mut value, custom);
            Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(value.to_string()))
                .map_err(|error| {
                    ApiError::upstream(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
                })
        }
        _ => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"models": custom}).to_string()))
            .map_err(|error| {
                ApiError::upstream(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            }),
    }
}

fn custom_models(registry: &Registry) -> serde_json::Map<String, Value> {
    registry
        .models()
        .iter()
        .map(|model| {
            let slug = model_slug(&model.id);
            let value = json!({
                "displayName": model.id,
                "description": format!("{} through Joocode", model.name),
                "supportsImages": true,
                "supportsThinking": model.reasoning,
                "recommended": true,
                "maxTokens": model.context_window.unwrap_or(128_000),
                "maxOutputTokens": model.max_output_tokens.unwrap_or(16_384),
                "tokenizerType": "LLAMA_WITH_SPECIAL",
                "model": placeholder(&model.id),
                "apiProvider": "API_PROVIDER_GOOGLE_GEMINI",
                "modelProvider": "MODEL_PROVIDER_GOOGLE",
                "supportedGenerationMethods": ["generateContent", "countTokens"]
            });
            (slug, value)
        })
        .collect()
}

fn merge_custom_models(value: &mut Value, custom: serde_json::Map<String, Value>) {
    let target_key = ["models", "availableModels", "available_models"]
        .into_iter()
        .find(|key| value.get(*key).is_some());
    let target = target_key.and_then(|key| value.get_mut(key));
    match target {
        Some(Value::Object(models)) => {
            for (key, entry) in custom {
                models.insert(key, entry);
            }
        }
        Some(Value::Array(models)) => {
            let mut inserted = custom.into_values().collect::<Vec<_>>();
            inserted.append(models);
            *models = inserted;
        }
        _ => value["models"] = Value::Object(custom),
    }
    let slugs = value
        .get("models")
        .and_then(Value::as_object)
        .map(|models| {
            models
                .keys()
                .filter(|name| name.starts_with("joocode-"))
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(sorts) = value
        .get_mut("agentModelSorts")
        .and_then(Value::as_array_mut)
    {
        for sort in sorts {
            for group in sort
                .get_mut("groups")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                if let Some(ids) = group.get_mut("modelIds").and_then(Value::as_array_mut) {
                    for slug in &slugs {
                        if !ids.iter().any(|id| id.as_str() == Some(slug)) {
                            ids.push(Value::String(slug.clone()));
                        }
                    }
                }
            }
        }
    }
}

fn requested_model(request: &Value) -> Option<&str> {
    request
        .get("model")
        .or_else(|| request.get("modelId"))
        .or_else(|| request.get("model_id"))
        .and_then(Value::as_str)
}

fn find_model<'a>(
    registry: &'a Registry,
    requested: &str,
) -> Option<&'a crate::provider::ModelInfo> {
    registry.models().iter().find(|model| {
        requested == model.id
            || requested == model_slug(&model.id)
            || requested == placeholder(&model.id)
            || requested.strip_prefix("models/") == Some(model_slug(&model.id).as_str())
    })
}

async fn generate(
    registry: &Registry,
    routed_model: &str,
    request: Value,
    stream: bool,
) -> Result<Response, ApiError> {
    let actual = request.get("request").unwrap_or(&request);
    let (provider, upstream_model) = registry
        .resolve(routed_model)
        .map_err(|error| ApiError::not_found(error.to_string()))?;
    let chat = gemini_to_chat(actual, &upstream_model, stream);
    let (base_url, headers) = provider
        .request_parts(registry.client())
        .await
        .map_err(|error| ApiError::upstream(StatusCode::BAD_GATEWAY, error.to_string()))?;
    let response = registry
        .client()
        .post(format!(
            "{}/chat/completions",
            base_url.trim_end_matches('/')
        ))
        .headers(headers)
        .json(&chat)
        .send()
        .await
        .map_err(|error| ApiError::upstream(StatusCode::BAD_GATEWAY, error.to_string()))?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(ApiError::upstream(
            StatusCode::BAD_GATEWAY,
            format!("upstream returned {status}: {text}"),
        ));
    }
    if stream {
        let events = async_stream::stream! {
            let mut tool_calls = std::collections::BTreeMap::<usize, (String, String, String)>::new();
            let reader = tokio_util::io::StreamReader::new(response.bytes_stream().map_err(std::io::Error::other));
            let mut lines = tokio::io::AsyncBufReadExt::lines(reader);
            while let Ok(Some(line)) = lines.next_line().await {
                let Some(data) = line.strip_prefix("data:") else { continue; };
                let data = data.trim();
                if data == "[DONE]" { break; }
                let Ok(chunk) = serde_json::from_str::<Value>(data) else { continue; };
                if let Some(text) = chunk.pointer("/choices/0/delta/content").and_then(Value::as_str) {
                    let event = cloud_event(vec![json!({"text":text})], "OTHER");
                    yield Ok::<Bytes, std::convert::Infallible>(Bytes::from(format!("data: {event}\n\n")));
                }
                for call in chunk.pointer("/choices/0/delta/tool_calls").and_then(Value::as_array).into_iter().flatten() {
                    let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                    let entry = tool_calls.entry(index).or_default();
                    if let Some(id) = call.get("id").and_then(Value::as_str) { entry.0.push_str(id); }
                    if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) { entry.1.push_str(name); }
                    if let Some(args) = call.pointer("/function/arguments").and_then(Value::as_str) { entry.2.push_str(args); }
                }
            }
            if !tool_calls.is_empty() {
                let parts = tool_calls.into_values().map(|(id, name, arguments)| {
                    let args = serde_json::from_str::<Value>(&arguments).unwrap_or_else(|_| json!({}));
                    json!({"functionCall":{"id":id,"name":name,"args":args}})
                }).collect::<Vec<_>>();
                let event = cloud_event(parts, "TOOL_CALL");
                yield Ok(Bytes::from(format!("data: {event}\n\n")));
            }
            let event = cloud_event(Vec::new(), "STOP");
            yield Ok(Bytes::from(format!("data: {event}\n\n")));
        };
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from_stream(events))
            .map_err(|error| {
                ApiError::upstream(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            })
    } else {
        let chat: Value = response
            .json()
            .await
            .map_err(|error| ApiError::upstream(StatusCode::BAD_GATEWAY, error.to_string()))?;
        let mapped = chat_to_gemini(&chat);
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"response":mapped,"traceId":"","metadata":{}}).to_string(),
            ))
            .map_err(|error| {
                ApiError::upstream(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            })
    }
}

fn gemini_to_chat(request: &Value, upstream_model: &str, stream: bool) -> Value {
    let mut messages = Vec::new();
    if let Some(parts) = request
        .pointer("/systemInstruction/parts")
        .and_then(Value::as_array)
    {
        let text = parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("");
        if !text.is_empty() {
            messages.push(json!({"role":"system","content":text}));
        }
    }
    for content in request
        .get("contents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let role = if content.get("role").and_then(Value::as_str) == Some("model") {
            "assistant"
        } else {
            "user"
        };
        let mut text = Vec::new();
        let mut calls = Vec::new();
        for part in content
            .get("parts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(value) = part.get("text").and_then(Value::as_str) {
                text.push(value.to_owned());
            }
            if let Some(data) = part.get("inlineData") {
                let mime = data
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .unwrap_or("application/octet-stream");
                let encoded = data.get("data").and_then(Value::as_str).unwrap_or_default();
                if mime.starts_with("image/") && !encoded.is_empty() {
                    text.push(format!("[Image: data:{mime};base64,{encoded}]"));
                }
            }
            if let Some(file) = part.get("fileData") {
                let mime = file
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .unwrap_or("application/octet-stream");
                let uri = file
                    .get("fileUri")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !uri.is_empty() {
                    text.push(format!("[File reference: {uri} ({mime})]"));
                }
            }
            if let Some(call) = part.get("functionCall") {
                calls.push(json!({
                    "id":call.get("id").cloned().unwrap_or_else(|| Value::String(crate::protocol::call_id())),
                    "type":"function",
                    "function":{
                        "name":call.get("name").cloned().unwrap_or_else(|| Value::String("tool".into())),
                        "arguments":serde_json::to_string(call.get("args").unwrap_or(&json!({}))).unwrap_or_else(|_| "{}".into())
                    }
                }));
            }
            if let Some(result) = part.get("functionResponse") {
                messages.push(json!({
                    "role":"tool",
                    "tool_call_id":result.get("id").cloned().unwrap_or(Value::Null),
                    "content":serde_json::to_string(result.get("response").unwrap_or(&Value::Null)).unwrap_or_default()
                }));
            }
        }
        if !calls.is_empty() {
            messages.push(json!({"role":"assistant","content":if text.is_empty(){Value::Null}else{Value::String(text.join("\n"))},"tool_calls":calls}));
        } else if !text.is_empty() {
            messages.push(json!({"role":role,"content":text.join("\n")}));
        }
    }
    let tools = request.get("tools").and_then(Value::as_array).into_iter().flatten().flat_map(|group| {
        group.get("functionDeclarations").and_then(Value::as_array).into_iter().flatten().map(|function| json!({
            "type":"function",
            "function":{
                "name":function.get("name"),
                "description":function.get("description"),
                "parameters":function.get("parameters").cloned().unwrap_or_else(|| json!({"type":"object","properties":{}}))
            }
        }))
    }).collect::<Vec<_>>();
    let mut output = json!({"model":upstream_model,"messages":messages,"stream":stream});
    if !tools.is_empty() {
        output["tools"] = Value::Array(tools);
    }
    if let Some(value) = request.pointer("/generationConfig/temperature") {
        output["temperature"] = value.clone();
    }
    if let Some(value) = request.pointer("/generationConfig/maxOutputTokens") {
        output["max_tokens"] = value.clone();
    }
    output
}

fn chat_to_gemini(chat: &Value) -> Value {
    let message = chat.pointer("/choices/0/message").unwrap_or(&Value::Null);
    let mut parts = Vec::new();
    if let Some(reasoning) = message
        .get("reasoning_content")
        .or_else(|| message.get("reasoning"))
        .and_then(Value::as_str)
        && !reasoning.is_empty()
    {
        parts.push(json!({"text":reasoning,"thought":true}));
    }
    if let Some(text) = message.get("content").and_then(Value::as_str)
        && !text.is_empty()
    {
        parts.push(json!({"text":text}));
    }
    for call in message
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let args = call
            .pointer("/function/arguments")
            .and_then(Value::as_str)
            .and_then(|value| serde_json::from_str::<Value>(value).ok())
            .unwrap_or_else(|| json!({}));
        parts.push(json!({"functionCall":{
            "id":call.get("id"),
            "name":call.pointer("/function/name"),
            "args":args
        }}));
    }
    json!({
        "candidates":[{"content":{"parts":parts,"role":"model"},"finishReason":if message.get("tool_calls").is_some(){"TOOL_CALL"}else{"STOP"},"index":0}],
        "usageMetadata":{
            "promptTokenCount":chat.pointer("/usage/prompt_tokens").cloned().unwrap_or_else(|| json!(0)),
            "candidatesTokenCount":chat.pointer("/usage/completion_tokens").cloned().unwrap_or_else(|| json!(0)),
            "totalTokenCount":chat.pointer("/usage/total_tokens").cloned().unwrap_or_else(|| json!(0))
        }
    })
}

fn cloud_event(parts: Vec<Value>, finish_reason: &str) -> String {
    json!({"response":{"candidates":[{"content":{"parts":parts,"role":"model"},"finishReason":finish_reason,"index":0}]},"traceId":"","metadata":{}}).to_string()
}

async fn forward_google(
    registry: &Registry,
    method: Method,
    uri: Uri,
    mut headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let target = if uri.path().contains("v1internal") || uri.path().contains("daily-cloudcode") {
        CLOUD_CODE_API
    } else {
        GOOGLE_API
    };
    for name in [
        header::HOST,
        header::CONTENT_LENGTH,
        header::CONNECTION,
        header::ACCEPT_ENCODING,
    ] {
        headers.remove(name);
    }
    let response = registry
        .client()
        .request(method, format!("{target}{uri}"))
        .headers(headers)
        .body(body)
        .send()
        .await
        .map_err(|error| ApiError::upstream(StatusCode::BAD_GATEWAY, error.to_string()))?;
    let status = response.status();
    let content_type = response.headers().get(header::CONTENT_TYPE).cloned();
    let stream = response.bytes_stream().map_err(std::io::Error::other);
    let mut builder = Response::builder().status(status);
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    builder
        .body(Body::from_stream(stream))
        .map_err(|error| ApiError::upstream(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}

fn model_slug(model: &str) -> String {
    format!(
        "joocode-{}",
        model
            .trim_start_matches("models/")
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            })
            .collect::<String>()
            .trim_matches('-')
    )
}

fn placeholder(model: &str) -> String {
    let hash = model.bytes().fold(5381_i64, |hash, byte| {
        ((hash << 5).wrapping_add(hash)).wrapping_add(byte as i64)
    });
    format!("MODEL_PLACEHOLDER_M{}", 400 + hash.unsigned_abs() % 200)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_patch_literal_preserves_archive_offsets() {
        let patched = fixed_js_literal("http://127.0.0.1:10100").unwrap();
        assert_eq!(patched.len(), OFFICIAL_GOOGLE_LITERAL.len());
        assert!(patched.starts_with("'http://127.0.0.1:10100'"));
    }

    #[test]
    fn model_slug_is_stable_and_readable() {
        assert_eq!(
            model_slug("crabcode/clika/gpt-5.4"),
            "joocode-crabcode-clika-gpt-5-4"
        );
        assert_eq!(
            placeholder("crabcode/clika/gpt-5.4"),
            placeholder("crabcode/clika/gpt-5.4")
        );
    }

    #[test]
    fn gemini_request_maps_tools_to_chat_completions() {
        let request = json!({
            "systemInstruction":{"parts":[{"text":"system"}]},
            "contents":[{"role":"user","parts":[{"text":"hello"}]}],
            "tools":[{"functionDeclarations":[{"name":"shell","description":"Run","parameters":{"type":"object"}}]}]
        });
        let chat = gemini_to_chat(&request, "upstream", false);
        assert_eq!(chat["model"], "upstream");
        assert_eq!(
            chat.pointer("/messages/0/role").and_then(Value::as_str),
            Some("system")
        );
        assert_eq!(
            chat.pointer("/tools/0/function/name")
                .and_then(Value::as_str),
            Some("shell")
        );
    }

    #[test]
    fn chat_tool_calls_map_back_to_gemini() {
        let output = chat_to_gemini(&json!({
            "choices":[{"message":{"content":null,"tool_calls":[{"id":"call_1","function":{"name":"shell","arguments":"{\"command\":\"pwd\"}"}}]}}]
        }));
        assert_eq!(
            output
                .pointer("/candidates/0/finishReason")
                .and_then(Value::as_str),
            Some("TOOL_CALL")
        );
        assert_eq!(
            output
                .pointer("/candidates/0/content/parts/0/functionCall/name")
                .and_then(Value::as_str),
            Some("shell")
        );
    }

    #[test]
    fn extracts_standard_gemini_model_from_uri() {
        let uri = "/v1beta/models/joocode-crabcode-clika-gpt-5-4:streamGenerateContent?alt=sse"
            .parse::<Uri>()
            .unwrap();
        assert_eq!(model_from_uri(&uri), Some("joocode-crabcode-clika-gpt-5-4"));
    }

    #[test]
    fn merges_custom_models_without_removing_native_models() {
        let mut response = json!({
            "models":{"gemini-3-8":{"displayName":"Gemini 3.8"}},
            "agentModelSorts":[{"groups":[{"modelIds":["gemini-3-8"]}]}]
        });
        let mut custom = serde_json::Map::new();
        custom.insert(
            "joocode-provider-model".into(),
            json!({"displayName":"provider/model"}),
        );
        merge_custom_models(&mut response, custom);

        assert!(response.pointer("/models/gemini-3-8").is_some());
        assert!(response.pointer("/models/joocode-provider-model").is_some());
        assert!(
            response
                .pointer("/agentModelSorts/0/groups/0/modelIds")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|value| value.as_str() == Some("joocode-provider-model"))
        );
    }
}
