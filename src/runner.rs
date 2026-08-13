// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 不坑老师 · https://github.com/44886/Exeify

//! runner 模式：读取自身载荷并用 WebView 显示。
//!
//! 本地模式通过内置的回环 HTTP 服务器（`http://127.0.0.1`）提供网页资源，
//! 以便 Vite/Vue/React 等框架的 ES Module 与前端路由正常工作。

use crate::config::{Mode, PackConfig, SplashCfg, WindowIcon, WindowState};
use crate::payload::Payload;
use crate::server;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::Read;
use std::time::{Duration, Instant};
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::{Fullscreen, WindowBuilder},
};
use wry::{NewWindowResponse, PageLoadEvent, WebViewBuilder};

/// 事件循环自定义事件。
enum UserEvent {
    /// 启动页加载完成，请求导航到真实目标（携带触发时刻，用于满足最少显示时长）。
    SplashLoaded,
    /// 页面请求打开新窗口（target="_blank" / window.open），携带目标 URL。
    /// 由事件循环在本应用内部新建 tao 窗口 + WebView 承接，绝不调用系统浏览器。
    OpenWindow(String),
}

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

/// 把 "#rrggbb" 解析为 wry 的 RGBA（不透明）。解析失败时返回 None。
fn parse_hex_rgba(hex: &str) -> Option<(u8, u8, u8, u8)> {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&h[0..2], 16).ok()?;
    let g = u8::from_str_radix(&h[2..4], 16).ok()?;
    let b = u8::from_str_radix(&h[4..6], 16).ok()?;
    Some((r, g, b, 255))
}

/// 用 config 里的用户图标构造窗口图标；没有或解码失败则回退内置图标。
fn window_icon(cfg: &Option<WindowIcon>) -> Option<tao::window::Icon> {
    if let Some(ic) = cfg {
        if let Some(rgba) = crate::b64::decode(&ic.rgba_b64) {
            if rgba.len() as u32 == ic.w * ic.h * 4 {
                if let Ok(icon) = tao::window::Icon::from_rgba(rgba, ic.w, ic.h) {
                    return Some(icon);
                }
            }
        }
    }
    crate::app_window_icon()
}

/// 生成启动页纯 HTML（供 WebViewBuilder::with_html 使用），图片铺满窗口、居中裁剪，配背景色兜底。
///
/// 注意：不要再拼 `data:text/html;charset=utf-8,` 前缀。data-URL 中 `#` 是 fragment 分隔符，
/// 会在 CSS 的 `background:#0f172a` 处截断整段 HTML，导致 `<img>` 丢失、启动图渲染不出来。
/// 用 with_html 直接注入本体即可，`<img>` 的 data: URI 在页面内可正常渲染。
fn splash_html(splash: &SplashCfg) -> String {
    let bg = if parse_hex_rgba(&splash.bg).is_some() {
        splash.bg.clone()
    } else {
        "#0f172a".to_string()
    };
    let fit = if splash.fit.is_empty() {
        "cover"
    } else {
        splash.fit.as_str()
    };
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\">\
<style>html,body{{margin:0;height:100%;background:{bg};overflow:hidden}}\
img{{position:fixed;inset:0;width:100%;height:100%;object-fit:{fit};object-position:center}}</style>\
</head><body><img src=\"data:{mime};base64,{data}\"></body></html>",
        bg = bg,
        fit = fit,
        mime = splash.mime,
        data = splash.image_b64,
    )
}

pub fn run(payload: Payload) -> Result<()> {
    let PackConfig {
        mode,
        url,
        entry,
        window,
        splash,
        window_icon: window_icon_cfg,
    } = payload.config.clone();

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
            // 导航到入口所在目录（根 index.html => "/"），而非 /index.html，
            // 以便前端路由（Vue Router 等 history 模式）能匹配到首页路由。
            let base = match entry.trim_start_matches('/').rsplit_once('/') {
                Some((dir, _)) => format!("/{dir}/"),
                None => "/".to_string(),
            };
            format!("http://127.0.0.1:{port}{base}")
        }
    };

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();

    // 窗口按启动状态构建：Normal 用宽高，Maximized 最大化，Fullscreen 无边框铺满。
    let mut builder = WindowBuilder::new()
        .with_title(&window.title)
        .with_resizable(window.resizable)
        .with_window_icon(window_icon(&window_icon_cfg));
    builder = match window.state {
        WindowState::Normal => {
            builder.with_inner_size(LogicalSize::new(window.width, window.height))
        }
        WindowState::Maximized => builder
            .with_inner_size(LogicalSize::new(window.width, window.height))
            .with_maximized(true),
        WindowState::Fullscreen => builder
            .with_fullscreen(Some(Fullscreen::Borderless(None)))
            .with_decorations(false),
    };
    let win = builder.build(&event_loop).context("创建窗口失败")?;

    // 背景色：启动图配了就用其背景，消除 WebView 初始化白屏。
    let bg = splash.as_ref().and_then(|s| parse_hex_rgba(&s.bg));

    // 有启动图：先加载启动页 data-URL，页面加载完成 + 满足最少时长后再导航到目标。
    // 无启动图：直接加载目标，行为与旧版一致。
    let mut wv_builder = WebViewBuilder::new();
    if let Some(rgba) = bg {
        wv_builder = wv_builder.with_background_color(rgba);
    }

    let has_splash = splash.is_some();
    if let Some(sp) = splash.as_ref() {
        let proxy = event_loop.create_proxy();
        wv_builder = wv_builder
            .with_html(splash_html(sp))
            .with_on_page_load_handler(move |event, _url| {
                if matches!(event, PageLoadEvent::Finished) {
                    let _ = proxy.send_event(UserEvent::SplashLoaded);
                }
            });
    } else {
        wv_builder = wv_builder.with_url(&target_url);
    }

    // 无条件挂新窗口处理器：页面里 target="_blank" / window.open 触发的新窗口请求，
    // 统一发事件到主循环，由本应用内部新建窗口承接（Deny 拒绝 WebView2 默认行为，绝不走系统浏览器）。
    let nw_proxy = event_loop.create_proxy();
    wv_builder = wv_builder.with_new_window_req_handler(move |url, _features| {
        let _ = nw_proxy.send_event(UserEvent::OpenWindow(url));
        NewWindowResponse::Deny
    });

    let webview = wv_builder.build(&win).context("创建 WebView 失败")?;

    // 启动页最少显示时长的计时起点（窗口出现即开始计时）。
    let started = Instant::now();
    let min_ms = splash.as_ref().map(|s| s.min_ms).unwrap_or(0);
    let deadline_min = started + Duration::from_millis(min_ms);
    // 兜底：即使启动页 Finished 事件不来，最多等 min_ms + 6s 也导航，避免卡死。
    let deadline_max = started + Duration::from_millis(min_ms.saturating_add(6000));
    // 记录启动页是否就绪、是否已切到目标（避免多次 Finished 重复导航）。
    let mut splash_loaded = false;
    let mut navigated = false;

    // 多窗口管理：主窗口 id 用于区分关闭事件；children 持有子窗口的 Window+WebView 保活
    // （必须存进 HashMap，否则一建即被 drop 立即关闭）。
    let main_id = win.id();
    let mut children: HashMap<tao::window::WindowId, (tao::window::Window, wry::WebView)> =
        HashMap::new();
    // 子窗口自身的新窗口处理器复用此 proxy，实现子窗口里再点 _blank 也能继续弹窗。
    let child_proxy = event_loop.create_proxy();
    // 子窗口图标复用主窗口图标配置（clone 一份 move 进闭包，避免与主窗口构建争用所有权）。
    let icon_cfg_for_children = window_icon_cfg.clone();
    // 子窗口默认沿用应用标题与主窗口宽高。
    let child_title = window.title.clone();
    let child_w = window.width;
    let child_h = window.height;

    event_loop.run(move |event, event_loop_target, control_flow| {
        match event {
            Event::WindowEvent {
                window_id,
                event: WindowEvent::CloseRequested,
                ..
            } => {
                if window_id == main_id {
                    // 主窗口关闭：直接退出事件循环（children 随闭包一起析构，子窗口不会残留）。
                    *control_flow = ControlFlow::Exit;
                    return;
                }
                // 子窗口关闭：移除即销毁该子窗口的 Window+WebView。
                children.remove(&window_id);
                *control_flow = ControlFlow::Wait;
                return;
            }
            Event::UserEvent(UserEvent::SplashLoaded) => {
                splash_loaded = true;
            }
            Event::UserEvent(UserEvent::OpenWindow(url)) => {
                // 在本应用内部新建窗口 + WebView 承接链接，绝不调用系统浏览器。
                let cp = child_proxy.clone();
                if let Ok(child_win) = WindowBuilder::new()
                    .with_title(&child_title)
                    .with_inner_size(LogicalSize::new(child_w, child_h))
                    .with_resizable(true)
                    .with_window_icon(window_icon(&icon_cfg_for_children))
                    .build(event_loop_target)
                {
                    let built = WebViewBuilder::new()
                        .with_url(&url)
                        .with_new_window_req_handler(move |u, _f| {
                            let _ = cp.send_event(UserEvent::OpenWindow(u));
                            NewWindowResponse::Deny
                        })
                        .build(&child_win);
                    if let Ok(child_wv) = built {
                        // 存进 HashMap 保活，避免 Window/WebView 被立即 drop。
                        children.insert(child_win.id(), (child_win, child_wv));
                    }
                }
                *control_flow = ControlFlow::Wait;
                return;
            }
            _ => {}
        }
        if has_splash && !navigated {
            let now = Instant::now();
            // 启动页已就绪且过了最少时长，或已到兜底时限 -> 切到目标。
            let ready = (splash_loaded && now >= deadline_min) || now >= deadline_max;
            if ready {
                navigated = true;
                let _ = webview.load_url(&target_url);
                *control_flow = ControlFlow::Wait;
            } else {
                // 未就绪时按相关时限唤醒；SplashLoaded 事件到达本身也会唤醒。
                let next = if splash_loaded {
                    deadline_min
                } else {
                    deadline_max
                };
                *control_flow = ControlFlow::WaitUntil(next);
            }
        } else {
            // 无启动图或已导航：只处理关闭，静候事件。
            *control_flow = ControlFlow::Wait;
        }
    });
}
