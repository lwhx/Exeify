// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 不坑老师 · https://github.com/44886/Exeify
//
// 发布版隐藏控制台窗口；debug 版保留控制台便于观察日志。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod b64;
mod config;
mod crypto;
mod gui;
mod icon;
mod packer;
mod payload;
mod runner;
mod server;
mod update;

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

    // 清理上次自更新残留的 <名字>.old.exe（best-effort，忽略错误）。
    update::cleanup_old();

    // 隐藏 CLI：便于脚本化打包与自测（GUI 用户不会用到）。
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 2 {
        match args[1].as_str() {
            // 新版：命名参数 CLI，便于 AI/脚本驱动。
            "pack" => std::process::exit(run_pack_cli(&args)),
            // 旧版：位置式命令，保持向后兼容。
            "pack-local" | "pack-url" => std::process::exit(run_cli(&args)),
            _ => {}
        }
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
            // CLI 默认开启源码保护，与 GUI 默认一致。
            packer::pack_local(
                Path::new(&args[2]),
                entry,
                win,
                Path::new(&args[3]),
                icon,
                None,
                true,
            )
        }
        "pack-url" if args.len() >= 4 => {
            let icon = args.get(4).map(Path::new);
            packer::pack_url(&args[2], win, Path::new(&args[3]), icon, None, true)
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

/// 新版命名参数 CLI 的用法文案（中文，列全参数）。
const PACK_USAGE: &str = "\
Exeify 打包 —— 命名参数 CLI（供 AI/脚本驱动）

用法:
  exeify pack --local <目录> [--entry index.html] --out <app.exe> [选项...]
  exeify pack --url <网址>   --out <app.exe> [选项...]

源（二选一，必填其一）:
  --local <目录>          本地网页目录
  --url <网址>            在线网址（http:// 或 https:// 开头）

必填:
  --out <路径.exe>        输出的产物 exe，必须以 .exe 结尾

窗口选项:
  --entry <文件>          本地模式入口文件（默认 index.html，仅 --local 有效）
  --title <文字>          窗口标题（默认 App）
  --width <数字>          窗口宽度（默认 1024）
  --height <数字>         窗口高度（默认 720）
  --window <模式>         启动状态：normal | maximized | fullscreen（默认 normal）
  --no-resizable          禁止缩放窗口（默认允许缩放）

图标/启动图:
  --icon <路径>           窗口与 exe 图标（.ico / .png）
  --splash <图片>         启动图（.png / .jpg），消除初始白屏
  --splash-bg <#rrggbb>   启动图背景色（默认 #0f172a）
  --splash-sec <秒>       启动图最少显示秒数（默认 1.5）

源码保护:
  --no-protect            关闭源码保护（默认开启：加密内嵌资源 + 禁用查看）

其它:
  -h, --help              打印本用法

成功输出 `OK: <输出路径>`（返回码 0）；失败输出 `失败: <原因>` 到 stderr（返回码 1）；
用法错误返回码 2。";

/// 解析后的打包计划（纯数据，便于单测）。
#[derive(Debug, Clone, PartialEq)]
struct PackPlan {
    /// true=本地目录；false=URL。
    is_local: bool,
    /// 本地目录 或 URL 字符串。
    source: String,
    /// 输出 exe 路径。
    out: String,
    /// 本地入口文件（URL 模式忽略）。
    entry: String,
    title: String,
    width: f64,
    height: f64,
    state: config::WindowState,
    resizable: bool,
    /// 图标路径（可选）。
    icon: Option<String>,
    /// 启动图路径（可选）。给了才构造 SplashCfg。
    splash: Option<String>,
    /// 启动图背景色。
    splash_bg: String,
    /// 启动图最少显示毫秒 = round(--splash-sec × 1000)。
    splash_ms: u64,
    /// 源码保护开关（默认 true）。
    protect: bool,
}

/// 手写解析 `pack` 子命令的命名参数（不引入 clap，保持零依赖）。
/// `args` 为 `--local` 起的参数切片（即 `std::env::args()[2..]`）。
/// 返回 Ok(PackPlan) 或 Err(用法/校验错误信息)。
fn parse_pack_args(args: &[String]) -> Result<PackPlan, String> {
    let mut local: Option<String> = None;
    let mut url: Option<String> = None;
    let mut out: Option<String> = None;
    let mut entry: Option<String> = None;
    let mut title = "App".to_string();
    let mut width: f64 = 1024.0;
    let mut height: f64 = 720.0;
    let mut state = config::WindowState::Normal;
    let mut resizable = true;
    let mut icon: Option<String> = None;
    let mut splash: Option<String> = None;
    let mut splash_bg = "#0f172a".to_string();
    let mut splash_sec: f64 = 1.5;
    let mut protect = true;

    // 取下一个值型参数，缺失即报错。
    fn take<'a>(it: &mut std::slice::Iter<'a, String>, flag: &str) -> Result<&'a String, String> {
        it.next().ok_or_else(|| format!("参数 {flag} 缺少取值"))
    }

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--local" => local = Some(take(&mut it, "--local")?.clone()),
            "--url" => url = Some(take(&mut it, "--url")?.clone()),
            "--out" => out = Some(take(&mut it, "--out")?.clone()),
            "--entry" => entry = Some(take(&mut it, "--entry")?.clone()),
            "--title" => title = take(&mut it, "--title")?.clone(),
            "--width" => {
                let v = take(&mut it, "--width")?;
                width = v
                    .parse::<f64>()
                    .map_err(|_| format!("--width 需要数字，得到：{v}"))?;
            }
            "--height" => {
                let v = take(&mut it, "--height")?;
                height = v
                    .parse::<f64>()
                    .map_err(|_| format!("--height 需要数字，得到：{v}"))?;
            }
            "--window" => {
                let v = take(&mut it, "--window")?;
                state = match v.as_str() {
                    "normal" => config::WindowState::Normal,
                    "maximized" => config::WindowState::Maximized,
                    "fullscreen" => config::WindowState::Fullscreen,
                    other => {
                        return Err(format!(
                            "--window 取值非法：{other}（应为 normal|maximized|fullscreen）"
                        ))
                    }
                };
            }
            "--no-resizable" => resizable = false,
            "--icon" => icon = Some(take(&mut it, "--icon")?.clone()),
            "--splash" => splash = Some(take(&mut it, "--splash")?.clone()),
            "--splash-bg" => splash_bg = take(&mut it, "--splash-bg")?.clone(),
            "--splash-sec" => {
                let v = take(&mut it, "--splash-sec")?;
                splash_sec = v
                    .parse::<f64>()
                    .map_err(|_| format!("--splash-sec 需要数字，得到：{v}"))?;
            }
            "--no-protect" => protect = false,
            other => return Err(format!("未知参数：{other}")),
        }
    }

    // 源必须恰好给一个。
    let (is_local, source) = match (local, url) {
        (Some(_), Some(_)) => return Err("--local 与 --url 只能二选一，不能同时给".to_string()),
        (Some(dir), None) => (true, dir),
        (None, Some(u)) => (false, u),
        (None, None) => return Err("必须提供 --local <目录> 或 --url <网址> 之一".to_string()),
    };

    let out = out.ok_or_else(|| "缺少 --out <路径.exe>".to_string())?;

    // 启动秒数转毫秒（四舍五入）。负数按 0 处理。
    let splash_ms = (splash_sec.max(0.0) * 1000.0).round() as u64;

    Ok(PackPlan {
        is_local,
        source,
        out,
        entry: entry.unwrap_or_else(|| "index.html".to_string()),
        title,
        width,
        height,
        state,
        resizable,
        icon,
        splash,
        splash_bg,
        splash_ms,
        protect,
    })
}

/// 新版 `pack` 子命令入口。`args` 为完整 `std::env::args()`。
/// 返回进程退出码：0 成功、1 打包失败、2 用法错误。
fn run_pack_cli(args: &[String]) -> i32 {
    use config::WindowCfg;
    use std::path::Path;

    // `pack --help` / `-h`：打印用法，退出码 0。
    let rest = &args[2..];
    if rest.iter().any(|a| a == "--help" || a == "-h") {
        println!("{PACK_USAGE}");
        return 0;
    }

    let plan = match parse_pack_args(rest) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("失败: {e}\n\n{PACK_USAGE}");
            return 2;
        }
    };

    // 校验输出必须以 .exe 结尾（提前给出清晰用法错误，退出码 2）。
    if Path::new(&plan.out)
        .extension()
        .map(|e| e.to_ascii_lowercase())
        != Some("exe".into())
    {
        eprintln!("失败: --out 必须以 .exe 结尾\n\n{PACK_USAGE}");
        return 2;
    }

    // 给了 --splash 才构造 SplashCfg（复用 packer 共享逻辑）。
    let splash = match plan.splash.as_deref() {
        Some(path) => {
            match packer::build_splash_cfg(Path::new(path), plan.splash_ms, &plan.splash_bg) {
                Ok(s) => Some(s),
                Err(e) => {
                    eprintln!("失败: {e:#}");
                    return 1;
                }
            }
        }
        None => None,
    };

    let window = WindowCfg {
        title: plan.title.clone(),
        width: plan.width,
        height: plan.height,
        resizable: plan.resizable,
        state: plan.state,
    };
    let icon = plan.icon.as_deref().map(Path::new);

    let result = if plan.is_local {
        packer::pack_local(
            Path::new(&plan.source),
            &plan.entry,
            window,
            Path::new(&plan.out),
            icon,
            splash,
            plan.protect,
        )
    } else {
        packer::pack_url(
            &plan.source,
            window,
            Path::new(&plan.out),
            icon,
            splash,
            plan.protect,
        )
    };

    match result {
        Ok(()) => {
            println!("OK: {}", plan.out);
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

#[cfg(test)]
mod tests {
    use super::*;
    use config::WindowState;

    /// 便捷：把 &str 数组转成 Vec<String>（模拟 args[2..]）。
    fn v(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_local_minimal_with_defaults() {
        let plan = parse_pack_args(&v(&["--local", "site", "--out", "app.exe"])).unwrap();
        assert!(plan.is_local);
        assert_eq!(plan.source, "site");
        assert_eq!(plan.out, "app.exe");
        assert_eq!(plan.entry, "index.html");
        assert_eq!(plan.title, "App");
        assert_eq!(plan.width, 1024.0);
        assert_eq!(plan.height, 720.0);
        assert_eq!(plan.state, WindowState::Normal);
        assert!(plan.resizable);
        assert!(plan.icon.is_none());
        assert!(plan.splash.is_none());
        assert_eq!(plan.splash_bg, "#0f172a");
        assert_eq!(plan.splash_ms, 1500); // 默认 1.5s
        assert!(plan.protect);
    }

    #[test]
    fn parses_url_minimal() {
        let plan =
            parse_pack_args(&v(&["--url", "https://example.com", "--out", "out.exe"])).unwrap();
        assert!(!plan.is_local);
        assert_eq!(plan.source, "https://example.com");
        assert_eq!(plan.out, "out.exe");
    }

    #[test]
    fn parses_full_local_options() {
        let plan = parse_pack_args(&v(&[
            "--local",
            "dir",
            "--entry",
            "home.html",
            "--out",
            "app.exe",
            "--title",
            "我的应用",
            "--width",
            "800",
            "--height",
            "600",
            "--window",
            "fullscreen",
            "--no-resizable",
            "--icon",
            "logo.png",
            "--splash",
            "s.png",
            "--splash-bg",
            "#123456",
            "--splash-sec",
            "2",
            "--no-protect",
        ]))
        .unwrap();
        assert_eq!(plan.entry, "home.html");
        assert_eq!(plan.title, "我的应用");
        assert_eq!(plan.width, 800.0);
        assert_eq!(plan.height, 600.0);
        assert_eq!(plan.state, WindowState::Fullscreen);
        assert!(!plan.resizable);
        assert_eq!(plan.icon.as_deref(), Some("logo.png"));
        assert_eq!(plan.splash.as_deref(), Some("s.png"));
        assert_eq!(plan.splash_bg, "#123456");
        assert_eq!(plan.splash_ms, 2000);
        assert!(!plan.protect);
    }

    #[test]
    fn window_maximized_parses() {
        let plan = parse_pack_args(&v(&[
            "--url",
            "https://x.com",
            "--out",
            "a.exe",
            "--window",
            "maximized",
        ]))
        .unwrap();
        assert_eq!(plan.state, WindowState::Maximized);
    }

    #[test]
    fn rejects_invalid_window_value() {
        let err = parse_pack_args(&v(&["--local", "d", "--out", "a.exe", "--window", "weird"]))
            .unwrap_err();
        assert!(err.contains("--window"), "错误信息应提到 --window: {err}");
    }

    #[test]
    fn rejects_missing_source() {
        let err = parse_pack_args(&v(&["--out", "a.exe"])).unwrap_err();
        assert!(err.contains("--local") || err.contains("--url"), "{err}");
    }

    #[test]
    fn rejects_missing_out() {
        let err = parse_pack_args(&v(&["--local", "d"])).unwrap_err();
        assert!(err.contains("--out"), "{err}");
    }

    #[test]
    fn rejects_both_local_and_url() {
        let err = parse_pack_args(&v(&[
            "--local",
            "d",
            "--url",
            "https://x.com",
            "--out",
            "a.exe",
        ]))
        .unwrap_err();
        assert!(err.contains("二选一") || err.contains("同时"), "{err}");
    }

    #[test]
    fn rejects_non_numeric_width() {
        let err =
            parse_pack_args(&v(&["--local", "d", "--out", "a.exe", "--width", "big"])).unwrap_err();
        assert!(err.contains("--width"), "{err}");
    }

    #[test]
    fn rejects_non_numeric_splash_sec() {
        let err = parse_pack_args(&v(&[
            "--local",
            "d",
            "--out",
            "a.exe",
            "--splash-sec",
            "soon",
        ]))
        .unwrap_err();
        assert!(err.contains("--splash-sec"), "{err}");
    }

    #[test]
    fn splash_sec_rounds_to_ms() {
        let plan = parse_pack_args(&v(&[
            "--url",
            "https://x.com",
            "--out",
            "a.exe",
            "--splash-sec",
            "1.234",
        ]))
        .unwrap();
        assert_eq!(plan.splash_ms, 1234);
    }

    #[test]
    fn rejects_flag_missing_value() {
        let err = parse_pack_args(&v(&["--local", "d", "--out"])).unwrap_err();
        assert!(err.contains("--out"), "{err}");
    }

    #[test]
    fn rejects_unknown_flag() {
        let err = parse_pack_args(&v(&["--local", "d", "--out", "a.exe", "--bogus"])).unwrap_err();
        assert!(err.contains("未知参数"), "{err}");
    }
}
