// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 不坑老师 · https://github.com/44886/Exeify

//! 打包配置：写进产物 exe 尾部，runner 启动时读取。

use serde::{Deserialize, Serialize};

/// 打包模式
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// 在线网址
    Url,
    /// 本地 HTML 目录（资源以 zip 追加在 payload 中）
    Local,
}

/// 窗口启动状态
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum WindowState {
    /// 普通窗口（用宽高）
    #[default]
    Normal,
    /// 最大化
    Maximized,
    /// 全屏无边框
    Fullscreen,
}

/// 窗口配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowCfg {
    pub title: String,
    pub width: f64,
    pub height: f64,
    pub resizable: bool,
    /// 启动状态（普通/最大化/全屏），旧产物无此字段时默认 Normal
    #[serde(default)]
    pub state: WindowState,
}

impl Default for WindowCfg {
    fn default() -> Self {
        Self {
            title: "App".to_string(),
            width: 1024.0,
            height: 720.0,
            resizable: true,
            state: WindowState::Normal,
        }
    }
}

/// 启动页配置：产物启动时先显示的品牌图，消除 WebView 初始化白屏。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplashCfg {
    /// png/jpg 原始字节的 base64
    pub image_b64: String,
    /// "image/png" | "image/jpeg"
    pub mime: String,
    /// 最少显示毫秒
    pub min_ms: u64,
    /// 背景色 "#rrggbb"（图片加载出来前兜底）
    pub bg: String,
    /// 图片填充方式，当前固定 "cover"
    pub fit: String,
}

/// 运行时窗口/任务栏图标（区别于 exe 文件图标）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowIcon {
    pub w: u32,
    pub h: u32,
    /// RGBA8 原始像素的 base64
    pub rgba_b64: String,
}

/// 内嵌资源加密信息（源码保护开启时写入）。key/nonce 均为遮蔽后的 base64。
/// 诚实：密钥随 exe 内嵌、格式公开，仅提高门槛，不是不可破解的安全。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncInfo {
    /// 遮蔽后的 key（base64）
    pub key_b64: String,
    /// 遮蔽后的 nonce（base64）
    pub nonce_b64: String,
}

/// 完整打包配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackConfig {
    pub mode: Mode,
    /// URL 模式下的目标地址
    #[serde(default)]
    pub url: Option<String>,
    /// 本地模式入口文件（相对路径），默认 index.html
    #[serde(default = "default_entry")]
    pub entry: String,
    #[serde(default)]
    pub window: WindowCfg,
    /// 启动页图片（可选）。为空时不显示启动页，行为与旧版一致。
    #[serde(default)]
    pub splash: Option<SplashCfg>,
    /// 运行时窗口/任务栏图标（可选）。为空时回退到内置图标。
    #[serde(default)]
    pub window_icon: Option<WindowIcon>,
    /// 内嵌资源加密信息。None=未加密（旧产物 / 未开启保护）。
    #[serde(default)]
    pub enc: Option<EncInfo>,
    /// 是否开启源码保护（禁用 DevTools + 屏蔽右键/开发者快捷键）。
    #[serde(default)]
    pub protect: bool,
}

fn default_entry() -> String {
    "index.html".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_config_json_deserializes_with_defaults() {
        // 旧产物 JSON：没有 state / splash / window_icon 字段
        let json = r#"{
            "mode": "url",
            "url": "https://example.com",
            "entry": "",
            "window": { "title": "App", "width": 1024.0, "height": 720.0, "resizable": true }
        }"#;
        let cfg: PackConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.window.state, WindowState::Normal);
        assert!(cfg.splash.is_none());
        assert!(cfg.window_icon.is_none());
        // 旧 JSON 无 enc/protect 字段：应默认 None / false，向后兼容。
        assert!(cfg.enc.is_none());
        assert!(!cfg.protect);
    }

    #[test]
    fn window_state_roundtrips_lowercase() {
        for (state, tag) in [
            (WindowState::Normal, "\"normal\""),
            (WindowState::Maximized, "\"maximized\""),
            (WindowState::Fullscreen, "\"fullscreen\""),
        ] {
            let s = serde_json::to_string(&state).unwrap();
            assert_eq!(s, tag);
            let back: WindowState = serde_json::from_str(&s).unwrap();
            assert_eq!(back, state);
        }
    }

    #[test]
    fn full_config_roundtrips() {
        let cfg = PackConfig {
            mode: Mode::Local,
            url: None,
            entry: "index.html".into(),
            window: WindowCfg {
                state: WindowState::Fullscreen,
                ..Default::default()
            },
            splash: Some(SplashCfg {
                image_b64: "AAAA".into(),
                mime: "image/png".into(),
                min_ms: 1500,
                bg: "#0f172a".into(),
                fit: "cover".into(),
            }),
            window_icon: Some(WindowIcon {
                w: 256,
                h: 256,
                rgba_b64: "BBBB".into(),
            }),
            enc: Some(EncInfo {
                key_b64: "KKKK".into(),
                nonce_b64: "NNNN".into(),
            }),
            protect: true,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: PackConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.window.state, WindowState::Fullscreen);
        assert_eq!(back.splash.as_ref().unwrap().min_ms, 1500);
        assert_eq!(back.window_icon.as_ref().unwrap().w, 256);
        assert_eq!(back.enc.as_ref().unwrap().key_b64, "KKKK");
        assert!(back.protect);
    }
}
