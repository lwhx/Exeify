// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 不坑老师 · https://github.com/44886/Exeify

//! GUI 打包器模式：白色极简界面 + 通过 IPC 驱动打包。

use crate::config::{SplashCfg, WindowCfg, WindowState};
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
    #[serde(default)]
    url: Option<String>,
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
    /// 启动状态："normal" | "maximized" | "fullscreen"
    #[serde(default)]
    state: Option<String>,
    /// 启动图文件路径（.png/.jpg），空则不生成启动页
    #[serde(default)]
    splash: Option<String>,
    /// 启动页最少显示毫秒
    #[serde(default)]
    splash_ms: Option<u64>,
    /// 启动页背景色 "#rrggbb"
    #[serde(default)]
    splash_bg: Option<String>,
    /// 源码保护：加密内嵌资源 + 运行时禁用查看。默认开启。
    #[serde(default = "default_true")]
    protect: bool,
}

/// serde 默认值：源码保护默认开启。
fn default_true() -> bool {
    true
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
    Some(format!(
        "data:{};base64,{}",
        mime,
        crate::b64::encode(&data)
    ))
}

/// 把启动图文件读成 data URL（用于界面预览）。仅接受 png/jpg，过大返回 None。
fn splash_data_url(path: &std::path::Path) -> Option<String> {
    let data = std::fs::read(path).ok()?;
    if data.len() > 8 * 1024 * 1024 {
        return None;
    }
    let mime = packer::splash_mime(path)?;
    Some(format!(
        "data:{};base64,{}",
        mime,
        crate::b64::encode(&data)
    ))
}

fn js_str(s: &str) -> String {
    // 用 serde 生成安全的 JS 字符串字面量
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

pub fn run() -> Result<()> {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let window = WindowBuilder::new()
        .with_title("Exeify — 网页打包器")
        .with_inner_size(LogicalSize::new(880.0, 640.0))
        .with_min_inner_size(LogicalSize::new(760.0, 560.0))
        .with_resizable(true)
        .with_window_icon(crate::app_window_icon())
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

fn handle_ipc(
    msg: &str,
    proxy: &tao::event_loop::EventLoopProxy<UserEvent>,
    webview: &wry::WebView,
) {
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
                let _ = webview.evaluate_script(&format!("window.__onFolderPicked({});", payload));
            }
        }
        // 用系统默认浏览器打开外部链接（避免把打包器界面导航走）
        "openExternal" => {
            if let Some(url) = parsed.url.as_deref() {
                if url.starts_with("http://") || url.starts_with("https://") {
                    let _ = std::process::Command::new("explorer").arg(url).spawn();
                }
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
        // 选择启动图（.png/.jpg）
        "pickSplash" => {
            let dialog = rfd::FileDialog::new().add_filter("图片", &["png", "jpg", "jpeg"]);
            if let Some(path) = dialog.pick_file() {
                let s = path.to_string_lossy().to_string();
                let preview = splash_data_url(&path).unwrap_or_default();
                let _ = webview.evaluate_script(&format!(
                    "window.__setSplash({}, {});",
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
        // 检查更新 —— 后台线程查 GitHub 最新版并回调前端
        "checkUpdate" => {
            let proxy = proxy.clone();
            std::thread::spawn(move || {
                let js = build_check_update_js();
                let _ = proxy.send_event(UserEvent::Eval(js));
            });
        }
        // 立即更新 —— 后台线程下载并自替换重启（成功则进程退出，无回调）
        "doUpdate" => {
            let url = parsed.url.unwrap_or_default();
            let proxy = proxy.clone();
            std::thread::spawn(move || {
                if url.trim().is_empty() {
                    let js = format!(
                        "window.__updateResult(false, {});",
                        js_str("缺少下载地址，请重新检查更新")
                    );
                    let _ = proxy.send_event(UserEvent::Eval(js));
                    return;
                }
                match do_update(&url) {
                    // 成功时 apply_update 会退出进程，通常走不到这里。
                    Ok(()) => {}
                    Err(e) => {
                        let js = format!(
                            "window.__updateResult(false, {});",
                            js_str(&format!("{e:#}"))
                        );
                        let _ = proxy.send_event(UserEvent::Eval(js));
                    }
                }
            });
        }
        // 打包 —— 放到后台线程，完成后回调
        "pack" => {
            let Some(req) = parsed.data else {
                let _ = webview.evaluate_script(&format!(
                    "window.__packResult(false, {});",
                    js_str("缺少打包参数")
                ));
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
        state: parse_state(req.state.as_deref()),
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

    let splash = build_splash(&req)?;

    let protect = req.protect;
    match req.mode.as_str() {
        "url" => {
            let url = req.url.unwrap_or_default();
            packer::pack_url(&url, window, &output, icon, splash, protect)?;
        }
        "local" => {
            let folder = req.folder.unwrap_or_default();
            if folder.trim().is_empty() {
                anyhow::bail!("请先选择本地网页目录");
            }
            let entry = req
                .entry
                .filter(|e| !e.trim().is_empty())
                .unwrap_or_else(|| "index.html".to_string());
            packer::pack_local(
                &PathBuf::from(folder),
                &entry,
                window,
                &output,
                icon,
                splash,
                protect,
            )?;
        }
        m => anyhow::bail!("未知模式：{m}"),
    }
    Ok(output.to_string_lossy().to_string())
}

/// 后台查最新版本，生成回调前端 `window.__updateInfo({...})` 的 JS。
///
/// 回调字段：ok(bool)、current、latest、hasUpdate(bool)、notes、error、url。
fn build_check_update_js() -> String {
    let current = crate::update::current_version();
    let payload = match crate::update::check_latest() {
        Ok(latest) => {
            let has_update = crate::update::is_newer(&latest.version, current);
            serde_json::json!({
                "ok": true,
                "current": current,
                "latest": latest.version,
                "hasUpdate": has_update,
                "notes": latest.notes,
                "url": latest.asset_url,
                "error": "",
            })
        }
        Err(e) => serde_json::json!({
            "ok": false,
            "current": current,
            "latest": "",
            "hasUpdate": false,
            "notes": "",
            "url": "",
            "error": format!("{e:#}"),
        }),
    };
    format!("window.__updateInfo({payload});")
}

/// 后台下载并自替换重启。成功时进程退出，不返回。
fn do_update(url: &str) -> Result<()> {
    let bytes = crate::update::download(url)?;
    crate::update::apply_update(&bytes)?;
    Ok(())
}

/// 把前端下拉值解析成窗口状态，未知/缺省时为普通窗口。
fn parse_state(s: Option<&str>) -> WindowState {
    match s {
        Some("maximized") => WindowState::Maximized,
        Some("fullscreen") => WindowState::Fullscreen,
        _ => WindowState::Normal,
    }
}

/// 由启动图路径读取字节并组装启动页配置。未选启动图返回 None。
fn build_splash(req: &PackReq) -> Result<Option<SplashCfg>> {
    let Some(path) = req
        .splash
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
    else {
        return Ok(None);
    };
    // 默认 1500ms；背景色默认深石板，与设计文档一致。
    let min_ms = req.splash_ms.unwrap_or(1500);
    let bg = req
        .splash_bg
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("#0f172a");
    // 复用 packer 的共享启动图构造逻辑（GUI/CLI 同款）。
    Ok(Some(packer::build_splash_cfg(&path, min_ms, bg)?))
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
        "index.html" => Some((
            include_bytes!("../ui/index.html"),
            "text/html; charset=utf-8",
        )),
        "style.css" => Some((include_bytes!("../ui/style.css"), "text/css; charset=utf-8")),
        "app.js" => Some((
            include_bytes!("../ui/app.js"),
            "text/javascript; charset=utf-8",
        )),
        "qrcode.jpg" => Some((include_bytes!("../ui/qrcode.jpg"), "image/jpeg")),
        _ => None,
    }
}
