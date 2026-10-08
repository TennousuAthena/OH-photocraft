# G3：PhotoCraft egui 手动循环

上游 `PhotocraftApp` 生产代码只消费 eframe 的 App、Frame、egui_wgpu、wgpu 接口。
原版 Frame 有 public `_new_kittest()`；窗口后端才是交叉编译阻断点。

本地 `vendor/eframe` 保留所需应用回调，原 desktop/web 项目仍使用原版 eframe。
上游 4337a62 未修改，完整的工具栏、画布、图层和菜单来自同一 `PhotocraftApp`。

帧序：原生输入排队 → `raw_input_hook` → `Context::run_ui` 的首轮调用 `logic` → `ui` →
tessellate → egui-wgpu 更新纹理/缓冲 → GLES render pass → Queue::present。
`Context::run_logic` 只适用于隐藏窗口；它不解释新一帧事件，不能替代 visible pass 内的 logic。

纹理 delta 按 egui 0.36 的所有权规则消费，重建窗口保留 renderer；重复 UI layout pass
不重复派发引擎命令。每次系统帧回调按 egui 的 repaint 请求和 delay 决定是否绘制。
帧外的延迟 repaint 请求保存在共享截止时间中；回调锁在 `run_ui` 前释放，避免 egui 回调
重入死锁。Lost 重建 native surface，Outdated 重新配置，均保留 GPU 设备与纹理。

2026-10-07 主机测试已通过：headless 真实上游 UI 产生绘制 shapes；PNG 打开 → OHOS
Ctrl+I 事件 → 反色 → 导出得到红像素的青色反色结果，同时保留工作文档原路径与未保存状态。
eframe 的应用回调测试也通过。上述用例属于本次 15 个通过测试，实际日志见
`/tmp/photocraft-ohos-tests-final.log`。

首个 UI 帧记录要求 shapes 非空且 surface 成功呈现，重建窗口时重置 `ui_presented`。

2026-10-08（北京时间）设备验证已确认：签名 HAP 安装/启动成功，完整上游 UI 在
`Gl; Maleoon 916` 上完成首帧呈现；触屏新建 `1920 × 1080` 文档并绘制笔刷 stroke 正常。
系统保存选择器完成 PNG 导出，页面状态显示「PNG 已保存」。native 键盘修复包重新安装后，
Ctrl+N 确实打开 New Document 对话框；此前未保存的测试画布自动恢复，状态显示
`Recovered 1 document`，原笔刷内容保留。

首帧与安装/启动证据目录：`logs/device-smoke/2026-10-07T16-20-07-385Z`，见
`hilog.txt`（本地日志，未随仓库发布）。设备交互截图保存在
`logs/device-validation/`。设备 Ctrl+I 与 PNG 重开正在复验，其结果另行记录。

当前结论是该 PC 上真实 UI 首帧、触屏编辑、保存选择器和未保存文档恢复已通过；
性能指标、中文 IME、专业完整工作流及数位板压感尚未验收。

2026-10-08 平台包增加每个异步输入后的真实 `run_ui` pass，立即刷新 IME 的文字/UTF-16
选区快照；中间 FullOutput 用 append 合并 texture deltas，只绘制最新 shapes，保持 egui
纹理消费规则。文件保存完成会在无 surface/VSync 时也刷新 Close 状态，后台标题栏关闭
不会停在已经成功的保存之后。原上游 checkout 保持 clean，改动由 UI overlay patch 重放
逐字节校验。主机输入与关闭结果不代替下一包真实系统 IME/clipboard 验收。
