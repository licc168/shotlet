# Shotlet 模块边界

结构照 DBX 的依赖方向：上层调用过程，过程在 `shotlet-core`，机制在下层 crate。下层不得依赖 core，包括 build 和 dev 依赖。桌面壳只改编这些过程，然后打开或关闭窗口。

```text
src-tauri（窗口、托盘、全局快捷键、IPC）
        │
   shotlet-core（过程与 AppState，不开窗口）
        ├── shotlet-media ── shotlet-capture ── shotlet-types
        │       ├── shotlet-image               shotlet-platform
        │       └── shotlet-platform
        ├── shotlet-store ── shotlet-types + shotlet-platform
        └── shotlet-platform
```

`scripts/architecture.test.mjs` 校验 workspace 依赖，以及 `commands.rs` 只调用过程模块。`npm test` 会跑这份守卫。

## 所有权

| Crate | 拥有的实现 | 不引入 |
| --- | --- | --- |
| `shotlet-types` | 截图、窗口、设置这些 DTO | 抓屏、文件、窗口句柄、应用状态 |
| `shotlet-platform` | PNG、剪贴板、id、冻结帧的字节读写 | 抓哪块屏、历史、会话 |
| `shotlet-image` | 像素拼接 | 文件、抓屏、历史 |
| `shotlet-capture` | 抓屏、窗口枚举、预览像素缓存 | 应用代次、开窗口、历史 |
| `shotlet-store` | 设置和历史文件 | 抓屏、开窗口 |
| `shotlet-media` | OCR、录制、滚动拼接 | 开窗口、应用状态 |
| `shotlet-core` | 截图、历史、滚动、录制、快捷键过程，以及 `AppState` | 创建或显示窗口 |
| `src-tauri` | 窗口位置、托盘、全局快捷键、事件广播 | 自己决定抓屏顺序、像素格式、会话是否仍有效 |

`shotlet-core` 用 `pub use` 把旧路径留在壳层原来的名字上。同一份 DTO 不在两处各定义一次。

目录里放真正的实现。过程进 `capture_session`、`library`、`scroll_session`、`record_session`，不要在 `commands.rs` 里再写一套。

## 壳层怎么调用

一次用户动作是：壳层先处理窗口，调用一个 core 过程，再按返回结果开关窗口。会话是否还有效只看 `AppState` 里的代次。抓屏 crate 不另设一套 tick。

按下 F1 时：

1. 壳层把已经揭开的遮罩 cloak 起来，并告诉 core 刚才有没有遮罩挡在屏幕上。cloak 之后调一次 `DwmFlush`，不再固定睡 60ms。`SHOTLET_OVERLAY_CLOAK=0` 时退回隐藏，并保留那 60ms。macOS 一直用隐藏。
2. `capture_session::grab` 只冻结光标所在的那块屏。每块屏的几何信息只查一次，剩余屏的抓取复用这份信息。
3. 启动时每块屏的遮罩已经摆好，并保持鼠标穿透和 cloak。F1 只在显示器矩形变了的时候才重摆。`overlay-show` 带上路径、宽高和代次，页面立刻取像素。画完调用 `overlay_ready`，壳层再取消穿透和 cloak，并给光标屏焦点。300ms 没等到也揭开。
4. 其余屏幕在这一轮里紧接着抓，不等光标屏的 `overlay_ready`。哪块先画好先揭开。
5. 这次按键仍是当前一次时才把新的遮罩交给页面。代次已经变了就丢掉。遮罩页只采用自己 slot 的那一次。自己的像素还没进这次冻结帧时，不能把这次代次记成已经画过。关闭时 cloak 并发 `overlay-hide`，页面丢掉上一帧。

`SHOTLET_TRACE=1` 时，F1 和贴图两条链路把每个阶段追加到 `shotlet.log`。不设这个变量时不计时。

贴图时，没有标注的选区由 `library::crop_pin_image` 从内存缓存裁出。页面用 `pin_pixels` 直接画这些像素。PNG 只编码一次，同一份字节在后台写入贴图文件和历史，窗口不等它。有标注的选区把画好的像素作为二进制请求体发给 `pin_annotated`，底图由壳层从同一块冻结缓存裁出。标注文档走 `pin_history`，不挡住贴图窗口。二进制请求体不可用时，仍走原来的 base64。

贴图窗口的位置和大小只由壳层决定。`pin_session` 记下每个窗口是在预热、待命、已分配还是已显示。页面重复报到不会把同一个窗口再放进待命。壳层先摆好窗口，再发 `pin-assign`。页面只画图。滚轮缩放走 `pin_zoom`，按原图尺寸一次摆到当前中心。位图像素换成桌面坐标用 `capture_session::desktop_rect`，贴图、录制和滚动截图共用。

启动后壳层预建一个隐藏的贴图窗口。页面加载完报到一次。没有待命窗口时仍新建。隐藏和切换贴图组跳过还没分配图片的窗口。窗口销毁时从 `pin_session` 删掉。

窗口列表和写冻结文件跟在抓屏机制里，不单开一条壳层流程。壳层不计算打包坐标，也不保存第二份“这次截图还算不算数”。

截图遮罩还在时，Esc 关掉这次截图。页面把这个键听在窗口上，不要求选区先被点中。壳层在遮罩显示期间再看一次这个键，窗口没拿到键盘焦点时也能关掉。

`commands.rs` 只出现 `capture_session::`、`library::`、`scroll_session::`、`record_session::`、`pin_session::`。不出现 `history::`、`capture::`、`scroll::`、`record::`、`ocr::`、`util::`、`settings::`。

## 不搬过来的部分

DBX 发布配置用 `opt-level = "s"` 换体积。Shotlet 的发布配置保持 `opt-level = 3`，抓屏和编码走速度。依赖方向照 DBX，编译体积策略不照搬。

这里没有 CLI、MCP，也没有效果解释器。React 界面不因为 crate 拆分而改交互。

## 平台

Windows 和 macOS 的差异照 DBX：不按操作系统拆 crate，也不把同一条业务分成 `src/windows` 和 `src/macos`。包仍然按职责切。系统差异只换实现文件，调用方始终用同一个模块名。

做法：

1. 同一模块名。Windows 编真正的实现，其他平台用 `#[path]` 编另一份文件。调用处不写 `cfg`，也不在一个函数里分出两套流程。
2. 另一份文件是完整实现或空实现。macOS 已有自己的行为时，保留那份行为，不要在 Windows 改动里改掉它。没有对应能力时，空实现什么都不做。
3. 只有几行、而且两边语义相同的差别，可以留在函数内部的 `cfg`。例如托盘图标、读 Esc。一整段流程、Win32 或 WebView2 调用，不留在共享文件里。
4. 依赖写在 `Cargo.toml` 的 `[target.'cfg(...)'.dependencies]`。macOS 不链 `webview2-com` 和 `windows`。Windows 不链 macOS 的 Objective-C。
5. 安装包资源可以按目录放，例如 NSIS 或 `Info.plist`。那是打包材料，不是业务代码。

```rust
#[cfg(windows)]
mod preview_buffer;
#[cfg(not(windows))]
#[path = "preview_buffer_noop.rs"]
mod preview_buffer;
```

现在已经这样拆开的：

| 模块名 | Windows | 其他平台 | 调用方 |
| --- | --- | --- | --- |
| `preview_buffer` | `src-tauri/src/preview_buffer.rs`，共享内存把冻结帧交给页面 | `preview_buffer_noop.rs`，什么都不做，页面仍走 `preview_rgba` | `ui.rs` 只调用 `preview_buffer::post` |
| `drive` | `crates/shotlet-media/src/scroll_watch.rs`。默认拖滚动条拼接。Windows 和 macOS 用同一份 | 同上 | `scroll::capture_manual` 只调用 `drive::run` |
| `wheel` | `scroll_wheel.rs`，发一次滚轮 | `scroll_wheel_noop.rs`，不发 | 只有用户打开「自动滚动」时，`drive::run` 才调用 `wheel::nudge` |

新增一种只有一个系统才有的能力时，按上表加一对文件。不要在 `ui.rs`、`commands.rs` 或 `scroll.rs` 里再写一段 `#[cfg(windows)] { ... } #[cfg(not(windows))] { ... }` 的流程。

## 改代码时

- 新的用户动作放进已有的 core 过程，或在 core 里加一个过程。
- 新的机制放进上表里拥有它的 crate。
- 壳层只增加窗口摆放和 IPC 转发。
- 不在下层 crate 里加应用代次、全局会话或“当前是不是这一次按键”。
- 不把同一条流程拆成壳层里的第二套状态机。
- 只在一个系统上成立的流程，按「平台」一节换成同名模块的另一份文件。改 Windows 时不要改 macOS 那份文件的行为。
