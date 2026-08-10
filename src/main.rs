// 发布版隐藏控制台窗口；debug 版保留控制台便于观察日志。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod gui;
mod packer;
mod payload;
mod runner;

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

fn main() {
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

/// 隐藏 CLI 入口。用法：
///   html2exe pack-local <目录> <输出.exe> [入口=index.html]
///   html2exe pack-url   <网址> <输出.exe>
fn run_cli(args: &[String]) -> i32 {
    use config::WindowCfg;
    use std::path::Path;
    let win = WindowCfg::default();
    let result = match args[1].as_str() {
        "pack-local" if args.len() >= 4 => {
            let entry = args.get(4).map(|s| s.as_str()).unwrap_or("index.html");
            packer::pack_local(Path::new(&args[2]), entry, win, Path::new(&args[3]))
        }
        "pack-url" if args.len() >= 4 => {
            packer::pack_url(&args[2], win, Path::new(&args[3]))
        }
        _ => {
            eprintln!("用法:\n  html2exe pack-local <目录> <输出.exe> [入口]\n  html2exe pack-url <网址> <输出.exe>");
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
        .set_title("html2exe")
        .set_description(msg)
        .show();
}
