// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 不坑老师 · https://github.com/44886/exeify

//! 本地回环 HTTP 服务器：把内存里的网页资源用真正的 `http://127.0.0.1` 源提供，
//! 使 Vite/Vue/React 等框架的 ES Module、路由、fetch 都能正常工作
//! （自定义协议 `app://` 无法执行 module 脚本）。

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

/// 启动服务器，返回监听端口。仅绑定回环地址，不对外暴露。
pub fn start(assets: HashMap<String, Vec<u8>>, entry: String) -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("无法启动本地服务器")?;
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
