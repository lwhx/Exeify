// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 不坑老师 · https://github.com/44886/html2exe

//! runner 模式：读取自身载荷并用 WebView 显示。
//!
//! 本地模式通过内置的回环 HTTP 服务器（`http://127.0.0.1`）提供网页资源，
//! 以便 Vite/Vue/React 等框架的 ES Module 与前端路由正常工作。

use crate::config::{Mode, PackConfig};
use crate::payload::Payload;
use crate::server;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::Read;
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};
use wry::WebViewBuilder;

/// 把 zip 全部解压进内存，键为规范化后的相对路径（无前导斜杠）。
fn unzip_to_map(archive: &[u8]) -> Result<HashMap<String, Vec<u8>>> {
    let reader = std::io::Cursor::new(archive);
    let mut zip = zip::ZipArchive::new(reader).context("打开内嵌资源失败")?;
    let mut map = HashMap::new();
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().trim_start_matches('/').to_string();
        let mut data = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut data)?;
        map.insert(name, data);
    }
    Ok(map)
}

pub fn run(payload: Payload) -> Result<()> {
    let PackConfig {
        mode,
        url,
        entry,
        window,
    } = payload.config.clone();

    let event_loop = EventLoop::new();
    let win = WindowBuilder::new()
        .with_title(&window.title)
        .with_inner_size(LogicalSize::new(window.width, window.height))
        .with_resizable(window.resizable)
        .with_window_icon(crate::app_window_icon())
        .build(&event_loop)
        .context("创建窗口失败")?;

    let target_url = match mode {
        Mode::Url => url.context("URL 模式缺少目标地址")?,
        Mode::Local => {
            let assets = unzip_to_map(&payload.archive)?;
            let entry = if entry.is_empty() {
                "index.html".to_string()
            } else {
                entry
            };
            let port = server::start(assets, entry.clone())?;
            format!("http://127.0.0.1:{port}/{}", entry.trim_start_matches('/'))
        }
    };

    let _webview = WebViewBuilder::new()
        .with_url(target_url)
        .build(&win)
        .context("创建 WebView 失败")?;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            *control_flow = ControlFlow::Exit;
        }
    });
}
