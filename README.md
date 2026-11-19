# Shotlet

Windows 和 macOS 上的截图、标注、贴图工具，用 Tauri 2、Rust 和 React 重新实现。交互参照 [imjakey/shotlet](https://github.com/imjakey/shotlet)：冻结屏幕、框选、标注、复制、保存、贴在所有窗口之上，以及历史记录。

这不是那个 C++/SDL 仓库的移植。原项目的 Node 插件和 C SDK 不在这里。

## 能做什么

- `F1` 冻结所有显示器并框选。`Ctrl+F1` 框选后直接复制。`Shift+F1` 截取光标下的窗口并进入标注。`F3` 把剪贴板里的图像贴到屏幕上。`Shift+F3` 隐藏或显示当前贴图组。`Ctrl+F3` 在两组贴图之间切换。这些键可以在设置里改。Shift 约束为正方形，滚轮在重叠窗口之间切换，双击窗口直接选中。拖选时有放大镜和颜色值。`Win+Shift+H` 打开历史。
- 矩形、椭圆、箭头、画笔、马赛克、高亮、文字、序号、水印。可以撤销，也可以再选中后移动。
- 复制、保存 PNG、OCR（Windows 系统识别）、贴图。贴图滚轮缩放，Ctrl+滚轮改透明度，双击关闭。
- 历史记录按名称、标签和 OCR 文字搜索，可以收藏、重命名、软删除、批量导出和再次编辑。
- 滚动长截图：对选区发送滚轮并拼接新露出的内容。
- 区域录制为 GIF。本机有 `ffmpeg` 时可以录 MP4。最长约 12 秒，画面会缩到 960px 以内。
- 托盘菜单带这组截屏和贴图动作。中英文、浅色/深色。关闭主窗口后仍留在托盘，左键托盘图标打开主窗口，菜单里退出。

## 开发

需要 Node.js 18+、Rust 1.77+，以及 Windows 上的 WebView2。模块边界见 [crates/ARCHITECTURE.md](crates/ARCHITECTURE.md)。

```bash
npm install
npm test
npm run tauri dev
```

配置在应用数据目录的 `settings.json`。截图默认写在同目录的 `history` 下。录制文件在「视频/Shotlet」。
