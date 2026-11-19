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

1. 壳层藏起已有遮罩，并告诉 core 刚才有没有遮罩挡在屏幕上。
2. `capture_session::grab` 只冻结光标所在的那块屏。每块屏的几何信息只查一次，剩余屏的抓取复用这份信息。
3. 壳层把这块屏的遮罩显示出来。冻结文件这时才开始写，出图前一直由内存缓存供像素。
4. 还有别的显示器时，壳层等光标屏这一代的 `overlay_ready` 到了再调用 `capture_session::capture_later`，250ms 不到就不等了。core 只抓剩余的屏。
5. 这次按键仍是当前一次时，壳层把新的遮罩显示出来。代次已经变了就丢掉。遮罩页收得到每一块屏的显示通知，只采用自己 slot 的那一次。自己的像素还没进这次冻结帧时，不能把这次代次记成已经画过。

`SHOTLET_TRACE=1` 时，F1 和贴图两条链路把每个阶段追加到 `shotlet.log`。不设这个变量时不计时。

贴图时，没有标注的选区由 `library::pin_capture_region` 直接从内存缓存裁出并写贴图文件，不经过 canvas 和 base64。有标注的选区由 `library::pin_annotated` 把 PNG 字节原样落盘，宽高从文件头读。两条路的历史记录都在后台线程写，窗口不等它。

窗口列表和写冻结文件跟在抓屏机制里，不单开一条壳层流程。壳层不计算打包坐标，也不保存第二份“这次截图还算不算数”。

截图遮罩还在时，Esc 关掉这次截图。页面把这个键听在窗口上，不要求选区先被点中。壳层在遮罩显示期间再看一次这个键，窗口没拿到键盘焦点时也能关掉。

`commands.rs` 只出现 `capture_session::`、`library::`、`scroll_session::`、`record_session::`。不出现 `history::`、`capture::`、`scroll::`、`record::`、`ocr::`、`util::`、`settings::`。

## 不搬过来的部分

DBX 发布配置用 `opt-level = "s"` 换体积。Shotlet 的发布配置保持 `opt-level = 3`，抓屏和编码走速度。依赖方向照 DBX，编译体积策略不照搬。

这里没有 CLI、MCP，也没有效果解释器。React 界面不因为 crate 拆分而改交互。

## 改代码时

- 新的用户动作放进已有的 core 过程，或在 core 里加一个过程。
- 新的机制放进上表里拥有它的 crate。
- 壳层只增加窗口摆放和 IPC 转发。
- 不在下层 crate 里加应用代次、全局会话或“当前是不是这一次按键”。
- 不把同一条流程拆成壳层里的第二套状态机。
