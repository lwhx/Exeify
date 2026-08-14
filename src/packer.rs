// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 不坑老师 · https://github.com/44886/Exeify

//! 打包逻辑：把目录/URL 生成产物 exe。

use crate::config::{EncInfo, Mode, PackConfig, SplashCfg, WindowCfg, WindowIcon};
use crate::crypto;
use crate::payload;
use anyhow::{anyhow, bail, Context, Result};
use std::io::Write;
use std::path::Path;

/// 把本地目录打成 zip（deflate），返回 zip 字节。
pub fn zip_dir(dir: &Path) -> Result<Vec<u8>> {
    if !dir.is_dir() {
        bail!("不是有效目录：{}", dir.display());
    }
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(buf);
    let opts: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let mut count = 0usize;
    for entry in walkdir(dir)? {
        let rel = entry
            .strip_prefix(dir)
            .context("计算相对路径失败")?
            .to_string_lossy()
            .replace('\\', "/");
        if rel.is_empty() {
            continue;
        }
        if entry.is_dir() {
            zip.add_directory(format!("{rel}/"), opts)
                .context("写入目录项失败")?;
        } else {
            let data = std::fs::read(&entry)
                .with_context(|| format!("读取文件失败：{}", entry.display()))?;
            zip.start_file(&rel, opts).context("创建 zip 条目失败")?;
            zip.write_all(&data).context("写入 zip 条目失败")?;
            count += 1;
        }
    }
    if count == 0 {
        bail!("目录里没有任何文件");
    }
    let cursor = zip.finish().context("完成 zip 失败")?;
    Ok(cursor.into_inner())
}

/// 扫描目录识别可用的网页入口（.html/.htm），返回相对路径列表。
/// 排序：根目录 index 优先、浅层优先、字母序。第一个即最佳默认入口。
pub fn detect_html_entries(dir: &Path) -> Vec<String> {
    let mut list: Vec<(bool, usize, String)> = Vec::new(); // (非index, 深度, 相对路径)
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((p, depth)) = stack.pop() {
        if depth > 4 {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&p) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                    if name.starts_with('.') || name == "node_modules" {
                        continue;
                    }
                }
                stack.push((path, depth + 1));
            } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                let ext = ext.to_ascii_lowercase();
                if ext == "html" || ext == "htm" {
                    if let Ok(rel) = path.strip_prefix(dir) {
                        let rel = rel.to_string_lossy().replace('\\', "/");
                        let is_index = rel.eq_ignore_ascii_case("index.html")
                            || rel.eq_ignore_ascii_case("index.htm");
                        list.push((!is_index, depth, rel));
                    }
                }
            }
        }
    }
    list.sort();
    list.into_iter().map(|(_, _, r)| r).collect()
}

/// 简单递归遍历（避免额外依赖 walkdir crate）。
fn walkdir(root: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(p) = stack.pop() {
        for entry in
            std::fs::read_dir(&p).with_context(|| format!("读取目录失败：{}", p.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                out.push(path.clone());
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    Ok(out)
}

/// 打包本地目录 -> 产物 exe。
/// `icon` 为可选的 .ico/.png 图标文件（同时用于 PE 资源图标 + 运行时窗口图标）；
/// `splash` 为可选启动页配置。
pub fn pack_local(
    dir: &Path,
    entry: &str,
    window: WindowCfg,
    output: &Path,
    icon: Option<&Path>,
    splash: Option<SplashCfg>,
    protect: bool,
) -> Result<()> {
    // 入口文件必须存在
    let entry_path = dir.join(entry);
    if !entry_path.is_file() {
        bail!("入口文件不存在：{}", entry_path.display());
    }
    let archive = zip_dir(dir)?;
    // 源码保护开启：加密 archive 并记录遮蔽后的 key/nonce；否则明文。
    let (archive, enc) = maybe_encrypt(archive, protect);
    let config = PackConfig {
        mode: Mode::Local,
        url: None,
        entry: entry.to_string(),
        window,
        splash,
        window_icon: window_icon_from(icon),
        enc,
        protect,
    };
    write_output(&config, &archive, output, icon)
}

/// 按 `protect` 决定是否加密 archive。
/// 返回 (可能加密后的 archive 字节, 加密信息)。protect=false 时逐字节维持原样、enc=None。
fn maybe_encrypt(archive: Vec<u8>, protect: bool) -> (Vec<u8>, Option<EncInfo>) {
    if !protect || archive.is_empty() {
        return (archive, None);
    }
    let key = crypto::random_key();
    let nonce = crypto::random_nonce();
    let cipher = crypto::encrypt(&key, &nonce, &archive);
    let enc = EncInfo {
        key_b64: crypto::mask_key_b64(&key),
        nonce_b64: crypto::mask_nonce_b64(&nonce),
    };
    (cipher, Some(enc))
}

/// 打包 URL -> 产物 exe。参数含义同 [`pack_local`]。
pub fn pack_url(
    url: &str,
    window: WindowCfg,
    output: &Path,
    icon: Option<&Path>,
    splash: Option<SplashCfg>,
    protect: bool,
) -> Result<()> {
    let url = url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        bail!("网址必须以 http:// 或 https:// 开头");
    }
    // URL 模式无内嵌 archive，无需加密；protect 仅影响运行时禁用查看。
    let config = PackConfig {
        mode: Mode::Url,
        url: Some(url.to_string()),
        entry: String::new(),
        window,
        splash,
        window_icon: window_icon_from(icon),
        enc: None,
        protect,
    };
    write_output(&config, &[], output, icon)
}

/// 把用户图标解码成 256×256 RGBA，供运行时设为窗口/任务栏图标。
/// 解码失败或未选图标时返回 None（运行时回退内置图标）。
fn window_icon_from(icon: Option<&Path>) -> Option<WindowIcon> {
    let path = icon?;
    let (w, h, rgba) = crate::icon::decode_to_rgba(path).ok()?;
    if w == 0 || h == 0 {
        return None;
    }
    const SIDE: u32 = 256;
    let rgba = crate::icon::resize_rgba_to(&rgba, w, h, SIDE, SIDE);
    Some(WindowIcon {
        w: SIDE,
        h: SIDE,
        rgba_b64: crate::b64::encode(&rgba),
    })
}

/// 把图标写进 stub 的 PE 资源，返回新的 stub 字节。
fn patch_icon(stub: Vec<u8>, icon_path: &Path) -> Result<Vec<u8>> {
    let ico_bytes = crate::icon::to_ico_bytes(icon_path)?;
    let mut image = editpe::Image::parse(stub).map_err(|e| anyhow!("解析 exe 失败：{e}"))?;
    let mut res = image.resource_directory().cloned().unwrap_or_default();
    res.set_main_icon(ico_bytes.as_slice())
        .map_err(|e| anyhow!("写入图标资源失败：{e}"))?;
    image
        .set_resource_directory(res)
        .map_err(|e| anyhow!("应用图标资源失败：{e}"))?;
    Ok(image.data().to_vec())
}

fn write_output(
    config: &PackConfig,
    archive: &[u8],
    output: &Path,
    icon: Option<&Path>,
) -> Result<()> {
    let mut stub = payload::clean_stub_from_current_exe()?;
    if let Some(icon) = icon {
        stub = patch_icon(stub, icon)?;
    }
    let bytes = payload::build_output(&stub, config, archive)?;
    if output.extension().map(|e| e.to_ascii_lowercase()) != Some("exe".into()) {
        return Err(anyhow!("输出文件名必须以 .exe 结尾"));
    }
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).ok();
        }
    }
    std::fs::write(output, &bytes)
        .with_context(|| format!("写入产物失败：{}", output.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn zip_dir_roundtrips_files() {
        let tmp = std::env::temp_dir().join(format!("h2e_test_{}", std::process::id()));
        let sub = tmp.join("assets");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(tmp.join("index.html"), b"<h1>hi</h1>").unwrap();
        std::fs::write(sub.join("app.js"), b"console.log(1)").unwrap();

        let zipped = zip_dir(&tmp).unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zipped)).unwrap();

        let mut names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .filter(|n| !n.ends_with('/'))
            .collect();
        names.sort();
        assert_eq!(names, vec!["assets/app.js", "index.html"]);

        let mut buf = String::new();
        archive
            .by_name("index.html")
            .unwrap()
            .read_to_string(&mut buf)
            .unwrap();
        assert_eq!(buf, "<h1>hi</h1>");

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn pack_url_rejects_bad_scheme() {
        let out = std::env::temp_dir().join("h2e_bad.exe");
        let err = pack_url("ftp://nope", WindowCfg::default(), &out, None, None, true).unwrap_err();
        assert!(err.to_string().contains("http"));
    }

    #[test]
    fn maybe_encrypt_off_is_passthrough() {
        // protect=false：archive 逐字节不变、enc=None（与旧版行为一致）。
        let data = b"raw-zip-bytes".to_vec();
        let (out, enc) = maybe_encrypt(data.clone(), false);
        assert_eq!(out, data);
        assert!(enc.is_none());
    }

    #[test]
    fn maybe_encrypt_on_roundtrips_via_config() {
        // protect=true：archive 被加密（内容变化），用 enc 里的遮蔽 key/nonce 可还原原文。
        let data = b"<html>secret source</html>".repeat(10);
        let (cipher, enc) = maybe_encrypt(data.clone(), true);
        assert_ne!(cipher, data, "加密后不应等于明文");
        let enc = enc.expect("protect=true 应产生 enc");
        let key = crypto::unmask_key(&enc.key_b64).unwrap();
        let nonce = crypto::unmask_nonce(&enc.nonce_b64).unwrap();
        let plain = crypto::decrypt(&key, &nonce, &cipher);
        assert_eq!(plain, data, "解密未还原原文");
    }

    #[test]
    fn maybe_encrypt_empty_archive_stays_none() {
        // URL 模式等空 archive：即便 protect=true 也不加密。
        let (out, enc) = maybe_encrypt(Vec::new(), true);
        assert!(out.is_empty());
        assert!(enc.is_none());
    }

    #[test]
    fn detect_entries_prefers_root_index() {
        let tmp = std::env::temp_dir().join(format!("h2e_detect_{}", std::process::id()));
        let sub = tmp.join("pages");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(tmp.join("about.html"), b"x").unwrap();
        std::fs::write(tmp.join("index.html"), b"x").unwrap();
        std::fs::write(sub.join("deep.html"), b"x").unwrap();

        let entries = detect_html_entries(&tmp);
        assert_eq!(entries.first().map(String::as_str), Some("index.html"));
        assert!(entries.contains(&"about.html".to_string()));
        assert!(entries.contains(&"pages/deep.html".to_string()));
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn detect_entries_empty_when_no_html() {
        let tmp = std::env::temp_dir().join(format!("h2e_nohtml_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("readme.txt"), b"x").unwrap();
        assert!(detect_html_entries(&tmp).is_empty());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn zip_dir_rejects_empty_dir() {
        let tmp = std::env::temp_dir().join(format!("h2e_empty_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        assert!(zip_dir(&tmp).is_err());
        std::fs::remove_dir_all(&tmp).ok();
    }
}
