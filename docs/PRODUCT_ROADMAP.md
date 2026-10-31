# 产品改进与验收路线

目标是可解释、可验证、可恢复的本机维护，不以开关数量或“提速百分比”为目标。

## 当前实现中的优先改进

- 恢复确认：单条、组、全部恢复和清空日志均核对确认快照；失配要求刷新。内部事务补偿不使用用户确认入口。保护针对进程内日志锁，不承诺阻止外部进程直接改写文件。
- 实验比较：保留原始历史，排除供电来源未知、混合或前后变化的实验；相同供电不等于相同工作负载，观察结果不能证明优化导致变化。
- 异常时间线：完整覆盖事件时间范围，起点前后分别标负值和正值，同时提供可访问文本。

## 软件清单与更新观察（只读首版已实现）

设置页提供显式确认后的 WinGet `list --upgrade-available` 查询；不会运行安装、升级或卸载操作，也不会自动接受源协议。

- 仅从当前用户的 Windows App Installer 包中解析 WinGet，并核对 App Installer 包名及受保护的 WindowsApps 路径；不回退到 PATH。
- 参数固定，禁用交互提示，90 秒超时、单飞并发和 512 KiB 显示上限。缺失、源协议要求、离线、超时、非零退出、空输出或超限均作为失败，绝不报告“无更新”。
- WinGet 表格受系统语言和终端布局影响，首版展示本地原始输出，不解析更新数量、不宣称完整覆盖；用户须自行查看其结果。
- 查询可能联网并更新 WinGet 源缓存，WinGet 也可能按已配置源行为发送查询或包标识。界面事前说明；结果只暂存于设置页内存，不进入 AeroTune 日志或持久存储，AeroTune 不单独上传结果。

剩余验收：在不同 Windows 版本/语言验证 App Installer 包定位、协议未接受、断网与超时；确认真实源警告的退出码和输出语义。若后续要解析结构化清单，应使用受支持的 WinGet API/SDK 并独立审查权限、来源和维护成本，不能从本地化表格猜测。

## 硬件传感器

优先已有 Windows/厂商数据来源，不默认引入内核驱动或要求管理员运行整个应用。

- 温度、负载和显存必须有明确设备或聚合范围；禁止把某张卡的温度与多卡聚合指标伪装成同一设备。
- 不可用、无权限、设备缺失和陈旧数据不能当作零值。
- 测试多 GPU、休眠唤醒、异常范围、采集超时和来源切换。

## 发布与安装

源码测试通过不等于安装验收。用户已授权 early-access GitHub 发布：v1.5.10 作为带生产公钥的手动引导版，v1.5.11 用应用内更新验收真实 GitHub feed。生产密钥位于受保护的 GitHub Environment；release builds 启用更新门。GitHub redirect/system-proxy 将在 .11 验收，UAC 取消、并发、安装中断/占用和修复仍未验证，因此不代表通用 GA。标准 Tauri 路径不提供自定义回滚机制。

## 参考项目

借鉴产品思路不意味着复用其代码；实际复用前须审核各项目许可证、依赖和权限模型。

- [System Informer](https://github.com/winsiderss/systeminformer)：进程和资源观察。
- [WinUtil](https://github.com/ChrisTitusTech/winutil)：软件维护与设置组织。
- [Libre Hardware Monitor](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor)：传感器支持范围。
- [Sniffnet](https://github.com/GyulyVGC/sniffnet)：网络观察与隐私边界。
