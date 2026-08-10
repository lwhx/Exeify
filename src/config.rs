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

/// 窗口配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowCfg {
    pub title: String,
    pub width: f64,
    pub height: f64,
    pub resizable: bool,
}

impl Default for WindowCfg {
    fn default() -> Self {
        Self {
            title: "App".to_string(),
            width: 1024.0,
            height: 720.0,
            resizable: true,
        }
    }
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
}

fn default_entry() -> String {
    "index.html".to_string()
}
