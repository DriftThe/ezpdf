//! Update check: fetch the latest GitHub Release and compare it with the running version.
//! Notify only (toast + settings text); no download or install.
//! Network/404 failure → Err; the caller decides whether to surface it (silent at launch, toast on manual check).

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use ts_rs::TS;

/// Upstream repo checked for releases.
const UPDATE_REPO: &str = "DriftThe/ezpdf";

/// Update-check result (rendered by the frontend).
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    /// Running app version (from tauri.conf).
    pub current: String,
    /// Latest release tag without the `v` prefix; a missing release is an Err, not None.
    pub latest: Option<String>,
    /// Whether a newer version is available (numeric comparison).
    pub newer: bool,
}

/// Numeric-segment compare: `v1.2.3` and `1.2.3` both parse, missing segments count as 0,
/// non-numeric suffixes (-beta) are ignored. a > b → true.
fn version_gt(a: &str, b: &str) -> bool {
    let parts = |s: &str| -> Vec<u64> {
        s.trim()
            .trim_start_matches('v')
            .split('.')
            .map(|p| {
                p.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0)
            })
            .collect()
    };
    let (x, y) = (parts(a), parts(b));
    for i in 0..x.len().max(y.len()) {
        let (l, r) = (
            x.get(i).copied().unwrap_or(0),
            y.get(i).copied().unwrap_or(0),
        );
        if l != r {
            return l > r;
        }
    }
    false
}

/// Fetch the latest release and compare; network/HTTP/parse failure → Err (silent at launch).
pub async fn check_update(current: &str) -> Result<UpdateInfo, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("failed to create update-check client: {e}"))?;
    let url = format!("https://api.github.com/repos/{UPDATE_REPO}/releases/latest");
    let resp = client
        .get(&url)
        .header("User-Agent", "ezpdf-update-check")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("update check failed: {e}"))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("update check failed: {e}"))?;
    if !status.is_success() {
        return Err(format!("update check failed: HTTP {status}"));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("release info is not JSON: {e}"))?;
    let latest = v["tag_name"].as_str().map(|s| s.trim_start_matches('v').to_string());
    let newer = latest.as_deref().map(|l| version_gt(l, current)).unwrap_or(false);
    Ok(UpdateInfo {
        current: current.to_string(),
        latest,
        newer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare_handles_prefix_and_segments() {
        assert!(version_gt("1.2.4", "1.2.3"));
        assert!(version_gt("v1.3", "1.2.9"));
        assert!(version_gt("2.0.0", "1.99.99"));
        assert!(!version_gt("1.2.3", "1.2.3"));
        assert!(!version_gt("v1.2", "1.2.0"));
        assert!(!version_gt("0.1.0", "0.1.1"));
        assert!(version_gt("1.2.3-beta", "1.2.2")); // non-numeric suffix ignored
    }
}
