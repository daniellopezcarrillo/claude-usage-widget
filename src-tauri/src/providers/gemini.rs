use std::path::PathBuf;

use serde::Deserialize;

use crate::errors::{AppError, AppResult};
use crate::providers::antigravity_cred;
use crate::types::{Provider, Status, UsageResponse, UsageWindow};

// === Legacy gemini-cli token (oauth_creds.json) ===

#[derive(Deserialize)]
struct Creds {
    access_token: String,
    #[serde(default)]
    expiry_date: Option<i64>,
}

#[derive(Deserialize)]
struct ProjectsFile {
    #[serde(default)]
    projects: std::collections::HashMap<String, String>,
}

fn creds_path() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".gemini").join("oauth_creds.json")
}

fn projects_path() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".gemini").join("projects.json")
}

fn read_legacy_token() -> AppResult<String> {
    let s = std::fs::read_to_string(creds_path())
        .map_err(|_| AppError::NotAuthenticated("gemini creds not found".into()))?;
    let c: Creds = serde_json::from_str(&s)
        .map_err(|_| AppError::NotAuthenticated("gemini creds malformed".into()))?;
    Ok(c.access_token)
}

fn read_first_project_id() -> Option<String> {
    let s = std::fs::read_to_string(projects_path()).ok()?;
    let pf: ProjectsFile = serde_json::from_str(&s).ok()?;
    pf.projects.into_values().next()
}

// === Antigravity token (wincred: gemini:antigravity) ===

#[derive(Deserialize)]
struct AgyBlob {
    token: AgyToken,
}

#[derive(Deserialize)]
struct AgyToken {
    access_token: String,
}

fn read_agy_token() -> Option<String> {
    let blob = antigravity_cred::read_token_blob("gemini:antigravity")?;
    let s = String::from_utf8(blob).ok()?;
    let parsed: AgyBlob = serde_json::from_str(s.trim_end_matches('\0')).ok()?;
    Some(parsed.token.access_token)
}

// === Legacy retrieveUserQuota response (REQUESTS buckets) ===

#[derive(Deserialize)]
pub(crate) struct QuotaBucket {
    #[serde(rename = "resetTime")]
    pub resets_at: String,
    #[serde(rename = "tokenType")]
    pub token_type: String,
    #[serde(rename = "modelId")]
    pub model_id: String,
    #[serde(rename = "remainingFraction")]
    pub remaining_fraction: f64,
}

#[derive(Deserialize, Default)]
pub(crate) struct QuotaResponse {
    #[serde(default)]
    pub buckets: Vec<QuotaBucket>,
}

// === Antigravity loadCodeAssist + fetchAvailableModels schemas ===

#[derive(Deserialize)]
struct LoadCodeAssistResp {
    #[serde(default, rename = "cloudaicompanionProject")]
    project: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct AvailableModelsResp {
    #[serde(default)]
    models: std::collections::HashMap<String, ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    #[serde(default, rename = "displayName")]
    display_name: Option<String>,
    #[serde(default, rename = "quotaInfo")]
    quota_info: Option<QuotaInfo>,
}

#[derive(Deserialize)]
struct QuotaInfo {
    #[serde(default, rename = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(default, rename = "resetTime")]
    reset_time: Option<String>,
}

// === Family classification (shared) ===
// 4 family display order: Flash, Pro, Claude, GPT-OSS.
// flash-lite collapses into "flash"; "other" is unmapped (e.g., agent helpers).

fn model_tier(model_id: &str) -> &'static str {
    if model_id.starts_with("claude-") { "claude" }
    else if model_id.starts_with("gpt-oss") || model_id.starts_with("gpt-") { "gpt" }
    else if model_id.contains("flash") { "flash" }
    else if model_id.contains("pro") { "pro" }
    else { "other" }
}

fn tier_label(key: &str) -> &'static str {
    match key {
        "flash" => "Flash",
        "pro" => "Pro",
        "claude" => "Claude",
        "gpt" => "GPT-OSS",
        _ => "Other",
    }
}

const TIER_ORDER: &[&str] = &["flash", "pro", "claude", "gpt"];
const DAY_SEC: u64 = 24 * 60 * 60;

fn compute_time_progress(resets_at: &str, duration_sec: u64) -> f64 {
    let reset = match chrono::DateTime::parse_from_rfc3339(resets_at) {
        Ok(dt) => dt.timestamp(),
        Err(_) => return 0.0,
    };
    let now = chrono::Utc::now().timestamp();
    let start = reset - duration_sec as i64;
    if now <= start { return 0.0; }
    if now >= reset { return 100.0; }
    ((now - start) as f64 / duration_sec as f64 * 100.0).round()
}

// === Legacy mapping (retrieveUserQuota → tiered windows) ===

pub(crate) fn map_raw_to_response(raw: &QuotaResponse) -> UsageResponse {
    let mut first_per_tier: std::collections::HashMap<&str, &QuotaBucket> = Default::default();
    for b in &raw.buckets {
        if b.token_type != "REQUESTS" { continue; }
        let tier = model_tier(&b.model_id);
        first_per_tier.entry(tier).or_insert(b);
    }

    let mut windows = Vec::new();
    for key in TIER_ORDER {
        if let Some(b) = first_per_tier.get(key) {
            let util = ((1.0 - b.remaining_fraction) * 100.0).round();
            windows.push(UsageWindow {
                key: (*key).to_string(),
                name: tier_label(key).to_string(),
                utilization: util,
                resets_at: b.resets_at.clone(),
                time_progress: compute_time_progress(&b.resets_at, DAY_SEC),
            });
        }
    }

    UsageResponse {
        provider: Provider::Gemini,
        status: Status::Ok,
        windows,
        extra_usage: None,
        error: None,
    }
}

// === Antigravity mapping (fetchAvailableModels → family windows) ===
// 4 families: Flash, Pro, Claude, GPT-OSS. For each family, pick the model
// variant with the lowest remainingFraction (most consumed — conservative view).
// Internal models (no displayName) and unmapped families are skipped.

fn map_antigravity_to_response(raw: &AvailableModelsResp) -> UsageResponse {
    use std::collections::HashMap;
    let mut worst_per_tier: HashMap<&str, (&ModelEntry, &str)> = HashMap::new();

    for (model_id, entry) in &raw.models {
        if entry.display_name.is_none() { continue; }
        let qi = match &entry.quota_info {
            Some(q) if q.remaining_fraction.is_some() && q.reset_time.is_some() => q,
            _ => continue,
        };
        let frac = qi.remaining_fraction.unwrap();
        let tier = model_tier(model_id);
        if tier == "other" { continue; }
        worst_per_tier
            .entry(tier)
            .and_modify(|(existing, _)| {
                let existing_frac = existing.quota_info.as_ref().and_then(|q| q.remaining_fraction).unwrap_or(1.0);
                if frac < existing_frac {
                    *existing = entry;
                }
            })
            .or_insert((entry, tier));
    }

    let mut windows = Vec::new();
    for key in TIER_ORDER {
        if let Some((entry, _)) = worst_per_tier.get(key) {
            let qi = entry.quota_info.as_ref().unwrap();
            let frac = qi.remaining_fraction.unwrap_or(1.0);
            let reset = qi.reset_time.clone().unwrap_or_default();
            // resetTime cycle is variable per-model; estimate duration from the gap between now and reset,
            // bounded between 1h (sprint) and DAY_SEC. Falls back to DAY_SEC if parse fails.
            let duration = match chrono::DateTime::parse_from_rfc3339(&reset) {
                Ok(dt) => {
                    let gap = dt.timestamp() - chrono::Utc::now().timestamp();
                    if gap > 0 { (gap as u64).max(3600).min(DAY_SEC) } else { DAY_SEC }
                }
                Err(_) => DAY_SEC,
            };
            windows.push(UsageWindow {
                key: (*key).to_string(),
                name: tier_label(key).to_string(),
                utilization: ((1.0 - frac) * 100.0).round(),
                resets_at: reset,
                time_progress: compute_time_progress(
                    &entry.quota_info.as_ref().and_then(|q| q.reset_time.clone()).unwrap_or_default(),
                    duration,
                ),
            });
        }
    }

    UsageResponse {
        provider: Provider::Gemini,
        status: Status::Ok,
        windows,
        extra_usage: None,
        error: None,
    }
}

// === HTTP helpers ===

async fn fetch_antigravity(token: &str) -> AppResult<UsageResponse> {
    let client = reqwest::Client::new();

    // 1. loadCodeAssist → cloudaicompanionProject
    let lca_body = serde_json::json!({
        "metadata": { "ideType": "ANTIGRAVITY", "platform": "PLATFORM_UNSPECIFIED", "pluginType": "GEMINI" }
    });
    let lca_res = client
        .post("https://daily-cloudcode-pa.googleapis.com/v1internal:loadCodeAssist")
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .header("User-Agent", "antigravity")
        .body(lca_body.to_string())
        .send()
        .await?;
    let lca_status = lca_res.status();
    if lca_status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(AppError::Expired);
    }
    if !lca_status.is_success() {
        let body = lca_res.text().await.unwrap_or_default();
        return Err(AppError::Api { status: lca_status.as_u16(), message: body });
    }
    let lca: LoadCodeAssistResp = lca_res.json().await?;
    let project_id = match lca.project {
        Some(serde_json::Value::String(s)) => Some(s),
        Some(serde_json::Value::Object(o)) => o.get("id").and_then(|v| v.as_str()).map(String::from),
        _ => None,
    };

    // 2. fetchAvailableModels
    let body = match &project_id {
        Some(p) => serde_json::json!({ "project": p }),
        None => serde_json::json!({}),
    };
    let res = client
        .post("https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels")
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .header("User-Agent", "antigravity")
        .body(body.to_string())
        .send()
        .await?;
    let status = res.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(AppError::Expired);
    }
    if !status.is_success() {
        let body = res.text().await.unwrap_or_default();
        return Err(AppError::Api { status: status.as_u16(), message: body });
    }
    let raw: AvailableModelsResp = res.json().await?;
    Ok(map_antigravity_to_response(&raw))
}

async fn fetch_legacy(token: &str) -> AppResult<UsageResponse> {
    let project_id = read_first_project_id();
    let body = match project_id {
        Some(p) => serde_json::json!({ "project": p }),
        None => serde_json::json!({}),
    };
    let client = reqwest::Client::new();
    let res = client
        .post("https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota")
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await?;

    let status = res.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(AppError::Expired);
    }
    if !status.is_success() {
        let body = res.text().await.unwrap_or_default();
        return Err(AppError::Api { status: status.as_u16(), message: body });
    }

    let raw: QuotaResponse = res.json().await?;
    Ok(map_raw_to_response(&raw))
}

pub async fn fetch() -> AppResult<UsageResponse> {
    // Prefer agy (Antigravity CLI) token if present — covers post-2026-06-18 era
    // and gives richer model coverage (Gemini 3.x, preview variants).
    if let Some(token) = read_agy_token() {
        return fetch_antigravity(&token).await;
    }
    let token = read_legacy_token()?;
    fetch_legacy(&token).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_dedupe_tier_and_compute_utilization() {
        let raw = QuotaResponse {
            buckets: vec![
                QuotaBucket {
                    resets_at: "2030-01-01T00:00:00Z".into(),
                    token_type: "REQUESTS".into(),
                    model_id: "gemini-2.0-flash-exp".into(),
                    remaining_fraction: 0.2,
                },
                QuotaBucket {
                    resets_at: "2030-01-01T00:00:00Z".into(),
                    token_type: "REQUESTS".into(),
                    model_id: "gemini-2.0-flash-002".into(),
                    remaining_fraction: 0.5,
                },
                QuotaBucket {
                    resets_at: "2030-01-01T00:00:00Z".into(),
                    token_type: "REQUESTS".into(),
                    model_id: "gemini-2.5-pro".into(),
                    remaining_fraction: 0.9,
                },
            ],
        };
        let resp = map_raw_to_response(&raw);
        // 4-family taxonomy: legacy response only fills flash + pro
        assert_eq!(resp.windows.len(), 2);
        assert_eq!(resp.windows[0].key, "flash");
        assert_eq!(resp.windows[0].utilization, 80.0);
        assert_eq!(resp.windows[1].key, "pro");
        assert_eq!(resp.windows[1].utilization, 10.0);
    }

    #[test]
    fn legacy_skips_non_requests_tokens() {
        let raw = QuotaResponse {
            buckets: vec![QuotaBucket {
                resets_at: "2030-01-01T00:00:00Z".into(),
                token_type: "INPUT_TOKENS".into(),
                model_id: "gemini-2.5-pro".into(),
                remaining_fraction: 0.5,
            }],
        };
        let resp = map_raw_to_response(&raw);
        assert_eq!(resp.windows.len(), 0);
    }

    fn mk_entry(display: &str, frac: f64, reset: &str) -> ModelEntry {
        ModelEntry {
            display_name: Some(display.into()),
            quota_info: Some(QuotaInfo {
                remaining_fraction: Some(frac),
                reset_time: Some(reset.into()),
            }),
        }
    }

    #[test]
    fn antigravity_picks_worst_per_family() {
        let mut models = std::collections::HashMap::new();
        models.insert("gemini-3-flash".into(), mk_entry("Gemini 3 Flash", 0.9, "2030-01-01T00:00:00Z"));
        models.insert("gemini-3.5-flash-low".into(), mk_entry("Gemini 3.5 Flash (Medium)", 0.4, "2030-01-01T00:00:00Z"));
        models.insert("gemini-3.1-flash-lite".into(), mk_entry("Gemini 3.1 Flash Lite", 0.7, "2030-01-01T00:00:00Z"));
        models.insert("gemini-3.1-pro-high".into(), mk_entry("Gemini 3.1 Pro (High)", 0.6, "2030-01-01T00:00:00Z"));
        models.insert("gemini-3.1-pro-low".into(), mk_entry("Gemini 3.1 Pro (Low)", 0.3, "2030-01-01T00:00:00Z"));
        models.insert("claude-sonnet-4-6".into(), mk_entry("Claude Sonnet 4.6 (Thinking)", 0.2, "2030-01-01T00:00:00Z"));
        models.insert("claude-opus-4-6-thinking".into(), mk_entry("Claude Opus 4.6 (Thinking)", 0.5, "2030-01-01T00:00:00Z"));
        models.insert("gpt-oss-120b-medium".into(), mk_entry("GPT-OSS 120B (Medium)", 0.8, "2030-01-01T00:00:00Z"));
        models.insert("chat_23310".into(), ModelEntry { display_name: None, quota_info: None });
        let raw = AvailableModelsResp { models };
        let resp = map_antigravity_to_response(&raw);

        // 4 families: Flash, Pro, Claude, GPT-OSS
        assert_eq!(resp.windows.len(), 4);
        let by_key: std::collections::HashMap<_, _> = resp.windows.iter().map(|w| (w.key.as_str(), w)).collect();
        // flash: worst among gemini-3-flash (0.9), gemini-3.5-flash-low (0.4), gemini-3.1-flash-lite (0.7)
        assert_eq!(by_key["flash"].utilization, 60.0);
        // pro: worst between gemini-3.1-pro-high (0.6), gemini-3.1-pro-low (0.3)
        assert_eq!(by_key["pro"].utilization, 70.0);
        // claude: worst between sonnet (0.2), opus (0.5)
        assert_eq!(by_key["claude"].utilization, 80.0);
        // gpt: only gpt-oss-120b-medium (0.8)
        assert_eq!(by_key["gpt"].utilization, 20.0);

        // Order respects TIER_ORDER
        assert_eq!(resp.windows[0].key, "flash");
        assert_eq!(resp.windows[1].key, "pro");
        assert_eq!(resp.windows[2].key, "claude");
        assert_eq!(resp.windows[3].key, "gpt");
    }
}
