# 源码保护：payload 加密 + 关运行时查看

变更 ID：`source-protection-2026-08`
状态：设计已定，待实现

## 目标（一句话）
消灭"把产物 exe 用 7-zip 直接解压就还原出网页源码"的问题：打包时加密内嵌资源、
运行时禁用 DevTools 与右键查看源码。

## 诚实前提（务必在 README/说明里如实写）
- 客户端网页最终必须把明文交给 WebView 渲染，**解密密钥必然在 exe 内**，所以这是
  **"提高门槛、防君子不防小人"**，不是不可破解的安全。
- 本工具开源，加密**格式与逻辑均公开**。因此：
  - ✅ 挡住：用 7-zip/改后缀直接解压的普通用户；不看本仓库的技术用户。
  - ❌ 挡不住：读了本仓库源码、照格式写提取脚本的人（每个 exe 的随机密钥就在文件里）。
- 要"绝对不可得"只能把逻辑挪到服务端 / WASM / 原生，不在本工具范围内。

## 一、加密（ChaCha20 流密码，每个 exe 随机密钥）

### 密码实现
- **手写 ChaCha20**（RFC 8439），**零新依赖**（与项目现有手写 base64 的取舍一致）。
  必须用 RFC 8439 §2.4.2 官方测试向量写单测验证 keystream 正确。
- 若维护者更想用现成 crate，可换 `chacha20`，但默认手写零依赖。

### 随机密钥/nonce 生成（打包时）
- 每次打包生成随机 key(32B) + nonce(12B)。**obfuscation 级**，不追求 CSPRNG 极致：
  优先零依赖方案——用 `std::collections::hash_map::RandomState::new()`（每次调用取 OS 熵
  作 SipHash 种子）多次取字节拼出；或维护者接受的话用 `getrandom` 小 crate。

### 加密范围与格式
- **只加密 archive（网页 zip 字节）**；config JSON 保持明文（含窗口设置/URL/图标/启动图，
  这些不是要藏的"源码"）。
- **不改 payload 二进制格式与 MAGIC**（`payload.rs` 把 archive 当不透明字节，加密后长度变化
  由既有 payload_len 记录，向后兼容）。加密只改 archive 内容，不改 trailer 结构。
- key/nonce 存进 config 新增字段（base64），并 XOR 一个编译期常量做**轻度遮蔽**（诚实说明：
  常量因开源而公开，仅挡 `strings`/非仓库读者，不构成真正保护）。

### config.rs 新增（全部 `#[serde(default)]`，向后兼容）
```rust
pub struct EncInfo {          // 加密信息
    pub key_b64: String,      // 遮蔽后的 key（base64）
    pub nonce_b64: String,    // 遮蔽后的 nonce（base64）
}
pub struct PackConfig {
    // ...现有...
    #[serde(default)] pub enc: Option<EncInfo>,   // None=未加密(旧产物)
    #[serde(default)] pub protect: bool,          // 是否禁用运行时查看
}
```

### packer.rs
- 新增参数 `protect: bool`（由 GUI/CLI 传入，默认 true）。
- `protect` 为真时：生成随机 key/nonce → ChaCha20 加密 zip → 填 `config.enc`（遮蔽后）、
  `config.protect = true`；否则维持现状（archive 明文、enc=None）。

### runner.rs 解密
- `unzip_to_map` 前：若 `config.enc.is_some()`，先取 key/nonce（去遮蔽）→ ChaCha20 解密
  archive（内存中，**不落盘**）→ 再解压。enc=None 走原明文路径。

## 二、关运行时查看（protect 为真时）

### runner.rs WebView
- `WebViewBuilder::with_devtools(false)`（核对 wry 0.56 该 API 存在与签名；类似此前核对方式）。
- `with_initialization_script(...)` 注入脚本，屏蔽：
  - `contextmenu` 事件（右键菜单/查看源码）：`e.preventDefault()`。
  - 常见开发者快捷键：`F12`、`Ctrl+Shift+I/J/C`、`Ctrl+U`：`keydown` 里 `preventDefault`。
- 诚实说明：JS 屏蔽可被禁用 JS/外部工具绕过，属 casual 阻挡，与整体定位一致。
- `protect=false` 时不注入、不禁 DevTools，保持现状（便于调试）。

## 三、UI / GUI

- 侧栏新增一个 section「源码保护」（第 5 项），含**一个开关**：
  「加密源码，防止直接解压 + 禁用查看源码」**默认开**，下方一行小字诚实提示
  「提高门槛，非绝对安全」。
- `ui/app.js`：pack 请求带 `protect`（bool）。
- `gui.rs`：`PackReq` 加 `#[serde(default = "..true")] protect`，`do_pack` 传给 packer。
- `main.rs` CLI：`pack-local`/`pack-url` 默认 protect=true（可留一个可选参数关，非必须）。

## 四、涉及文件
| 文件 | 改动 |
|------|------|
| `src/crypto.rs`（新增） | 手写 ChaCha20 + 遮蔽 helper + RFC8439 向量测试 + 加解密往返测试 |
| `src/config.rs` | 加 `EncInfo`、`enc`、`protect` 字段（serde default），加旧 JSON 兼容测试 |
| `src/packer.rs` | `protect` 参数；加密 archive、填 enc |
| `src/runner.rs` | 解密 archive；`with_devtools(false)` + 屏蔽脚本 |
| `src/gui.rs` | PackReq 加 protect；透传 |
| `src/main.rs` | 注册 mod crypto；CLI 默认 protect=true |
| `ui/index.html` | 新增「源码保护」侧栏项 + section + 开关 |
| `ui/style.css` | 视需要 |
| `ui/app.js` | 侧栏项 + pack 带 protect |

## 五、不变量
- 不改 payload 二进制格式与 MAGIC；新字段全 `#[serde(default)]`，旧产物仍可运行。
- 手写 ChaCha20 必须过 RFC 8439 官方测试向量。
- 解密只在内存，绝不落盘。
- `protect=false` 时行为与旧版逐字节一致。
- 零新依赖优先；UI 图标 SVG、无 emoji；`cargo fmt` + `clippy -D warnings` 干净。
- 不 commit（由 PM 审查后决定）。

## 六、验证
- `cargo test`：ChaCha20 向量、加解密往返、加密后 payload 往返、旧 JSON 兼容，全过。
- 手测：打一个带源码的本地站（protect 开）→ 用 7-zip 打开产物 exe **无法解出源码**；
  运行正常；F12/右键被禁。再打一个 protect 关 → 行为如旧版。
