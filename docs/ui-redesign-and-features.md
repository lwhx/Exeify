# 卡片式界面重构 + 启动页 / 窗口图标 / 全屏 三大特性

变更 ID：`ui-redesign-2026-08`
状态：设计已定，待实现

## 目标（一句话）

把打包器界面改成卡片式分类，并新增三项能力：产物启动时显示启动页图片（消除白屏）、
运行时窗口/任务栏使用用户选择的图标、窗口可设置为最大化 / 全屏无边框。

---

## 一、三大特性设计

### 特性 A：启动页图片（消除白屏）

**根因**：`runner.rs` 创建窗口后直接挂 WebView，WebView2 进程初始化 + 首次导航渲染前，
窗口是纯白的。

**行为决策（已确认）**：
- 消失时机：**页面就绪（WebView 首帧/加载完成）+ 最少显示 N 秒**（防一闪而过）。
- 图片摆放：**cover 铺满窗口、居中裁剪**，并配一个**背景色**在图片加载出来前兜底。

**技术方案**：
1. 打包时把启动图（png/jpg 原始字节）base64 存入 config；连带 `min_ms`、`bg`（背景色）、`fit`。
2. runner 端：
   - `WebViewBuilder::with_background_color(bg)` —— 覆盖 WebView 初始化那段纯白（瞬间显示品牌底色）。
   - **先加载启动页 data-URL HTML**（内嵌 base64 图 + `object-fit:cover` + 背景色），
     再在 `PageLoadEvent::Finished`（wry 页面加载完成事件）且已过 `min_ms` 后
     `webview.load_url(真实目标)`。
   - 用"先启动页、后导航"而非"往目标页注入遮罩"，避免远程站点 CSP 拦截内联图/样式。
3. 若未设置启动图：保持现状（仅 `with_background_color` 可选），不加启动页逻辑。

**边界**：
- `min_ms` 默认 1500ms，UI 暴露为"启动页最少显示(秒)"数字框，默认 1.5（用户可调）。
- 背景色 `bg` 默认 `#0f172a`（深石板），UI 提供颜色选择器可改。
- 启动图为空 → 特性完全关闭，行为与旧版一致。

### 特性 B：窗口 / 任务栏图标用用户图标（bug 修复）

**根因**：
- 用户图标只写进产物 exe 的 PE 资源（`packer.rs:patch_icon`）→ 只影响资源管理器**文件图标**。
- 运行时窗口/任务栏图标走 `runner.rs:54 → app_window_icon()`，而该函数（`main.rs:31`）
  **永远加载内置 `assets/icon.png`**，从没用过用户图标。任务栏图标 = 窗口图标，所以一起没变。

**技术方案**：
1. 打包时若用户选了图标：把源图解码成 RGBA（复用 `icon::decode_png_rgba_bytes`；`.ico` 用 `ico` crate 读首帧），
   取一个标准尺寸（如 256×256，用现有 `resize_rgba`），把 `{w, h, rgba(base64)}` 存进 config。
2. runner 端：优先用 config 里的用户图标构造 `tao::window::Icon::from_rgba(...)` 设 `with_window_icon`；
   没有则回退 `app_window_icon()`（内置图标）。
3. PE 资源图标逻辑（文件图标）保持不变。

**注意**：源图是 `.ico` 时也要能拿到 RGBA。`ico` crate 支持读取；实现时封装一个
`icon::decode_to_rgba(path) -> (w,h,Vec<u8>)`，png 与 ico 都走它。

### 特性 C：窗口启动状态（普通 / 最大化 / 全屏无边框）

**技术方案**：
1. config 新增枚举：
   ```rust
   #[derive(Serialize, Deserialize)]
   #[serde(rename_all = "lowercase")]
   pub enum WindowState { Normal, Maximized, Fullscreen }
   ```
   `WindowCfg` 加 `#[serde(default)] state: WindowState`（默认 `Normal`）。
2. runner 端按 state：
   - `Normal`：现状（`with_inner_size` 用宽高）。
   - `Maximized`：`with_maximized(true)`。
   - `Fullscreen`：`with_fullscreen(Some(Fullscreen::Borderless(None)))` + `with_decorations(false)`。
3. UI：把"宽 / 高"与一个"启动状态"下拉放一起；选到"最大化 / 全屏"时把宽高输入置灰（宽高仅普通模式生效）。

---

## 二、卡片式界面重构（`ui/`）

四张卡片，纵向排列，沿用现有配色与 SVG 图标（**禁止 emoji 图标**）：

1. **内容来源**：本地目录 / 在线网址 Tab + 入口文件下拉（迁移现有 local/url 面板）。
2. **窗口设置**：窗口标题、启动状态下拉（普通/最大化/全屏无边框）、宽 / 高、允许调整大小。
3. **外观**：程序图标（现有）、启动页图片（新增：选择/清除/预览）、启动页最少显示秒数、背景色。
4. **输出**：保存路径 + 开始打包按钮 + 状态区。

关于弹窗保持不变。卡片是纯 CSS/HTML 结构调整，逻辑走原有 IPC。

---

## 三、数据格式变更（`config.rs` + payload）

**不改 trailer 二进制格式与 MAGIC**，只在 config JSON 里加**可选**字段（`#[serde(default)]`），
旧产物仍可解析（向后兼容）。图标/启动图以 base64 内嵌进 config JSON。

```rust
pub struct WindowCfg {
    pub title: String,
    pub width: f64,
    pub height: f64,
    pub resizable: bool,
    #[serde(default)] pub state: WindowState,   // 新增
}

pub struct SplashCfg {          // 新增
    pub image_b64: String,      // png/jpg 原始字节的 base64
    pub mime: String,           // "image/png" | "image/jpeg"
    pub min_ms: u64,            // 最少显示毫秒
    pub bg: String,             // 背景色 "#rrggbb"
    pub fit: String,            // "cover"
}

pub struct WindowIcon {         // 新增：运行时窗口/任务栏图标
    pub w: u32,
    pub h: u32,
    pub rgba_b64: String,       // RGBA8 原始像素的 base64
}

pub struct PackConfig {
    // ...现有...
    #[serde(default)] pub splash: Option<SplashCfg>,
    #[serde(default)] pub window_icon: Option<WindowIcon>,
}
```

---

## 四、涉及文件与改动

| 文件 | 改动 |
|------|------|
| `src/config.rs` | 加 `WindowState`、`SplashCfg`、`WindowIcon`；`WindowCfg`/`PackConfig` 加字段 |
| `src/icon.rs` | 加 `decode_to_rgba`（png+ico 统一）；供窗口图标复用缩放 |
| `src/packer.rs` | `pack_*` 接收 splash / window state / 生成 window_icon RGBA 写进 config |
| `src/gui.rs` | IpcMsg/PackReq 加字段；新增 `pickSplash` 处理 + 预览 base64；组装 config |
| `src/runner.rs` | 启动页两段导航 + `with_background_color`；窗口图标优先用户图标；窗口状态分支 |
| `src/main.rs` | CLI 入口按需透传（可保持默认，不强求） |
| `ui/index.html` | 改为四卡片结构 + 启动页/背景色/状态下拉控件 |
| `ui/style.css` | 卡片样式 |
| `ui/app.js` | 新控件事件 + pack 请求组装新字段 + 状态联动置灰 |

---

## 五、不变量 / 禁止项

- 不改 trailer 二进制格式与 `MAGIC`；新字段一律 `#[serde(default)]`，旧产物必须仍能运行。
- UI 图标必须 SVG，**禁止 emoji**。
- `cargo fmt` + `cargo clippy -D warnings` 必须过。
- 启动图 / 图标为空时，行为回退到与旧版完全一致。
- 不在实现中 commit（由 PM 审查后决定）。

## 六、验证

- `cargo build`（注意本机：E 盘满 + cargo 网络限流，构建需重定向到 C 盘 target + 关沙箱）。
- `cargo test`（payload/packer 既有测试须绿；为新字段加 serde 默认值往返测试）。
- 手测三条关键路径：
  1. 选图标打包 → 运行产物 → 标题栏 + 任务栏图标 = 用户图标。
  2. 选启动图打包 → 运行产物 → 先看到启动图（无白屏），≥N 秒后切到网页。
  3. 选"全屏无边框"打包 → 运行产物 → 无边框铺满屏幕。
