// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 不坑老师 · https://github.com/44886/Exeify

//! 本地回环 HTTP 服务器：把内存里的网页资源用真正的 `http://127.0.0.1` 源提供，
//! 使 Vite/Vue/React 等框架的 ES Module、路由、fetch 都能正常工作
//! （自定义协议 `app://` 无法执行 module 脚本）。

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

/// 根据种子字符串派生一个确定性端口（纯函数）。
///
/// 为什么不用 std 的 `DefaultHasher`/`RandomState`：它们的哈希种子每个进程随机，
/// 同一输入在不同次启动会得到不同哈希 -> 端口漂移 -> localStorage/IndexedDB 的
/// "源(含端口)"隔离读不到旧数据。这里手写 **FNV-1a**（无随机种子、跨进程稳定），
/// 保证同一产物每次启动派生出同一端口，从而源稳定、数据持久。
///
/// 结果映射到 `[10000, 60000)`，避开特权端口(<1024)与常见系统端口区间。
fn stable_port(seed: &str) -> u16 {
    // FNV-1a 64 位：offset basis 与 prime 为标准常量。
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    for &byte in seed.as_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    // 映射到 [10000, 60000)：区间宽度 50000。
    (10000 + (hash % 50000)) as u16
}

/// 取当前 exe 文件名(stem)作为端口派生种子；取不到时用固定种子兜底。
fn port_seed() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "exeify".to_string())
}

/// 启动服务器，返回监听端口。仅绑定回环地址，不对外暴露。
///
/// 端口策略（Bug 2 修复）：用当前 exe 文件名派生确定性端口 `p`，依次尝试
/// 绑定 `[p, p+1, …, p+9]`（确定性退避，偶发占用时仍尽量落在稳定端口）；
/// 全部失败才回退 `127.0.0.1:0`（随机端口，该次源会变、数据不持久，属极端兜底）。
pub fn start(assets: HashMap<String, Vec<u8>>, entry: String) -> Result<u16> {
    let base = stable_port(&port_seed());
    let listener = bind_deterministic(base).context("无法启动本地服务器")?;
    let port = listener.local_addr()?.port();
    let assets = Arc::new(assets);
    let entry = Arc::new(entry);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let assets = assets.clone();
            let entry = entry.clone();
            std::thread::spawn(move || {
                let _ = handle(stream, &assets, &entry);
            });
        }
    });
    Ok(port)
}

/// 按序尝试绑定 `[base, base+1, …, base+9]`；`base+k` 可能越过 u16 上限，
/// 用 `saturating_add` 收敛到 65535（重复尝试同一端口无副作用）。全部失败
/// 才回退随机端口 `127.0.0.1:0`。返回成功绑定的监听器。
fn bind_deterministic(base: u16) -> std::io::Result<TcpListener> {
    for k in 0..10u16 {
        let port = base.saturating_add(k);
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)) {
            return Ok(listener);
        }
    }
    // 极端兜底：确定性端口全被占用，退回随机端口（该次不持久）。
    TcpListener::bind("127.0.0.1:0")
}

fn handle(
    stream: TcpStream,
    assets: &HashMap<String, Vec<u8>>,
    entry: &str,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }
    // 读掉剩余请求头
    loop {
        let mut h = String::new();
        let n = reader.read_line(&mut h)?;
        if n == 0 || h == "\r\n" || h == "\n" {
            break;
        }
    }

    let raw_path = request_line.split_whitespace().nth(1).unwrap_or("/");
    let raw_path = raw_path.split(['?', '#']).next().unwrap_or("/");
    let decoded = percent_decode(raw_path);
    let mut key = decoded.trim_start_matches('/').to_string();
    if key.is_empty() {
        key = entry.to_string();
    }

    let (status, body, mime): (&str, Vec<u8>, &str) = match assets.get(&key) {
        Some(b) => ("200 OK", b.clone(), guess_mime(&key)),
        None => {
            // SPA 回退：无扩展名的路径（前端路由）返回入口页
            if !std::path::Path::new(&key)
                .file_name()
                .map(|n| n.to_string_lossy().contains('.'))
                .unwrap_or(false)
            {
                match assets.get(entry) {
                    Some(b) => ("200 OK", b.clone(), guess_mime(entry)),
                    None => ("404 Not Found", b"404".to_vec(), "text/plain"),
                }
            } else {
                (
                    "404 Not Found",
                    format!("404 未找到：{key}").into_bytes(),
                    "text/plain; charset=utf-8",
                )
            }
        }
    };

    log_request(raw_path, &key, status);

    let mut out = stream;
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
        body.len()
    );
    out.write_all(header.as_bytes())?;
    out.write_all(&body)?;
    out.flush()
}

fn log_request(path: &str, key: &str, status: &str) {
    if let Ok(log) = std::env::var("EXEIFY_LOG") {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
        {
            let _ = writeln!(f, "{path} {key} -> {status}");
        }
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn guess_mime(path: &str) -> &'static str {
    match path
        .rsplit('.')
        .next()
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
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
        Some("map") => "application/json",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_port_is_deterministic() {
        // 同一输入必须每次得到同一端口（这正是数据持久的根本保证）。
        assert_eq!(stable_port("MyApp"), stable_port("MyApp"));
        assert_eq!(stable_port("exeify"), stable_port("exeify"));
    }

    #[test]
    fn stable_port_in_range() {
        // 结果必须落在 [10000, 60000)：避开特权端口，且不越 u16 上限。
        let long = "x".repeat(500);
        let seeds = ["", "a", "MyApp", "很长的中文名字带 emoji 🚀", long.as_str()];
        for seed in seeds {
            let p = stable_port(seed);
            assert!(
                (10000..60000).contains(&p),
                "seed {seed:?} -> port {p} 越界"
            );
        }
    }

    #[test]
    fn stable_port_differs_for_different_seeds() {
        // 不同输入大概率映射到不同端口（弱碰撞即可，非密码学要求）。
        let seeds = ["app-a", "app-b", "app-c", "index", "MyApp", "installer"];
        let ports: std::collections::HashSet<u16> = seeds.iter().map(|s| stable_port(s)).collect();
        // 6 个不同种子在 5 万端口空间里碰撞概率极低，要求至少 5 个互异。
        assert!(ports.len() >= 5, "碰撞过多：{ports:?}");
    }
}
