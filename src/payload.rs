// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 不坑老师 · https://github.com/44886/html2exe

//! 产物 exe 尾部载荷的读写。
//!
//! 尾部格式（追加在原始 exe 之后）：
//! ```text
//! [ payload 字节 ]         本地模式=zip；URL 模式=空
//! [ config json 字节 ]
//! [ u32 LE: config_len ]
//! [ u64 LE: payload_len ]
//! [ 8 字节 MAGIC ]
//! ```

use crate::config::PackConfig;
use anyhow::{anyhow, Context, Result};

pub const MAGIC: &[u8; 8] = b"H2EPKG01";
const TRAILER_META: usize = 4 + 8 + 8; // config_len(u32) + payload_len(u64) + magic(8)

/// 已解析的载荷
pub struct Payload {
    pub config: PackConfig,
    /// 本地模式的 zip 字节；URL 模式为空
    pub archive: Vec<u8>,
}

/// 从当前 exe 读取载荷。无魔数返回 Ok(None)（即 GUI 模式）。
pub fn read_from_current_exe() -> Result<Option<Payload>> {
    let exe = std::env::current_exe().context("无法定位当前 exe")?;
    let bytes = std::fs::read(&exe).context("无法读取当前 exe")?;
    read_from_bytes(&bytes)
}

/// 从字节流尾部解析载荷。
pub fn read_from_bytes(bytes: &[u8]) -> Result<Option<Payload>> {
    let n = bytes.len();
    if n < TRAILER_META {
        return Ok(None);
    }
    if &bytes[n - 8..n] != MAGIC {
        return Ok(None);
    }
    let payload_len = u64::from_le_bytes(bytes[n - 16..n - 8].try_into().unwrap()) as usize;
    let config_len = u32::from_le_bytes(bytes[n - 20..n - 16].try_into().unwrap()) as usize;

    let config_end = n - 20;
    let config_start = config_end
        .checked_sub(config_len)
        .ok_or_else(|| anyhow!("载荷损坏：config 长度越界"))?;
    let payload_start = config_start
        .checked_sub(payload_len)
        .ok_or_else(|| anyhow!("载荷损坏：payload 长度越界"))?;

    let config: PackConfig = serde_json::from_slice(&bytes[config_start..config_end])
        .context("解析打包配置失败")?;
    let archive = bytes[payload_start..config_start].to_vec();

    Ok(Some(Payload { config, archive }))
}

/// 把 config + payload 追加到 stub 字节后，返回完整产物字节。
pub fn build_output(stub: &[u8], config: &PackConfig, archive: &[u8]) -> Result<Vec<u8>> {
    let config_json = serde_json::to_vec(config).context("序列化配置失败")?;
    let mut out = Vec::with_capacity(stub.len() + archive.len() + config_json.len() + TRAILER_META);
    out.extend_from_slice(stub);
    out.extend_from_slice(archive);
    out.extend_from_slice(&config_json);
    out.extend_from_slice(&(config_json.len() as u32).to_le_bytes());
    out.extend_from_slice(&(archive.len() as u64).to_le_bytes());
    out.extend_from_slice(MAGIC);
    Ok(out)
}

/// 返回当前 exe 去掉已有载荷后的"干净 stub"字节（用于自我复制打包）。
/// 若当前 exe 本身没有载荷，则原样返回。
pub fn clean_stub_from_current_exe() -> Result<Vec<u8>> {
    let exe = std::env::current_exe().context("无法定位当前 exe")?;
    let bytes = std::fs::read(&exe).context("无法读取当前 exe")?;
    Ok(strip_trailer(bytes))
}

/// 去掉尾部载荷，返回原始 stub。
fn strip_trailer(bytes: Vec<u8>) -> Vec<u8> {
    let n = bytes.len();
    if n < TRAILER_META || &bytes[n - 8..n] != MAGIC {
        return bytes;
    }
    let payload_len = u64::from_le_bytes(bytes[n - 16..n - 8].try_into().unwrap()) as usize;
    let config_len = u32::from_le_bytes(bytes[n - 20..n - 16].try_into().unwrap()) as usize;
    let total_trailer = TRAILER_META + config_len + payload_len;
    if total_trailer > n {
        return bytes;
    }
    let mut b = bytes;
    b.truncate(b.len() - total_trailer);
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Mode, PackConfig, WindowCfg};

    fn sample_cfg() -> PackConfig {
        PackConfig {
            mode: Mode::Local,
            url: None,
            entry: "index.html".into(),
            window: WindowCfg::default(),
        }
    }

    #[test]
    fn no_magic_means_gui_mode() {
        let stub = b"just an exe with no trailer".to_vec();
        assert!(read_from_bytes(&stub).unwrap().is_none());
    }

    #[test]
    fn roundtrip_local_payload() {
        let stub = b"PRETEND_EXE_BYTES".to_vec();
        let archive = b"zip-bytes-here".to_vec();
        let cfg = sample_cfg();
        let out = build_output(&stub, &cfg, &archive).unwrap();

        let p = read_from_bytes(&out).unwrap().expect("应能解析出载荷");
        assert_eq!(p.config.mode, Mode::Local);
        assert_eq!(p.config.entry, "index.html");
        assert_eq!(p.archive, archive);
    }

    #[test]
    fn roundtrip_url_payload_empty_archive() {
        let stub = b"EXE".to_vec();
        let cfg = PackConfig {
            mode: Mode::Url,
            url: Some("https://example.com".into()),
            entry: String::new(),
            window: WindowCfg::default(),
        };
        let out = build_output(&stub, &cfg, &[]).unwrap();
        let p = read_from_bytes(&out).unwrap().unwrap();
        assert_eq!(p.config.mode, Mode::Url);
        assert_eq!(p.config.url.as_deref(), Some("https://example.com"));
        assert!(p.archive.is_empty());
    }

    #[test]
    fn strip_trailer_recovers_original_stub() {
        let stub = b"ORIGINAL_STUB_BYTES_1234567890".to_vec();
        let out = build_output(&stub, &sample_cfg(), b"payload").unwrap();
        assert_eq!(strip_trailer(out), stub);
    }

    #[test]
    fn strip_trailer_noop_without_magic() {
        let raw = b"no trailer at all".to_vec();
        assert_eq!(strip_trailer(raw.clone()), raw);
    }
}
