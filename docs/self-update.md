# 一键更新（打包器自更新，基于 GitHub Release）

变更 ID：`self-update-2026-08`
状态：设计已定，待实现

## 目标（一句话）
打包器 `exeify.exe` 能检测 GitHub 最新 Release，一键下载新版并自替换重启。
**仅更新打包器工具本身**，不涉及打包出来的产物 exe。

## 决策（已定）
- 网络：新增 `ureq`（HTTPS crate，rustls TLS，纯 Rust，不依赖系统 curl/OpenSSL）。
- 触发：**手动**——「关于」弹窗里放「检查更新」按钮，点了才查，不静默联网。
- 完整性：HTTPS（rustls 证书校验）为基线；SHA256 校验列为后续增强（需发布时附校验和），本次不做。

## 原理（Windows 自替换）
1. 查最新版：`GET https://api.github.com/repos/44886/Exeify/releases/latest`
   （带 `User-Agent: exeify-updater` 与 `Accept: application/vnd.github+json`），
   取 `tag_name` 与第一个后缀 `.exe` 的资产 `browser_download_url`。
2. 比较版本：当前版本 = `env!("CARGO_PKG_VERSION")`，与 tag（去掉前导 `v`）比。
3. 下载：`GET` 该资产（ureq 自动跟随重定向到 CDN），得到新 exe 字节。
4. 自替换：
   - `current = std::env::current_exe()`；`old = 同目录/<name>.old.exe`。
   - 先删残留 old → `fs::rename(current, old)`（Windows 允许重命名运行中的 exe）
     → 把新字节写到 `current` → `Command::new(current).spawn()` 启动新版 → 本进程退出。
   - 失败回滚：写失败则把 old 改名回 current。
5. 清理：`main()` 启动时 best-effort 删除残留的 `<name>.old.exe`。

## 涉及文件
| 文件 | 改动 |
|------|------|
| `Cargo.toml` | 加 `ureq`（核对当前版本/特性；最小 TLS 特性，rustls） |
| `src/update.rs`（新增） | 版本比较（纯函数 + 单测）、check_latest、download、apply_update、cleanup_old |
| `src/gui.rs` | IPC：`checkUpdate` / `doUpdate`；后台线程执行，结果回调 UI |
| `src/main.rs` | `mod update;`；启动时 `update::cleanup_old()` |
| `ui/index.html` | 「关于」弹窗加：当前版本、「检查更新」按钮、结果区、「立即更新」按钮 |
| `ui/app.js` | 按钮事件 + 回调渲染（当前/最新/有无更新/更新中/失败） |
| `ui/style.css` | 视需要 |

## 关键接口（src/update.rs）
```rust
pub fn current_version() -> &'static str { env!("CARGO_PKG_VERSION") }

/// 语义化版本比较：latest 是否比 current 新。解析 major.minor.patch。
pub fn is_newer(latest: &str, current: &str) -> bool;  // 纯函数，重点单测

pub struct Latest { pub tag: String, pub version: String,
                    pub asset_url: String, pub notes: String }
pub fn check_latest() -> anyhow::Result<Latest>;       // 网络
pub fn download(url: &str) -> anyhow::Result<Vec<u8>>; // 网络
pub fn apply_update(new_exe: &[u8]) -> anyhow::Result<()>; // 自替换+重启（成功则不返回，进程退出）
pub fn cleanup_old();                                   // 启动时清理 .old
```

## GUI 交互
- 「关于」弹窗顶部显示 `当前版本 vX.Y.Z`。
- 「检查更新」按钮 → 后台 `check_latest` → 回调：
  - 无网络/失败：显示「检查失败：<原因>」。
  - 已最新：显示「已是最新版本」。
  - 有新版：显示「发现新版本 vA.B.C」+「立即更新」按钮（可附 Release 说明摘要）。
- 「立即更新」→ 后台 `download` +（进度可选，简单显示「下载中…」）→ `apply_update`
  → 提示「更新完成，正在重启」→ 进程自替换重启。

## 不变量 / 约束
- 仅手动触发，不静默联网（尊重隐私）。
- 网络与自替换都在后台线程，不阻塞 UI；失败有清晰提示、可回滚，不留坏 exe。
- exe 目录不可写（如 Program Files）时给出提示，不静默失败。
- `is_newer` 必须有单测覆盖：更大/更小/相等/位数不齐（0.3 vs 0.3.0）/带 v 前缀。
- 不改产物 runner 逻辑；不引入除 ureq 外的重依赖。
- `cargo fmt` + `clippy -D warnings` 干净；UI 图标 SVG、无 emoji；不 commit。

## 验证
- 单测：`is_newer` 各类用例。
- 手测：点「检查更新」——当前 0.3.0、仓库最新若为 0.2.1 → 显示「已是最新」；
  待发布 0.3.1 后再测能否检测、下载、替换、重启成功；断网时提示「检查失败」。
