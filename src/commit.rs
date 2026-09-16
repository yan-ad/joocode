use std::{
    io::Write,
    process::{Command, Stdio},
    time::Duration,
};

use anyhow::{Context, bail};
use serde_json::{Value, json};

use crate::provider::{Registry, WireApi};

const DEFAULT_COMMIT_RULES: &str = "\
Write commit messages that follow the Conventional Commits specification
(https://www.conventionalcommits.org/en/v1.0.0/).

Structure:
<type>[optional scope][!]: <description>

- Use `feat` for a new feature and `fix` for a bug fix.
- Other suitable types include `build`, `chore`, `ci`, `docs`, `perf`, `refactor`, `revert`, `style`, and `test`.
- Keep the description concise and imperative, without a trailing period.
- Add a body only when it provides useful context not present in the subject.
- Return only the commit message, with no markdown fence or commentary.";

pub async fn run(
    registry: &Registry,
    model: Option<&str>,
    dry_run: bool,
    co_authors: &[String],
) -> anyhow::Result<()> {
    let diff = staged_diff()?;
    if diff.trim().is_empty() {
        bail!("nothing staged to commit; stage changes with `git add` first");
    }
    let ticket = current_branch().as_deref().and_then(ticket_from_branch);
    let model = select_model(registry, model)?;
    let rules = commit_rules();
    let mut message = generate_message(registry, &model, &rules, &diff).await?;
    message = cleanup_message(&message);
    if message.is_empty() {
        bail!("model returned an empty commit message");
    }
    if let Some(ticket) = ticket.as_deref() {
        message = append_ticket(&message, ticket);
    }
    if !co_authors.is_empty() {
        message = append_co_authors(&message, co_authors);
    }
    println!("{message}");
    if dry_run {
        return Ok(());
    }
    run_commit(&message)
}

fn staged_diff() -> anyhow::Result<String> {
    let output = Command::new("git")
        .args(["diff", "--cached", "--no-color"])
        .output()
        .context("failed to run `git diff --cached`")?;
    if !output.status.success() {
        bail!(
            "`git diff --cached` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn current_branch() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!branch.is_empty() && branch != "HEAD").then_some(branch)
}

/// Extract the trailing work-item reference (for example `AB#3340` or `JIRA-123`)
/// from a branch name such as `sa/AB#3340`.
fn ticket_from_branch(branch: &str) -> Option<String> {
    let chars: Vec<char> = branch.chars().collect();
    let mut best: Option<String> = None;
    let mut index = 0;
    while index < chars.len() {
        if !chars[index].is_ascii_alphabetic() {
            index += 1;
            continue;
        }
        let mut end = index;
        while end < chars.len() && chars[end].is_ascii_alphanumeric() {
            end += 1;
        }
        if end < chars.len() && (chars[end] == '#' || chars[end] == '-') {
            let mut digits = end + 1;
            while digits < chars.len() && chars[digits].is_ascii_digit() {
                digits += 1;
            }
            if digits > end + 1 {
                best = Some(chars[index..digits].iter().collect());
                index = digits;
                continue;
            }
        }
        index = end.max(index + 1);
    }
    best
}

fn commit_rules() -> String {
    crate::target_config::TargetPreferences::load()
        .ok()
        .and_then(|preferences| preferences.commit_rules)
        .map(|rules| rules.trim().to_owned())
        .filter(|rules| !rules.is_empty())
        .unwrap_or_else(|| DEFAULT_COMMIT_RULES.to_owned())
}

fn select_model(registry: &Registry, requested: Option<&str>) -> anyhow::Result<String> {
    if let Some(model) = requested {
        registry
            .resolve(model)
            .with_context(|| format!("unknown model '{model}'"))?;
        return Ok(model.to_owned());
    }
    if let Some(model) = crate::local_config::default_model_route()?
        && registry.resolve(&model).is_ok()
    {
        return Ok(model);
    }
    registry
        .models()
        .first()
        .map(|model| model.id.clone())
        .context("no models available; run `jcx doctor` for source diagnostics")
}

async fn generate_message(
    registry: &Registry,
    model: &str,
    rules: &str,
    diff: &str,
) -> anyhow::Result<String> {
    let (provider, upstream_model) = registry.resolve(model)?;
    let (base_url, headers) = provider.request_parts(registry.client()).await?;
    let user_prompt = format!("Generate a commit message for the staged changes below.\n\n{diff}");

    let (url, body) = match provider.wire_api {
        WireApi::OpenAiChat => (
            format!("{}/chat/completions", base_url.trim_end_matches('/')),
            json!({
                "model": upstream_model,
                "messages": [
                    {"role": "system", "content": rules},
                    {"role": "user", "content": user_prompt},
                ],
                "temperature": 0.2,
                "stream": false,
            }),
        ),
        WireApi::OpenAiResponses => (
            format!("{}/responses", base_url.trim_end_matches('/')),
            json!({
                "model": upstream_model,
                "instructions": rules,
                "input": user_prompt,
                "stream": false,
            }),
        ),
        WireApi::AnthropicMessages => (
            format!("{}/messages", base_url.trim_end_matches('/')),
            json!({
                "model": upstream_model,
                "max_tokens": 1024,
                "system": rules,
                "messages": [{"role": "user", "content": user_prompt}],
            }),
        ),
        WireApi::Gemini => (
            format!(
                "{}/models/{}:generateContent",
                base_url.trim_end_matches('/'),
                upstream_model
            ),
            json!({
                "systemInstruction": {"parts": [{"text": rules}]},
                "contents": [{"role": "user", "parts": [{"text": user_prompt}]}],
            }),
        ),
    };

    let response = registry
        .client()
        .post(url)
        .headers(headers)
        .json(&body)
        .timeout(Duration::from_secs(120))
        .send()
        .await
        .context("failed to request the model")?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        bail!("model request failed ({status}): {text}");
    }

    let body: Value = response.json().await.context("invalid model response")?;
    match provider.wire_api {
        WireApi::OpenAiChat => body
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .map(str::to_owned),
        WireApi::OpenAiResponses => body
            .get("output_text")
            .and_then(Value::as_str)
            .or_else(|| {
                body.pointer("/output/0/content/0/text")
                    .and_then(Value::as_str)
            })
            .map(str::to_owned),
        WireApi::AnthropicMessages => body
            .pointer("/content/0/text")
            .and_then(Value::as_str)
            .map(str::to_owned),
        WireApi::Gemini => body
            .pointer("/candidates/0/content/parts/0/text")
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
    .context("model returned no text content")
}

fn append_ticket(message: &str, ticket: &str) -> String {
    let message = message.trim();
    if message.is_empty() || message.contains(ticket) {
        return message.to_owned();
    }
    let mut lines = message.lines();
    let subject = lines.next().unwrap_or_default();
    let rest: Vec<&str> = lines.collect();
    let mut output = format!("{subject} {ticket}");
    if !rest.is_empty() {
        output.push('\n');
        output.push_str(&rest.join("\n"));
    }
    output
}

/// Append `Co-authored-by` trailers for each co-author token. A token is
/// either a bare email or `Name <email>`; bare emails derive a display name
/// from their local part.
fn append_co_authors(message: &str, co_authors: &[String]) -> String {
    let trailers = co_authors
        .iter()
        .map(|token| {
            let (name, email) = parse_co_author(token);
            format!("Co-authored-by: {name} <{email}>")
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{}\n\n{trailers}", message.trim_end())
}

fn parse_co_author(token: &str) -> (String, String) {
    if let Some(open) = token.find('<')
        && let Some(close) = token.rfind('>')
    {
        let name = token[..open].trim();
        let email = token[open + 1..close].trim();
        if !name.is_empty() && !email.is_empty() {
            return (name.to_owned(), email.to_owned());
        }
    }
    let email = token.trim().to_owned();
    let name = name_from_email(&email);
    (name, email)
}

fn name_from_email(email: &str) -> String {
    let local = email.split('@').next().unwrap_or(email);
    local
        .split(['.', '_', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn cleanup_message(message: &str) -> String {
    let mut lines: Vec<&str> = message.lines().collect();
    if lines
        .first()
        .is_some_and(|line| line.trim().starts_with("```"))
    {
        lines.remove(0);
    }
    if lines.last().is_some_and(|line| line.trim() == "```") {
        lines.pop();
    }
    lines.join("\n").trim().to_owned()
}

fn run_commit(message: &str) -> anyhow::Result<()> {
    let mut child = Command::new("git")
        .args(["commit", "-F", "-"])
        .stdin(Stdio::piped())
        .spawn()
        .context("failed to run `git commit`")?;
    child
        .stdin
        .take()
        .context("failed to open git commit stdin")?
        .write_all(message.as_bytes())
        .context("failed to write the commit message")?;
    let status = child.wait().context("git commit did not finish")?;
    if !status.success() {
        bail!("git commit exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_work_item_from_branch() {
        assert_eq!(ticket_from_branch("sa/AB#3340"), Some("AB#3340".into()));
        assert_eq!(ticket_from_branch("main"), None);
        assert_eq!(
            ticket_from_branch("feat/JIRA-1234"),
            Some("JIRA-1234".into())
        );
        assert_eq!(ticket_from_branch("release/v1.2.3"), None);
        assert_eq!(
            ticket_from_branch("fix/PROJ-42-extra"),
            Some("PROJ-42".into())
        );
    }

    #[test]
    fn appends_ticket_as_subject_suffix() {
        assert_eq!(
            append_ticket("feat: add search", "AB#3340"),
            "feat: add search AB#3340"
        );
        assert_eq!(
            append_ticket("feat: add search\n\nAdds a search box.", "AB#3340"),
            "feat: add search AB#3340\n\nAdds a search box."
        );
        assert_eq!(
            append_ticket("feat: add search AB#3340", "AB#3340"),
            "feat: add search AB#3340"
        );
    }

    #[test]
    fn strips_markdown_fence_from_message() {
        assert_eq!(
            cleanup_message("```\nfeat: add search\n```"),
            "feat: add search"
        );
    }

    #[test]
    fn appends_co_author_trailers() {
        assert_eq!(
            append_co_authors(
                "feat: add search",
                &[
                    "yanuar@kiriminaja.com".into(),
                    "claude@anthropic.com".into()
                ]
            ),
            "feat: add search\n\nCo-authored-by: Yanuar <yanuar@kiriminaja.com>\nCo-authored-by: Claude <claude@anthropic.com>"
        );
    }

    #[test]
    fn parses_name_and_email_co_author() {
        assert_eq!(
            parse_co_author("Yanuar Aditia <yanuar@kiriminaja.com>"),
            ("Yanuar Aditia".into(), "yanuar@kiriminaja.com".into())
        );
        assert_eq!(
            parse_co_author("john.doe@example.com"),
            ("John Doe".into(), "john.doe@example.com".into())
        );
    }
}
