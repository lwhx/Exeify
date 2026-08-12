// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 不坑老师 · https://github.com/44886/Exeify

//! 图标处理：把用户的 .png / .ico 统一转成 Windows .ico 字节。
//!
//! 用 `png` 库自行解码，支持 索引色 / 灰度 / RGB / RGBA 全类型，
//! 再缩放到多个标准尺寸生成 .ico（避免 `ico::read_png` 只吃 RGB/RGBA 的限制）。

use anyhow::{anyhow, bail, Context, Result};
use std::path::Path;

/// 把图标文件转成 .ico 字节。`.ico` 直接读取，其它按 PNG 解码转换。
pub fn to_ico_bytes(path: &Path) -> Result<Vec<u8>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "ico" {
        return std::fs::read(path).with_context(|| format!("读取图标失败：{}", path.display()));
    }
    let (w, h, rgba) = decode_png_rgba(path)?;
    build_ico_from_rgba(w, h, &rgba)
}

/// 把图标文件（.png / .ico）统一解码为 RGBA8，返回 (宽, 高, 像素)。
/// 用于运行时窗口/任务栏图标：`.ico` 取尺寸最大的一帧，其它按 PNG 解码。
pub fn decode_to_rgba(path: &Path) -> Result<(u32, u32, Vec<u8>)> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "ico" {
        let file = std::fs::File::open(path)
            .with_context(|| format!("打开图标失败：{}", path.display()))?;
        let dir = ico::IconDir::read(std::io::BufReader::new(file))
            .map_err(|e| anyhow!("ICO 解析失败：{e}"))?;
        let entry = dir
            .entries()
            .iter()
            .max_by_key(|e| e.width() * e.height())
            .ok_or_else(|| anyhow!("ICO 没有任何图标帧"))?;
        let img = entry.decode().map_err(|e| anyhow!("ICO 解码失败：{e}"))?;
        Ok((img.width(), img.height(), img.rgba_data().to_vec()))
    } else {
        decode_png_rgba(path)
    }
}

/// 双线性缩放 RGBA（对外复用，如生成标准尺寸的窗口图标）。
pub fn resize_rgba_to(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    resize_rgba(src, sw, sh, dw, dh)
}

/// 解码 PNG 为 RGBA8，兼容索引色 / 灰度等。
fn decode_png_rgba(path: &Path) -> Result<(u32, u32, Vec<u8>)> {
    let file =
        std::fs::File::open(path).with_context(|| format!("打开图标失败：{}", path.display()))?;
    decode_png_rgba_reader(file)
}

/// 从内存字节解码 PNG 为 RGBA8（用于内嵌图标/窗口图标）。
pub fn decode_png_rgba_bytes(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    decode_png_rgba_reader(std::io::Cursor::new(bytes))
}

fn decode_png_rgba_reader<R: std::io::Read>(reader: R) -> Result<(u32, u32, Vec<u8>)> {
    let mut decoder = png::Decoder::new(reader);
    // EXPAND：索引色→RGB(A)、低位深→8位、tRNS→alpha；STRIP_16：16位→8位
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|e| anyhow!("PNG 解析失败：{e}"))?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| anyhow!("PNG 读取失败：{e}"))?;
    let data = &buf[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => data.to_vec(),
        png::ColorType::Rgb => data
            .chunks(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => data
            .chunks(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => data.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => bail!("PNG 调色板未正确展开"),
    };
    Ok((info.width, info.height, rgba))
}

/// 从 RGBA 生成多尺寸 .ico。
fn build_ico_from_rgba(w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    if w == 0 || h == 0 {
        bail!("图标尺寸无效");
    }
    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    let max_side = w.max(h).min(256); // ICO 单条目最大 256
                                      // 生成不超过源尺寸的标准尺寸集合
    let mut sizes: Vec<u32> = [256, 128, 64, 48, 32, 16]
        .into_iter()
        .filter(|&s| s <= max_side)
        .collect();
    if sizes.is_empty() {
        sizes.push(max_side.max(1));
    }
    for s in sizes {
        let resized = resize_rgba(rgba, w, h, s, s);
        let img = ico::IconImage::from_rgba_data(s, s, resized);
        dir.add_entry(ico::IconDirEntry::encode(&img).map_err(|e| anyhow!("图标编码失败：{e}"))?);
    }
    let mut out = Vec::new();
    dir.write(&mut out)
        .map_err(|e| anyhow!("图标写出失败：{e}"))?;
    Ok(out)
}

/// 双线性缩放 RGBA。
fn resize_rgba(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    if sw == dw && sh == dh {
        return src.to_vec();
    }
    let mut out = vec![0u8; (dw * dh * 4) as usize];
    let sx = sw as f32 / dw as f32;
    let sy = sh as f32 / dh as f32;
    let px = |xx: u32, yy: u32, c: usize| src[((yy * sw + xx) * 4 + c as u32) as usize] as f32;
    for y in 0..dh {
        let fy = ((y as f32 + 0.5) * sy - 0.5).max(0.0);
        let y0 = fy.floor() as u32;
        let y1 = (y0 + 1).min(sh - 1);
        let wy = fy - y0 as f32;
        for x in 0..dw {
            let fx = ((x as f32 + 0.5) * sx - 0.5).max(0.0);
            let x0 = fx.floor() as u32;
            let x1 = (x0 + 1).min(sw - 1);
            let wx = fx - x0 as f32;
            for c in 0..4 {
                let top = px(x0, y0, c) * (1.0 - wx) + px(x1, y0, c) * wx;
                let bot = px(x0, y1, c) * (1.0 - wx) + px(x1, y1, c) * wx;
                let v = top * (1.0 - wy) + bot * wy;
                out[((y * dw + x) * 4 + c as u32) as usize] = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}
