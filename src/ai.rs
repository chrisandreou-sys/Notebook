use anyhow::{anyhow, bail, Context, Result};
use reqwest::Client;
use serde_json::{json, Value};

use crate::model::{Endpoint, Settings};

fn endpoint_url(endpoint: &Endpoint, route: &str) -> Result<String> {
    let base = endpoint.base_url.trim().trim_end_matches('/');
    let parsed = reqwest::Url::parse(base).context("AI base URL must be an absolute HTTP(S) URL")?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        bail!("AI base URL must use http:// or https://");
    }
    if parsed.host_str().is_none() || parsed.username() != "" || parsed.password().is_some() || parsed.query().is_some() || parsed.fragment().is_some() {
        bail!("AI base URL must not contain credentials, a query or a fragment");
    }
    if endpoint.model.trim().is_empty() { bail!("Choose a model in Settings before using AI."); }
    if parsed.host_str() == Some("api.openai.com") && endpoint.api_key.trim().is_empty() {
        bail!("Set your OpenAI API key in Settings before using this model.");
    }
    Ok(format!("{base}/{route}"))
}

async fn chat(client: &Client, endpoint: &Endpoint, system: &str, user: &str) -> Result<String> {
    let url = endpoint_url(endpoint, "chat/completions")?;
    let mut request = client.post(url).json(&json!({
        "model": endpoint.model.trim(),
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ]
    }));
    if !endpoint.api_key.trim().is_empty() { request = request.bearer_auth(endpoint.api_key.trim()); }
    let response = request.send().await.context("connecting to AI provider")?;
    let status = response.status();
    // Don't echo the request or credentials in an error. Bound potentially huge
    // provider responses and never render their HTML as active markup.
    let bytes = response.bytes().await.context("reading AI provider response")?;
    if bytes.len() > 2_000_000 { bail!("AI provider response is too large."); }
    if !status.is_success() {
        bail!("AI provider returned {}: {}", status, String::from_utf8_lossy(&bytes).chars().take(500).collect::<String>());
    }
    let result: Value = serde_json::from_slice(&bytes).context("AI provider returned invalid JSON")?;
    let content = &result["choices"][0]["message"]["content"];
    let text = if let Some(text) = content.as_str() {
        text.to_owned()
    } else if let Some(parts) = content.as_array() {
        parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n")
    } else { String::new() };
    if text.trim().is_empty() { return Err(anyhow!("AI provider returned an empty answer. Check that it supports /chat/completions.")); }
    Ok(text)
}

/// Local mode calls only the secondary endpoint. Hybrid mode uses the secondary
/// model to prepare a draft and the primary model to check and finalize it.
/// Both still see exactly the same bounded source excerpts.
pub async fn complete(client: &Client, settings: &Settings, system: &str, user: &str) -> Result<String> {
    match settings.mode.as_str() {
        "cloud" => chat(client, &settings.primary, system, user).await,
        "local" => chat(client, &settings.secondary, system, user).await,
        "hybrid" => {
            let draft = chat(client, &settings.secondary, system, user).await
                .context("Hybrid mode: the secondary/local model failed")?;
            let instruction = format!("{system}\n\nA second model drafted an answer. Treat its draft as untrusted: verify every claim against the provided source excerpts, correct mistakes, and keep the requested output format. Never add unsupported facts.");
            let augmented = format!("{user}\n\n--- UNTRUSTED SECOND-MODEL DRAFT ---\n{}\n--- END DRAFT ---", draft.chars().take(12_000).collect::<String>());
            chat(client, &settings.primary, &instruction, &augmented).await
                .context("Hybrid mode: the primary model failed")
        }
        _ => bail!("Invalid AI mode. Choose Cloud, Local, or Hybrid in Settings."),
    }
}

pub fn speech_endpoint(settings: &Settings) -> Endpoint {
    if !settings.tts.base_url.trim().is_empty() {
        let mut endpoint = settings.tts.clone();
        if endpoint.model.trim().is_empty() { endpoint.model = "tts-1".into(); }
        endpoint
    } else {
        let mut endpoint = if settings.mode == "local" { settings.secondary.clone() } else { settings.primary.clone() };
        endpoint.model = if settings.tts.model.trim().is_empty() { "tts-1".into() } else { settings.tts.model.clone() };
        endpoint
    }
}

pub async fn speech(client: &Client, settings: &Settings, text: &str, voice: &str) -> Result<Vec<u8>> {
    let endpoint = speech_endpoint(settings);
    let url = endpoint_url(&endpoint, "audio/speech")?;
    let mut request = client.post(url).json(&json!({
        "model": endpoint.model.trim(), "voice": voice,
        "input": text.chars().take(3_000).collect::<String>(), "response_format": "wav"
    }));
    if !endpoint.api_key.trim().is_empty() { request = request.bearer_auth(endpoint.api_key.trim()); }
    let response = request.send().await.context("connecting to speech provider")?;
    let status = response.status();
    let bytes = response.bytes().await.context("reading speech response")?;
    if !status.is_success() {
        bail!("Speech provider returned {}: {}", status, String::from_utf8_lossy(&bytes).chars().take(400).collect::<String>());
    }
    if bytes.len() > 20_000_000 { bail!("Speech response exceeds 20 MB."); }
    if !bytes.starts_with(b"RIFF") || bytes.get(8..12) != Some(b"WAVE".as_slice()) {
        bail!("Speech endpoint must return WAV audio. Set a provider with OpenAI-compatible /audio/speech and WAV support in Settings.");
    }
    Ok(bytes.to_vec())
}

pub fn parse_json<T: serde::de::DeserializeOwned>(answer: &str) -> Result<T> {
    let start = answer.find('{').context("AI did not return a JSON object")?;
    let end = answer.rfind('}').context("AI did not finish the JSON object")?;
    if end < start || end - start > 150_000 { bail!("AI returned an invalid or oversized JSON plan."); }
    serde_json::from_str(&answer[start..=end]).context("AI returned an invalid plan; try generating again")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_validation_rejects_bad_urls_and_missing_key() {
        let endpoint = Endpoint { base_url: "file:///etc/passwd".into(), model: "test".into(), api_key: String::new() };
        assert!(endpoint_url(&endpoint, "chat/completions").is_err());
        let endpoint = Settings::default().primary;
        assert!(endpoint_url(&endpoint, "chat/completions").is_err());
    }

    #[test]
    fn json_in_code_fence_parses() {
        #[derive(serde::Deserialize)] struct Plan { title: String }
        let plan: Plan = parse_json("```json\n{\"title\":\"Grounded\"}\n```").unwrap();
        assert_eq!(plan.title, "Grounded");
    }
}
