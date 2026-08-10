//! GUI 打包器模式：白色极简界面 + 通过 IPC 驱动打包。

use crate::config::WindowCfg;
use crate::packer;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::borrow::Cow;
use std::path::PathBuf;
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::WindowBuilder,
};
use wry::{
    http::{header::CONTENT_TYPE, Request, Response},
    WebViewBuilder,
};

/// 事件循环自定义事件
enum UserEvent {
    /// 来自前端的原始 IPC 消息
    Ipc(String),
    /// 在 WebView 中执行 JS
    Eval(String),
}

/// 前端发来的消息
#[derive(Debug, Deserialize)]
struct IpcMsg {
    action: String,
    #[serde(default)]
    data: Option<PackReq>,
    #[serde(default, rename = "defaultName")]
    default_name: Option<String>,
}

/// 打包请求
#[derive(Debug, Deserialize)]
struct PackReq {
    mode: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    folder: Option<String>,
    #[serde(default)]
    entry: Option<String>,
    #[serde(default)]
    icon: Option<String>,
    title: String,
    width: f64,
    height: f64,
    resizable: bool,
    output: String,
}

/// 把图标文件读成 data URL，用于界面预览。文件过大则返回 None。
fn icon_data_url(path: &std::path::Path) -> Option<String> {
    let data = std::fs::read(path).ok()?;
    if data.len() > 4 * 1024 * 1024 {
        return None;
    }
    let mime = match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    };
    Some(format!("data:{};base64,{}", mime, base64_encode(&data)))
}

/// 极简 base64 编码（标准字母表），避免额外依赖。
fn base64_encode(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[(n >> 18 & 63) as usize] as char);
        out.push(T[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

fn js_str(s: &str) -> String {
    // 用 serde 生成安全的 JS 字符串字面量
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

pub fn run() -> Result<()> {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let window = WindowBuilder::new()
        .with_title("html2exe — 网页打包器")
        .with_inner_size(LogicalSize::new(560.0, 720.0))
        .with_min_inner_size(LogicalSize::new(480.0, 600.0))
        .with_resizable(true)
        .build(&event_loop)
        .context("创建窗口失败")?;

    let ipc_proxy = proxy.clone();
    let webview = WebViewBuilder::new()
        .with_url(crate::asset_base_url())
        .with_custom_protocol("app".to_string(), move |_id, request| serve_ui(&request))
        .with_ipc_handler(move |req: Request<String>| {
            let _ = ipc_proxy.send_event(UserEvent::Ipc(req.into_body()));
        })
        .build(&window)
        .context("创建 WebView 失败")?;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control_flow = ControlFlow::Exit,
            Event::UserEvent(UserEvent::Eval(js)) => {
                let _ = webview.evaluate_script(&js);
            }
            Event::UserEvent(UserEvent::Ipc(msg)) => {
                handle_ipc(&msg, &proxy, &webview);
            }
            _ => {}
        }
    });
}

fn handle_ipc(msg: &str, proxy: &tao::event_loop::EventLoopProxy<UserEvent>, webview: &wry::WebView) {
    let parsed: IpcMsg = match serde_json::from_str(msg) {
        Ok(m) => m,
        Err(e) => {
            let _ = webview.evaluate_script(&format!(
                "window.__log({});",
                js_str(&format!("消息解析失败：{e}"))
            ));
            return;
        }
    };

    match parsed.action.as_str() {
        // 选择本地目录 —— 在主线程弹原生对话框，并自动识别网页入口
        "pickFolder" => {
            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                let path = folder.to_string_lossy().to_string();
                let entries = packer::detect_html_entries(&folder);
                let payload = serde_json::json!({
                    "folder": path,
                    "entries": entries,
                    "entry": entries.first().cloned().unwrap_or_default(),
                });
                let _ = webview
                    .evaluate_script(&format!("window.__onFolderPicked({});", payload));
            }
        }
        // 选择图标文件（.ico/.png）
        "pickIcon" => {
            let dialog = rfd::FileDialog::new().add_filter("图标", &["ico", "png"]);
            if let Some(path) = dialog.pick_file() {
                let s = path.to_string_lossy().to_string();
                let preview = icon_data_url(&path).unwrap_or_default();
                let _ = webview.evaluate_script(&format!(
                    "window.__setIcon({}, {});",
                    js_str(&s),
                    js_str(&preview)
                ));
            }
        }
        // 选择输出 exe 路径
        "pickOutput" => {
            let default_name = parsed.default_name.unwrap_or_else(|| "app.exe".to_string());
            let dialog = rfd::FileDialog::new()
                .set_file_name(&default_name)
                .add_filter("可执行文件", &["exe"]);
            if let Some(path) = dialog.save_file() {
                let mut path = path;
                if path.extension().is_none() {
                    path.set_extension("exe");
                }
                let s = path.to_string_lossy().to_string();
                let _ = webview.evaluate_script(&format!("window.__setOutput({});", js_str(&s)));
            }
        }
        // 打包 —— 放到后台线程，完成后回调
        "pack" => {
            let Some(req) = parsed.data else {
                let _ = webview
                    .evaluate_script(&format!("window.__packResult(false, {});", js_str("缺少打包参数")));
                return;
            };
            let proxy = proxy.clone();
            std::thread::spawn(move || {
                let result = do_pack(req);
                let js = match result {
                    Ok(out) => format!(
                        "window.__packResult(true, {});",
                        js_str(&format!("打包成功：{out}"))
                    ),
                    Err(e) => format!("window.__packResult(false, {});", js_str(&format!("{e}"))),
                };
                let _ = proxy.send_event(UserEvent::Eval(js));
            });
        }
        other => {
            let _ = webview.evaluate_script(&format!(
                "window.__log({});",
                js_str(&format!("未知动作：{other}"))
            ));
        }
    }
}

fn do_pack(req: PackReq) -> Result<String> {
    let window = WindowCfg {
        title: if req.title.trim().is_empty() {
            "App".to_string()
        } else {
            req.title.clone()
        },
        width: if req.width > 0.0 { req.width } else { 1024.0 },
        height: if req.height > 0.0 { req.height } else { 720.0 },
        resizable: req.resizable,
    };
    let output = PathBuf::from(&req.output);
    if req.output.trim().is_empty() {
        anyhow::bail!("请先选择输出路径");
    }

    let icon_path = req
        .icon
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    let icon = icon_path.as_deref();

    match req.mode.as_str() {
        "url" => {
            let url = req.url.unwrap_or_default();
            packer::pack_url(&url, window, &output, icon)?;
        }
        "local" => {
            let folder = req.folder.unwrap_or_default();
            if folder.trim().is_empty() {
                anyhow::bail!("请先选择本地网页目录");
            }
            let entry = req.entry.filter(|e| !e.trim().is_empty()).unwrap_or_else(|| "index.html".to_string());
            packer::pack_local(&PathBuf::from(folder), &entry, window, &output, icon)?;
        }
        m => anyhow::bail!("未知模式：{m}"),
    }
    Ok(output.to_string_lossy().to_string())
}

// ---- 内嵌 UI 资源 ----

fn serve_ui(request: &Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let path = request.uri().path().trim_start_matches('/');
    let key = if path.is_empty() { "index.html" } else { path };
    match ui_asset(key) {
        Some((data, mime)) => Response::builder()
            .header(CONTENT_TYPE, mime)
            .body(Cow::Borrowed(data))
            .unwrap(),
        None => Response::builder()
            .status(404)
            .header(CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(Cow::Owned(format!("404: {key}").into_bytes()))
            .unwrap(),
    }
}

fn ui_asset(key: &str) -> Option<(&'static [u8], &'static str)> {
    match key {
        "index.html" => Some((include_bytes!("../ui/index.html"), "text/html; charset=utf-8")),
        "style.css" => Some((include_bytes!("../ui/style.css"), "text/css; charset=utf-8")),
        "app.js" => Some((include_bytes!("../ui/app.js"), "text/javascript; charset=utf-8")),
        "qrcode.jpg" => Some((include_bytes!("../ui/qrcode.jpg"), "image/jpeg")),
        _ => None,
    }
}
