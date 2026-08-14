// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 不坑老师 · https://github.com/44886/Exeify

//! 打包器自更新：基于 GitHub Release 检测最新版、下载并自替换重启。
//!
//! 仅更新打包器工具本身（`exeify.exe`），不涉及打包产物 runner 逻辑。
//! 仅手动触发（「关于」弹窗里点「检查更新」），不静默联网。

use anyhow::{Context, Result};
use serde::Deserialize;

/// 查询最新 Release 的 GitHub API 地址。
const RELEASE_API: &str = "https://api.github.com/repos/44886/Exeify/releases/latest";
/// 请求 GitHub API/资产时使用的 User-Agent（GitHub 要求带 UA）。
const USER_AGENT: &str = "exeify-updater";
/// 下载体积上限（256MB），防止异常响应耗尽内存。
const MAX_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;

/// 当前打包器版本，来自 `Cargo.toml`。
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// 解析出的最新版本信息。
#[derive(Debug, Clone)]
pub struct Latest {
    /// 原始 tag（可能带 `v` 前缀），如 `v0.3.1`。保留以便调试/日志。
    #[allow(dead_code)]
    pub tag: String,
    /// 归一化后的版本号（去掉前导 `v`），如 `0.3.1`。
    pub version: String,
    /// 首个以 `.exe` 结尾资产的下载地址。
    pub asset_url: String,
    /// Release 说明正文。
    pub notes: String,
}

/// 语义化版本比较：`latest` 是否严格比 `current` 新。
///
/// 解析 `major.minor.patch`，容忍：
/// - 前导 `v` / `V`（如 `v0.3.1`）
/// - 位数不齐（如 `0.3` 视作 `0.3.0`）
///
/// 任一侧解析失败则视为「不更新」返回 `false`（保守，避免误升级）。
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

/// 把版本字符串解析成 `(major, minor, patch)`，非法输入返回 `None`。
fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim();
    let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
    if s.is_empty() {
        return None;
    }
    let mut parts = s.split('.');
    let major = parse_component(parts.next())?;
    let minor = parse_component(parts.next().or(Some("0")))?;
    let patch = parse_component(parts.next().or(Some("0")))?;
    // 多余的分段（如 0.3.1.4）忽略，只要前三段合法即可。
    Some((major, minor, patch))
}

/// 解析单个版本分量；缺省视作 `0`，非数字视作非法。
fn parse_component(part: Option<&str>) -> Option<u64> {
    match part {
        None => Some(0),
        Some(p) => {
            let p = p.trim();
            if p.is_empty() {
                Some(0)
            } else {
                p.parse::<u64>().ok()
            }
        }
    }
}

// ---- GitHub API 响应结构（仅取需要的字段）----

#[derive(Debug, Deserialize)]
struct ReleaseResp {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    assets: Vec<AssetResp>,
}

#[derive(Debug, Deserialize)]
struct AssetResp {
    #[serde(default)]
    name: String,
    #[serde(default)]
    browser_download_url: String,
}

/// 查询最新 Release。带 GitHub 要求的 `User-Agent` 与 `Accept` 头，
/// 解析出 tag、说明与首个 `.exe` 资产下载地址。
pub fn check_latest() -> Result<Latest> {
    let body = ureq::get(RELEASE_API)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github+json")
        .call()
        .context("请求 GitHub 失败（可能无网络或被限流）")?
        .body_mut()
        .read_to_string()
        .context("读取 GitHub 响应失败")?;

    let resp: ReleaseResp = serde_json::from_str(&body).context("解析 GitHub Release 响应失败")?;

    let tag = resp.tag_name.trim().to_string();
    if tag.is_empty() {
        anyhow::bail!("未获取到版本号（tag_name 为空）");
    }
    let version = tag.strip_prefix(['v', 'V']).unwrap_or(&tag).to_string();

    let asset_url = resp
        .assets
        .iter()
        .find(|a| a.name.to_ascii_lowercase().ends_with(".exe"))
        .map(|a| a.browser_download_url.clone())
        .filter(|u| !u.is_empty())
        .context("最新版本未附带可下载的 .exe 资产")?;

    Ok(Latest {
        tag,
        version,
        asset_url,
        notes: resp.body,
    })
}

/// 下载资产字节。ureq 默认跟随重定向到 CDN。
pub fn download(url: &str) -> Result<Vec<u8>> {
    let bytes = ureq::get(url)
        .header("User-Agent", USER_AGENT)
        .call()
        .context("下载失败（网络中断或被拦截）")?
        .body_mut()
        .with_config()
        .limit(MAX_DOWNLOAD_BYTES)
        .read_to_vec()
        .context("读取下载内容失败")?;
    if bytes.is_empty() {
        anyhow::bail!("下载内容为空");
    }
    Ok(bytes)
}

/// 计算与当前 exe 同目录、`<stem>.old.exe` 的备份路径。
fn old_exe_path(current: &std::path::Path) -> std::path::PathBuf {
    let stem = current
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "app".to_string());
    let dir = current
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    dir.join(format!("{stem}.old.exe"))
}

/// 自替换并重启：
/// 1. `fs::rename(current, old)`（Windows 允许重命名运行中的 exe）
/// 2. 把新字节写入 `current`
/// 3. 启动新版进程，本进程退出（成功则不返回）
///
/// 写失败会回滚（把 old 改名回 current）并返回错误；
/// exe 目录不可写时返回清晰错误，供 UI 提示。
pub fn apply_update(new_exe: &[u8]) -> Result<()> {
    let current = std::env::current_exe().context("无法定位当前程序路径")?;
    let old = old_exe_path(&current);

    // 先删残留 old（best-effort），避免 rename 目标已存在导致失败。
    let _ = std::fs::remove_file(&old);

    std::fs::rename(&current, &old).with_context(|| {
        format!(
            "无法替换程序文件（目录可能不可写，如 Program Files）：{}",
            current.display()
        )
    })?;

    // 写新字节到原路径；失败则回滚。
    if let Err(e) = std::fs::write(&current, new_exe) {
        // 回滚：把备份改名回原路径。
        let _ = std::fs::rename(&old, &current);
        return Err(
            anyhow::Error::new(e).context(format!("写入新版本失败，已回滚：{}", current.display()))
        );
    }

    // 启动新版；即使启动失败也已完成替换，下次手动启动即为新版。
    std::process::Command::new(&current)
        .spawn()
        .with_context(|| format!("已更新，但重启新版本失败：{}", current.display()))?;

    // 成功：退出当前（旧）进程。
    std::process::exit(0);
}

/// best-effort 删除残留的 `<stem>.old.exe`，忽略任何错误。
/// 由 `main()` 启动时调用（此时旧进程已退出，文件可删）。
pub fn cleanup_old() {
    if let Ok(current) = std::env::current_exe() {
        let old = old_exe_path(&current);
        let _ = std::fs::remove_file(old);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_patch_is_newer() {
        assert!(is_newer("0.3.1", "0.3.0"));
    }

    #[test]
    fn newer_minor_is_newer() {
        assert!(is_newer("0.4.0", "0.3.9"));
    }

    #[test]
    fn newer_major_is_newer() {
        assert!(is_newer("1.0.0", "0.9.9"));
    }

    #[test]
    fn older_is_not_newer() {
        assert!(!is_newer("0.2.1", "0.3.0"));
        assert!(!is_newer("0.3.0", "0.3.1"));
        assert!(!is_newer("0.9.9", "1.0.0"));
    }

    #[test]
    fn equal_is_not_newer() {
        assert!(!is_newer("0.3.0", "0.3.0"));
        assert!(!is_newer("1.2.3", "1.2.3"));
    }

    #[test]
    fn tolerates_missing_patch_component() {
        // 0.3 == 0.3.0，不算更新
        assert!(!is_newer("0.3", "0.3.0"));
        assert!(!is_newer("0.3.0", "0.3"));
        // 0.4 > 0.3.0
        assert!(is_newer("0.4", "0.3.0"));
        // 0.3.1 > 0.3
        assert!(is_newer("0.3.1", "0.3"));
    }

    #[test]
    fn tolerates_missing_minor_component() {
        // 1 == 1.0.0
        assert!(!is_newer("1", "1.0.0"));
        assert!(is_newer("2", "1.9.9"));
    }

    #[test]
    fn tolerates_v_prefix() {
        assert!(is_newer("v0.3.1", "0.3.0"));
        assert!(is_newer("0.3.1", "v0.3.0"));
        assert!(is_newer("v0.3.1", "v0.3.0"));
        assert!(!is_newer("v0.3.0", "v0.3.0"));
        // 大写 V 也容忍
        assert!(is_newer("V0.3.1", "V0.3.0"));
    }

    #[test]
    fn tolerates_whitespace() {
        assert!(is_newer("  0.3.1  ", "0.3.0"));
        assert!(!is_newer("0.3.0", "  0.3.0  "));
    }

    #[test]
    fn invalid_input_is_not_newer() {
        // 非法输入保守返回 false，不误升级
        assert!(!is_newer("", "0.3.0"));
        assert!(!is_newer("0.3.0", ""));
        assert!(!is_newer("abc", "0.3.0"));
        assert!(!is_newer("0.3.0", "xyz"));
        assert!(!is_newer("1.x.0", "0.3.0"));
        assert!(!is_newer("v", "0.3.0"));
    }

    #[test]
    fn ignores_extra_components() {
        // 只比较前三段
        assert!(!is_newer("0.3.0.4", "0.3.0.9"));
        assert!(is_newer("0.3.1.0", "0.3.0.9"));
    }

    #[test]
    fn old_exe_path_appends_old_suffix() {
        let p = std::path::Path::new("C:\\tools\\exeify.exe");
        let old = old_exe_path(p);
        assert_eq!(old.file_name().unwrap().to_string_lossy(), "exeify.old.exe");
        assert_eq!(old.parent(), p.parent());
    }
}
