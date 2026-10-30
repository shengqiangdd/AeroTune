# AeroTune · Windows 系统优化中枢 — 编码前置设计文档

> 版本 v0.1 · 2026-10-11 · 状态：前置检查通过，进入编码

---

## 1. 项目定位

一款原生级 Windows 系统优化软件，围绕三大场景提供一键式调优：

| 场景 | 目标 | 核心手段 |
|------|------|----------|
| 🎮 游戏流畅 | 帧率稳定、延迟降低 | 高性能/卓越电源计划、关闭 GameDVR、HAGS 开关、内存整理 |
| 🔋 日用省电 | 笔记本续航延长 | 节能电源计划、电池状态监测、电池报告、传递/后台负载控制 |
| 💻 编码顺滑 | IDE/编译响应优先 | 平衡计划、前台响应优先（Win32PrioritySeparation）、内存整理 |

附带能力：实时系统监控（CPU/内存/GPU/磁盘/网络/电池）、进程管理（结束/优先级/挂起）、
启动项管理、垃圾清理、网络优化（DNS/TCP）、**变更日志 + 一键还原**（安全兜底）。

## 2. 前置环境结论（实测）

| 项 | 结果 | 结论 |
|----|------|------|
| Node | v24.18.1 / npm 9.2（npmmirror 源）/ pnpm 9.7 | ✅ |
| Rust | 1.97.1 stable（rustup @ F:\dev） | ✅ |
| 默认工具链 | `stable-x86_64-pc-windows-gnu` | ⚠️ Tauri 要求 MSVC |
| MSVC / Win SDK | 未安装（vswhere 无 VC 工具集） | ⚠️ 正在后台安装 VS2022 Build Tools → F:\VS2022BuildTools（`--nocache`，最小组件集） |
| WebView2 | 154.0.4258.62 | ✅ |
| 磁盘 | **C 盘仅剩 5.7G（97%）**，F 盘 158G | ⚠️ 所有重负载安装一律避开 C 盘；构建 target 目录放 F 盘 |
| 当前权限 | 管理员 | ✅ 安装类操作可静默完成 |

**工具链策略**：项目内 `rustup override set stable-x86_64-pc-windows-msvc`（仅影响本项目，不动全局 gnu 默认），
Cargo 全局构建目录设为 `F:\dev\cargo-tgt\aerotune`（CARGO_TARGET_DIR）避免撑爆 C 盘。

## 3. 技术选型（对比结论）

| 候选 | 评价 | 结论 |
|------|------|------|
| **Tauri 2（Rust + WebView2）** | 体积小（~8MB 级）、内存占用低（对"优化软件"自身即是口碑）、Rust 原生调 Win32（进程/电源/注册表）最干净、系统托盘/Mica 等前沿能力 | ✅ **选用** |
| Electron | 生态成熟但运行时 ~150MB/200MB+ 内存，与"省电优化"定位冲突 | ✗ |
| WinUI 3 (C#) | 原生 Fluent，但工具链重、打包复杂、前端迭代效率低 | ✗ |

**前端栈**（2026 现行主线）：React 18.3 + TypeScript 5.7 + Vite 6 + Tailwind CSS 4（`@tailwindcss/vite` 原生插件，
无 PostCSS 配置）+ lucide-react 图标。图表手写 SVG（环形仪表 + 面积走势图），不引入重型图表库。

**Rust 侧依赖**：`tauri 2`、`sysinfo 0.33`（CPU/内存/进程/磁盘/网络）、`winreg`（注册表）、
`windows-sys`（进程优先级/挂起/内存整理/电源状态/管理员判断，raw API 零包装）、`walkdir`、`regex`。

## 4. 架构

```
┌─────────────────────── AeroTune (Tauri 2, Win11) ────────────────────────┐
│  WebView2 前端 (React+TS+Tailwind4, 自绘深色 UI)                          │
│  9 页面: 仪表盘/游戏/省电/编码/进程/启动项/清理/网络/设置                    │
│        ▲ listen("stats" 1Hz 推送)      invoke(command) ▼                  │
├──────────────────────── Rust 核心 (src-tauri) ────────────────────────────┤
│  stats.rs     监控总线: sysinfo 轮询线程 → 缓存 + 事件推送                  │
│  processes.rs 进程枚举/结束/优先级/NtSuspend/NtResume/EmptyWorkingSet      │
│  power.rs     powercfg 计划管理 + GetSystemPowerStatus 电池               │
│  tweaks.rs    注册表调优(GameDVR/HAGS/PrioritySeparation) + 模式编排       │
│  startup.rs   HKCU/HKLM Run + 启动文件夹, 禁用/恢复                        │
│  cleaner.rs   临时目录/浏览器缓存/更新缓存/回收站 扫描与清理                │
│  network.rs   网卡/DNS/TCP 自适应调优 (PowerShell JSON 桥, 避开本地化解析)  │
│  admin.rs     管理员检测/提权重启/打开目录                                 │
│  journal.rs   变更日志: 每项可逆调优记录 old→new, 支持单项/全部还原          │
└───────────────────────────────────────────────────────────────────────────┘
```

**设计原则**
1. **可逆优先**：所有注册表/网络/电源级修改先读旧值记入 `changes.json`（AppData），UI 可单项/一键还原；进程结束、文件清理等不可逆操作不进日志、不静默执行。
2. **权限分级**：启动时检测管理员状态；HKLM/系统级操作需要管理员，UI 提供一键提权重启（PowerShell `-Verb RunAs`）。
3. **本地化免疫**：凡涉及系统命令输出的解析，一律走 PowerShell `ConvertTo-Json` 或 GUID 正则，不依赖中文/英文文案。
4. **无窗口闪黑**：所有 `Command` 调用带 `CREATE_NO_WINDOW`。

## 5. Tauri Command API（前后端契约）

| 命令 | 说明 |
|------|------|
| `get_system_info` | OS/型号/CPU/架构/管理员状态 |
| `get_stats` | 一次监控快照（事件 `stats` 同构 1Hz 推送） |
| `list_processes` / `kill_process` / `set_priority` / `suspend_process` / `resume_process` / `trim_memory` | 进程域 |
| `list_power_plans` / `set_power_plan` / `battery_report` | 电源域 |
| `apply_mode(mode)` / `get_mode_state` | 场景模式编排（game/power/dev/none） |
| `set_tweak(key, on)` | 单项调优开关：gamedvr / hags / priority_boost |
| `list_startup` / `toggle_startup` | 启动项 |
| `scan_junk` / `clean_junk` | 清理 |
| `list_adapters` / `get_dns` / `set_dns` / `flush_dns` / `get_tcp_autotuning` / `set_tcp_autotuning` | 网络 |
| `list_changes` / `revert_change` / `revert_all` | 变更日志 |
| `is_admin` / `relaunch_as_admin` / `open_path` | 权限与导航 |

## 6. 风险与缓解

| 风险 | 缓解 |
|------|------|
| Build Tools 安装慢/失败 | 已后台启动；失败则重试（组件最小化 ~3G 装在 F 盘）；极端情况下回退 `windows-gnu` 尝试编译 |
| 首次 cargo 编译 ~5-10min | 后台跑 `cargo build`，期间完成前端验证 |
| powercfg 输出本地化 | GUID 正则 + `*` 活动标记，名称取末尾括号 |
| HAGS/优先级分离等 HKLM 写入 | 管理员检测 + 提权重启；写入失败返回明确错误给 UI toast |
| 挂起进程导致假死 | 仅白名单外用户主动操作，UI 二次确认；会话内记录挂起集 |
| sysinfo 0.33 API 差异 | 固定版本 `=0.33`，不随 floating 漂移 |

## 7. 里程碑

1. ✅ 前置检查与选型（本文档）
2. 脚手架 + 后端全部模块 → `cargo check` 通过
3. 前端 9 页面 → `vite build` + `tsc` 通过
4. `cargo build` 出 debug exe，启动验证
5. 交付：源码 + 运行说明（NSIS 安装包构建命令备好）
