# Replay Guardian

一个 Rust 编写的 Windows 10/11 x64 托盘工具。可以监控 NVIDIA 即时回放，在关闭时自动打开。

目前最新版本为 0.1.4。请先从旧版设置中退出，再运行新版。

使用 Win32 原生窗口、托盘、注册表通知和 WinHTTP。

## 使用

可以直接运行exe文件启动工具。

默认自动选择检测方式、60 秒检测间隔、120 秒重试间隔、开启自动恢复。已有配置继续使用保存的间隔。自启和恢复成功通知默认关闭。**监控无法判断你是否主动关闭了即时回放；需要保持关闭时，请先暂停监控或关闭自动恢复。**

### 建议优先使用 MCP

需要 NVIDIA App 11.0.9.5xx 或以上，启用 NVIDIA App 的 MCP 访问和 In-Game Overlay。系统托盘中可查看当前实际采用的检测方式，设置下拉框显示你的选择。

- 自动：优先 MCP，其次经过本机校验的注册表事件，最后注册表轮询。
- NVIDIA MCP：只使用官方接口，不可用时显示未知和错误。
- 注册表轮询：按 5、10、30 或 60 秒读取状态。
- 注册表事件：变化时重读，另有健康检查和恢复重试。

MCP 使用本机 HTTP；HTTP 不可用时按需使用 NVIDIA 安装目录里的官方 stdio 桥接程序。现代协议 `2026-07-28` 和旧协议 `2025-11-25` 的处理流程分开。

## 配置与自启

有效设置自动保存到 `%LOCALAPPDATA%\ReplayGuardian\config.json`。配置损坏或无法读取时关闭自动恢复，并提示原因。

故障日志保存在同目录的 `errors.log`。写入将超过 64 KiB 时，自动删除最早的完整记录，保留最近约 32 KiB，不再生成备份日志。旧版本留下的 `errors.previous.log` 可手动删除。

自启使用当前用户的 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\ReplayGuardian`，不需要管理员权限。移动 EXE 后请重新关闭、打开自启设置以更新路径。卸载时先关闭自启，再退出程序并删除 EXE；需要清理配置时可自行删除上述配置目录。

## 构建

开发需要 Rust stable MSVC 工具链、Visual Studio C++ Build Tools 和 Windows SDK。用户运行发布版不需要安装 Rust 或额外 VC 运行时，系统 DLL 由 Windows 提供。

在项目目录运行：

```powershell
cargo test --locked
cargo clippy --locked -- -D warnings
cargo build --release --locked
```

Rust 已安装但终端找不到 Cargo 时，重开终端，或使用：

```powershell
& "$env:USERPROFILE\.cargo\bin\cargo.exe" build --release --locked
```

`.cargo/config.toml` 为 x64 MSVC 启用静态 CRT；发布配置启用体积优化、LTO、移除符号和 panic abort。提交了 Cargo.lock，用于固定本次依赖版本。直接依赖只有 `windows-sys`、`serde` 和 `serde_json`，其余是它们的间接依赖。

## 源码结构

`monitor.rs` 负责等待、方式选择和恢复协调；`recovery.rs` 负责防重复和确认规则；`detector/` 包含 MCP 和只读注册表；`ui.rs` 包含原生设置与托盘；`config.rs` 负责配置、自启和限量故障日志。

界面和监控各用一个线程。使用官方 stdio 桥接时另有一个阻塞读取线程和 NVIDIA 桥接子进程，连接销毁时结束子进程并回收读取线程。网络操作有超时，退出时可能需要等待当前请求返回。
