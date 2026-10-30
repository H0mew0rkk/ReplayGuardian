# Replay Guardian

一个 Rust 编写的 Windows 10/11 x64 托盘工具。监控 NVIDIA 即时回放，在确认关闭后尝试恢复，再重新读取状态确认结果。

当前版本 0.1.1 已更新原生设置界面：分组布局、状态配色、系统字体及直接暂停入口。升级前请先从托盘退出旧版，再运行新版 EXE；原有配置继续使用。详情见 [界面更新与验证](docs/UI-0.1.1.md)。

使用 Win32 原生窗口、托盘、注册表通知和 WinHTTP。不使用 Electron、Tauri、WebView 或异步运行时，不需要管理员权限。

## 使用

编译后运行 `target\release\replay-guardian.exe`，或使用交付目录中的 EXE。普通启动显示设置窗口；关闭窗口只隐藏到托盘。双击托盘图标重新打开设置，右键可以暂停、恢复监控或退出。

默认自动选择检测方式、十秒检测间隔、开启自动恢复。自启和恢复成功通知默认关闭。**监控无法判断你是否主动关闭了即时回放；需要保持关闭时，请先暂停监控或关闭自动恢复。**

### 建议优先使用 MCP

需要 NVIDIA App 11.0.9.5xx 或以上，启用 NVIDIA App 的 MCP 访问和 In-Game Overlay。系统托盘中可查看当前实际采用的检测方式，设置下拉框显示你的选择。

- 自动：优先 MCP，其次经过本机校验的注册表事件，最后注册表轮询。
- NVIDIA MCP：只使用官方接口，不可用时显示未知和错误。
- 注册表轮询：按 5、10、30 或 60 秒读取状态。
- 注册表事件：变化时重读，另有健康检查和恢复重试，不依赖下一次变化才能重试。

MCP 使用本机 HTTP；HTTP 不可用时按需使用 NVIDIA 安装目录里的官方 stdio 桥接程序。不会替你打开 NVIDIA MCP 开关或提高访问权限。现代协议 `2026-07-28` 和旧协议 `2025-11-25` 的处理流程分开；不声称支持所有历史协议。

“测试检测方式”只检查连接、状态读取、明确开启工具是否可见、注册表候选值和事件订阅，不主动切换回放。

### 注册表必须先校验

NVIDIA 没有为这些内部注册表值承诺稳定接口。候选路径：

```text
HKCU\Software\NVIDIA Corporation\Global\ShadowPlay\NVSPCAPS
值名：{1B1D3DAA-601D-49E5-8508-81736CA28C6D}
类型：四字节 REG_BINARY
开启：01 00 00 00
关闭：00 00 00 00
```

不能用 `DwmDvrEnabledV1`、`IsShadowPlayEnabled`、`RecEnabled` 等配置值代替运行状态。本机实测这些值在回放关闭时仍为 1。

点击“校验注册表”，按提示在 NVIDIA 中手动开启、关闭、再开启，每次记录状态。校验期间自动恢复暂停；取消校验会恢复正常监控。校验记录绑定当前用户及 NVIDIA 安装文件指纹，文件变化后需要重新校验。即使校验通过，仍需验证受保护内容、Overlay 重启和驱动更新等情况。

未校验、值缺失、类型或编码异常、Overlay 未运行时，注册表检测不会触发恢复。程序从不写入 NVIDIA 的状态注册表。

### 恢复及通知

- 优先向官方接口发送 `enable: true`。官方拒绝访问或明确报出保护限制时，不使用快捷键绕过。
- MCP 不可用且注册表通过校验后，才考虑 SendInput 备用快捷键。默认 `Alt+Shift+F10`，可设置 Alt/Shift/Ctrl 与字母、数字或 F1–F24，必须和 NVIDIA Overlay 保持一致。
- 快捷键前多次确认关闭，并检查 Overlay 和按键状态。状态未知时不发送；输入部分失败时不立即补发。
- 一次恢复后进入十五秒验证窗口，不重复发送操作。失败按基础重试间隔退避，最高五分钟；未知状态先恢复读取。
- 成功通知需要你开启该设置，并且程序主动尝试过恢复，随后在验证窗口内重新读到开启状态。正常启动、原本已开启和失败都不通知。Windows 可以抑制通知展示。

快捷键是切换操作，先读取再发送仍不能完全消除竞争风险。注册表也可能残留旧状态，因此建议使用官方 MCP。程序不关闭其他应用，不修改 DRM、Desktop Capture 或 NVIDIA 的权限设置。

## 配置与自启

有效设置自动保存到 `%LOCALAPPDATA%\ReplayGuardian\config.json`。配置损坏或无法读取时关闭自动恢复，并提示原因。故障日志在同一目录中，正常检测不持续写日志。

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

## 诊断和资源测量

```powershell
# 仅读取一次状态并写入指定文件，随后退出
.\target\release\replay-guardian.exe --diagnose .\diagnostic.txt

# 本次运行禁用自动恢复、自启修改和注册表校验，不保存设置
.\target\release\replay-guardian.exe --safe

# 从托盘后台启动，按需创建设置控件
.\target\release\replay-guardian.exe --background

# 先退出已有实例；只读后台测量十分钟，结束后关闭本次测试进程
.\scripts\measure.ps1
```

测量脚本记录工作集、私有内存、CPU 时间、线程和句柄，并把结果写入 `docs\validation\local`（本机输出，不提交）。CPU 同时给出相对单核和整机的比例。不调用清理工作集 API，也不把 EXE 大小当作内存占用。

**10 MB 常驻工作集是优化目标，不是当前版本的保证。** 0.1.0 的十分钟后台实测工作集为 14.73–15.49 MiB，仍然超标；平均整机 CPU 为 0.003036%。0.1.1 未重复十分钟采样。历史实测、真实恢复结果及未完成的验证见 [验证记录](docs/VALIDATION.md)。

## 源码结构

`monitor.rs` 负责等待、方式选择和恢复协调；`recovery.rs` 负责防重复和确认规则；`detector/` 包含 MCP 和只读注册表；`ui.rs` 包含原生设置与托盘；`config.rs` 负责配置、自启和限量故障日志。

界面和监控各用一个线程。使用官方 stdio 桥接时另有一个阻塞读取线程和 NVIDIA 桥接子进程，连接销毁时结束子进程并回收读取线程。网络操作有超时，退出时可能需要等待当前请求返回。设置窗口关闭不会结束监控进程；同一用户会话只运行一个实例。

可行性依据与设计：[实施前方案](docs/DESIGN.md)。

## 公开源码与隐私

版本标签对应各版源码归档；整理时统一了文档路径和隐私规则，未修改程序功能。历史测试输出中的本机路径、进程编号和注册表指纹已脱敏，测试结果与性能数值保留。`target/`、`dist/`、本地配置、凭据和新增测量输出不提交。分享诊断或压缩包前仍需单独检查内容。
