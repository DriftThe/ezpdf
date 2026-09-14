//! 更新检查（用户 2026-09-14）：查询 GitHub Releases 最新发布，与当前版本比较。
//! 只做提醒（toast + 常规设置页文字），不下载不安装——仓库地址固定为项目主页。
//! 网络失败/仓库未发布（404）→ Err，由调用方决定是否打扰（启动检查静默、手动检查 toast）。

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use ts_rs::TS;

/// 发布仓库（用户提供：https://github.com/DriftThe/ezpdf）
const UPDATE_REPO: &str = "DriftThe/ezpdf";

/// 更新检查结果（前端 settings store 展示）
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    /// 当前应用版本（tauri.conf 的 version）
    pub current: String,
    /// 最新发布版本（tag 去掉 v 前缀）；无发布时应为 Err 而非 None
    pub latest: Option<String>,
    /// 发布页地址（Releases 页面）
    pub url: Option<String>,
    /// 是否有更新（语义化数字比较）
    pub newer: bool,
    /// 发布说明（截断）
    pub notes: Option<String>,
}

/// 版本比较：`v1.2.3` vs `1.2.3` 均可；按数字段比较（缺失段视作 0），
/// 非数字后缀（-beta 等）忽略。a > b → true。
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

/// 查询最新 release 并比较；网络/HTTP/解析失败 → Err（启动检查静默处理）
pub async fn check_update(current: &str) -> Result<UpdateInfo, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("更新检查客户端创建失败: {e}"))?;
    let url = format!("https://api.github.com/repos/{UPDATE_REPO}/releases/latest");
    let resp = client
        .get(&url)
        .header("User-Agent", "ezpdf-update-check")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("检查更新失败: {e}"))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("检查更新失败: {e}"))?;
    if !status.is_success() {
        return Err(format!("检查更新失败: HTTP {status}"));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("发布信息非 JSON: {e}"))?;
    let latest = v["tag_name"].as_str().map(|s| s.trim_start_matches('v').to_string());
    let html = v["html_url"].as_str().map(String::from);
    let notes = v["body"].as_str().map(|b| {
        let mut s: String = b.chars().take(600).collect();
        if s.len() < b.len() {
            s.push('…');
        }
        s
    });
    let newer = latest.as_deref().map(|l| version_gt(l, current)).unwrap_or(false);
    Ok(UpdateInfo {
        current: current.to_string(),
        latest,
        url: html,
        newer,
        notes,
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
        assert!(version_gt("1.2.3-beta", "1.2.2")); // 非数字后缀忽略
    }
}
