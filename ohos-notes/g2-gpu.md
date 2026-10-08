# G2：原生 GPU 与窗口

原计划所述 `Instance::create_surface_ohos` 不是 wgpu 30.0.1 的实际公开 API；此版本
Vulkan raw-window-handle surface 路径未接 OHOS。已有的 OHOS 路径在 GLES 后端：

- [wgpu-hal 30.0.1 GLES EGL 实现](https://github.com/gfx-rs/wgpu/blob/v30.0.1/wgpu-hal/src/gles/egl.rs)
- `SurfaceTargetUnsafe::RawHandle` + `OhosDisplayHandle` + `OhosNdkWindowHandle`

本实现强制 `Backends::GL`，使用 SDK XComponent surface callback 传入的 OHNativeWindow。
C++ 在 Rust 使用前 retain，destroy RPC 完成后才 release。Rust 独立渲染线程持有 egui、
GPU 和应用状态，surface 重建复用 renderer 与已上传纹理。

PhotoCraft 文档暂用上游 CPU 合成器，egui UI/纹理由 GLES 显示；这不代表 compose.wgsl 的
GPU 合成已通过。Vulkan、新设备上的 GLES 和完整 compute compositor 是后续独立验证项。

2026-10-08（北京时间）已在 API 26 的 `2in1` PC 完成设备验证：

- 签名 native HAP 安装成功，`EntryAbility` 启动成功。
- 实际 adapter 为 `Gl; Maleoon 916`。
- XComponent surface 为 `2432 × 1298` 像素，density 为 `1.9`。
- Rust 渲染线程记录 `PhotoCraft UI first frame presented`，设备显示完整 PhotoCraft UI。
- 触屏新建 `1920 × 1080` 文档、笔刷绘制和系统保存选择器导出 PNG 已完成。

设备 smoke 记录目录：`logs/device-smoke/2026-10-07T16-20-07-385Z`。其中
`install.txt`（本地日志，未随仓库发布）、
`start.txt`（本地日志，未随仓库发布） 与
`hilog.txt`（本地日志，未随仓库发布） 分别保留安装、启动、
adapter/surface/首帧证据。目录时间采用 UTC，设备日志时间为北京时间。

运行时诊断写入 `<filesDir>/photocraft-runtime.json`，hilog 的 PhotoCraft 标签记录真实 adapter
和 `PhotoCraft UI first frame presented`，不能把 XComponent 的 first callback 当成成功出图。

本轮已验证该设备的原生 GLES 首帧及基础交互。帧率、延迟、4K 性能、resize/最小化恢复压力
测试和完整 GPU 文档合成仍待测量；本记录不代表整套 M0 或完整专业移植已通过。
