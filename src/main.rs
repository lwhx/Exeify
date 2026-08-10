// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 不坑老师 · https://github.com/44886/Exeify
//
// 发布版隐藏控制台窗口；debug 版保留控制台便于观察日志。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod gui;
mod icon;
mod packer;
mod payload;
mod runner;
mod server;

/// 自定义协议 `app` 的起始地址。
/// Windows/WebView2 下自定义协议被映射为 `http://<scheme>.localhost/`，
/// 其它平台使用 `app://localhost/`。页面内一律用相对路径引用资源。
pub fn asset_base_url() -> &'static str {
    #[cfg(windows)]
    {
        "http://app.localhost/"
    }
    #[cfg(not(windows))]
    {
        "app://localhost/"
    }
}

/// 应用窗口图标（来自内嵌的 `assets/icon.png`），用于任务栏与标题栏。
/// tao/winit 默认不会用 exe 的 PE 图标，必须运行时显式设置。
pub fn app_window_icon() -> Option<tao::window::Icon> {
    const PNG: &[u8] = include_bytes!("../assets/icon.png");
    let (w, h, rgba) = icon::decode_png_rgba_bytes(PNG).ok()?;
    tao::window::Icon::from_rgba(rgba, w, h).ok()
}

fn main() {
    // 把 WebView2 的用户数据目录重定向到 %LOCALAPPDATA%，
    // 避免在 exe 旁边生成 <名字>.exe.WebView2 缓存目录。
    redirect_webview_data_dir();

    // 隐藏 CLI：便于脚本化打包与自测（GUI 用户不会用到）。
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 2 && matches!(args[1].as_str(), "pack-local" | "pack-url") {
        std::process::exit(run_cli(&args));
    }

    match payload::read_from_current_exe() {
        // 尾部带载荷 -> runner 模式
        Ok(Some(p)) => {
            if let Err(e) = runner::run(p) {
                fatal(&format!("启动失败：{e:#}"));
            }
        }
        // 无载荷 -> GUI 打包器模式
        Ok(None) => {
            if let Err(e) = gui::run() {
                fatal(&format!("界面启动失败：{e:#}"));
            }
        }
        Err(e) => {
            // 读取载荷出错也退回 GUI，避免完全打不开
            eprintln!("读取载荷出错：{e:#}，退回打包器界面");
            if let Err(e) = gui::run() {
                fatal(&format!("界面启动失败：{e:#}"));
            }
        }
    }
}

/// 将 WebView2 用户数据目录设置到 `%LOCALAPPDATA%\exeify\<exe名>`。
/// 若用户已通过环境变量指定，则尊重其设置。
fn redirect_webview_data_dir() {
    if std::env::var_os("WEBVIEW2_USER_DATA_FOLDER").is_some() {
        return;
    }
    let Some(base) = std::env::var_os("LOCALAPPDATA") else {
        return;
    };
    let stem = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "app".to_string());
    let safe: String = stem
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = if safe.is_empty() {
        "app".to_string()
    } else {
        safe
    };
    let dir = std::path::PathBuf::from(base).join("exeify").join(safe);
    if std::fs::create_dir_all(&dir).is_ok() {
        std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &dir);
    }
}

/// 隐藏 CLI 入口。用法：
///   exeify pack-local <目录> <输出.exe> [入口=index.html]
///   exeify pack-url   <网址> <输出.exe>
fn run_cli(args: &[String]) -> i32 {
    use config::WindowCfg;
    use std::path::Path;
    let win = WindowCfg::default();
    let result = match args[1].as_str() {
        "pack-local" if args.len() >= 4 => {
            let entry = args.get(4).map(|s| s.as_str()).unwrap_or("index.html");
            let icon = args.get(5).map(Path::new);
            packer::pack_local(Path::new(&args[2]), entry, win, Path::new(&args[3]), icon)
        }
        "pack-url" if args.len() >= 4 => {
            let icon = args.get(4).map(Path::new);
            packer::pack_url(&args[2], win, Path::new(&args[3]), icon)
        }
        _ => {
            eprintln!("用法:\n  exeify pack-local <目录> <输出.exe> [入口]\n  exeify pack-url <网址> <输出.exe>");
            return 2;
        }
    };
    match result {
        Ok(()) => {
            println!("OK: {}", args[3]);
            0
        }
        Err(e) => {
            eprintln!("失败: {e:#}");
            1
        }
    }
}

fn fatal(msg: &str) {
    eprintln!("{msg}");
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("Exeify")
        .set_description(msg)
        .show();
}
