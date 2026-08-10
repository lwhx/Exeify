// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 不坑老师 · https://github.com/44886/html2exe

//! runner 模式：读取自身载荷并用 WebView 显示。

use crate::config::{Mode, PackConfig};
use crate::payload::Payload;
use anyhow::{bail, Context, Result};
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};
use wry::{
    http::{header::CONTENT_TYPE, Request, Response},
    WebViewBuilder,
};

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

fn guess_mime(path: &str) -> &'static str {
    // 常见类型手动映射，兜底用 octet-stream
    match path
        .rsplit('.')
        .next()
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("html") | Some("htm") => "text/html",
        Some("js") | Some("mjs") => "text/javascript",
        Some("css") => "text/css",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("wasm") => "application/wasm",
        Some("txt") => "text/plain",
        _ => "application/octet-stream",
    }
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

    let builder = match mode {
        Mode::Url => {
            let target = url.context("URL 模式缺少目标地址")?;
            WebViewBuilder::new().with_url(target)
        }
        Mode::Local => {
            let assets = Arc::new(unzip_to_map(&payload.archive)?);
            let entry = if entry.is_empty() {
                "index.html".to_string()
            } else {
                entry
            };
            let assets_cl = assets.clone();
            let entry_cl = entry.clone();
            // 直接导航到入口文件本身，使其相对资源（含子目录入口）能正确解析
            let start_url = format!(
                "{}{}",
                crate::asset_base_url(),
                entry.trim_start_matches('/')
            );
            WebViewBuilder::new()
                .with_url(start_url)
                .with_custom_protocol("app".to_string(), move |_id, request| {
                    serve_local(&assets_cl, &entry_cl, &request)
                })
        }
    };

    let _webview = builder.build(&win).context("创建 WebView 失败")?;

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

fn serve_local(
    assets: &HashMap<String, Vec<u8>>,
    entry: &str,
    request: &Request<Vec<u8>>,
) -> Response<Cow<'static, [u8]>> {
    let path = request.uri().path();
    let mut key = path.trim_start_matches('/').to_string();
    if key.is_empty() {
        key = entry.to_string();
    }
    let hit = assets.contains_key(&key);
    if let Ok(log) = std::env::var("HTML2EXE_LOG") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
        {
            let _ = writeln!(
                f,
                "{} {} -> {}",
                request.uri(),
                key,
                if hit { "200" } else { "404" }
            );
        }
    }
    match assets.get(&key) {
        Some(data) => Response::builder()
            .header(CONTENT_TYPE, guess_mime(&key))
            .body(Cow::Owned(data.clone()))
            .unwrap(),
        None => Response::builder()
            .status(404)
            .header(CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(Cow::Owned(format!("404 未找到：{key}").into_bytes()))
            .unwrap(),
    }
}

/// 供 main 调用：无载荷时不应进入此函数。
#[allow(dead_code)]
pub fn ensure_payload(p: Option<Payload>) -> Result<Payload> {
    match p {
        Some(p) => Ok(p),
        None => bail!("没有可运行的载荷"),
    }
}
