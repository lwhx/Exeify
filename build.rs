// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 不坑老师 · https://github.com/44886/exeify

fn main() {
    // 把 exeify.exe 自身图标编入 PE 资源（仅 Windows）。
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/icon.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        if let Err(e) = res.compile() {
            // 缺少 rc.exe 时不硬失败，仅告警（此时用默认图标）。
            println!("cargo:warning=嵌入图标失败：{e}");
        }
    }
}
