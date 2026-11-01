# AeroTune · Windows 系统优化中枢

AeroTune 是一款基于 **Rust、Tauri 2 和 React** 的 Windows 桌面系统监控与管理工具。它提供实时遥测、可解释的系统建议、场景模式，以及进程、启动项、服务、存储和网络管理功能。

> 系统优化可能改变 Windows 设置或影响正在运行的程序。请先阅读每项操作的说明；效果取决于 Windows 版本、硬件、驱动和权限，不保证提升性能或续航。

## 功能概览

- **监控与诊断**：CPU、内存、磁盘、GPU、网络与电池数据；健康概览、异常观察、智能建议和本机趋势。
- **场景与自动化**：游戏、省电、编码场景；可选的自适应引擎会按识别到的场景调整部分系统参数。自动化需由用户启用，相关边界见下文和变更记录。
- **系统管理**：进程与启动项管理、服务浏览、网络设置、存储分析、清理与系统维护工具。
- **可审阅操作**：支持的操作会记录恢复信息；执行清理、结束进程等有影响的操作前会显示确认。并非所有系统修改都可撤销。
- **软件更新观察**：可手动确认 WinGet 只读查询；查询可能访问软件源并更新本地缓存，不自动接受协议或安装软件，原始结果不保证完整识别。
- **Windows 桌面集成**：系统托盘、通知、开机启动选项，以及受限的 Windows 设置快捷入口。

功能和算法细节见[历史与版本记录](CHANGELOG.md)。隐私数据字段见[隐私清单](docs/DATA_PRIVACY_INVENTORY.md)，后续改进与验收边界见[产品路线](docs/PRODUCT_ROADMAP.md)。

## 安全与已知限制

- 自动引擎启用后可能自动修改部分系统参数；手动操作和自动操作的具体行为以应用内说明为准。执行前请确认目标、权限和潜在影响。
- 恢复日志不等于完整事务或绝对回滚保证。外部程序改动、断电、资源间部分失败等情况可能需要人工处理；仅明确标注支持恢复的记录可使用通用撤销功能。
- 清理、进程和服务操作有各自的确认与校验边界。测试通过不代表在所有硬件、驱动和 Windows 配置上均安全或有效。
- 导出的诊断报告可能包含主机名、CPU 型号等设备信息；分享前请检查并自行脱敏。
- GPU 数据取决于 Windows 性能计数器和驱动支持；异常检测属于启发式观察，不是故障判定或概率结论。

## 下载与更新状态

- Windows x64 NSIS 安装包采用 per-machine 安装方式，安装到受保护的 Program Files，安装需要管理员权限。
- 当前工作区配置版本为 **1.5.11**。Windows 安装包尚无 Authenticode 签名；Tauri Minisign 更新签名不等于发布者身份，UAC 可能显示 Unknown publisher。详见 [Windows Code Signing Policy](docs/WINDOWS_CODE_SIGNING.md)。
- v1.5.10 引导版已于 2026-11-01 在 [GitHub Releases](https://github.com/shengqiangdd/AeroTune/releases/tag/v1.5.10) 公开，并从经 Minisign 验证的 NSIS ZIP 安装；Program Files 可执行文件与卸载注册表均报告 1.5.10。当前正准备从该安装实例验收 v1.5.11 应用内更新。当前 updater 不记录每一跳 redirect；WinHTTP 直连、WinINet 代理关闭已记录，应用内更新结果待验证。取消/中断/并发/修复仍未验证，因此不代表通用 GA。详见[更新设计](docs/UPDATER.md)和[发布阻塞项](docs/UPDATER_RELEASE_BLOCKERS.md)。

## 开发

当前 CI 使用 Node.js 24.18.1、pnpm 11.8.0 和 Rust 1.97.1（MSVC）。在 Windows 开发环境中：

```bash
pnpm install --frozen-lockfile
pnpm run tauri dev
pnpm run check:release
```

构建 NSIS 安装包：

```bash
pnpm run tauri build
```

此命令生成的本地安装器不会因此自动获得 Authenticode 签名。直接运行 `cargo build --release` 时还需启用 `tauri/custom-protocol`；正常使用 Tauri CLI 可自动配置。

## 许可

本项目采用 [MIT License](LICENSE)。许可文本中的版权持有人为 `shengqiangdd`。

## 更新记录

详见[`CHANGELOG.md`](CHANGELOG.md)。
