# G4：输入与压感

SDK API 26 的 XComponent native headers 确认：touch force、tool type、tilt X/Y 可读；
旧 XComponent 无 twist 字段。API 可读不等于外接数位板真机压力数据已经验证。

- 鼠标、触摸坐标在桥接层按 density 换算，Rust 按 egui UI zoom 再换为最终逻辑点；surface 使用真实像素 buffer geometry。
- `take_with_zoom` 同时缩放 pointer、Point 滚动与 screen_rect，保留系统 `native_pixels_per_point`。
- 鼠标/普通触摸以 pressure=-1 表示无笔；只有 pen 输入 force，鼠标在上游默认压力=1。
- pointer cancel/leave 释放按钮，window blur 释放按键与修饰键。
- 物理键盘由 `OH_NativeXComponent_RegisterKeyEventCallbackWithResult` 直接转发，处理后返回 true；ArkTS 不重复绑定 `onKeyEvent`。
- OHOS Ctrl 映射为 egui command，保留 Shift/Alt，支持快捷键 repeat、Insert/Delete/Home/End。
- Ctrl/Shift/Alt 使用 API 20 起的 `GetKeyEventModifierKeyStates` 快照（bit 0/1/2），CapsLock/NumLock 使用原生 getter。API 26 旧 XComponent key event 没有 committed-text getter，当前提供 US ASCII 字母、Shift 标点、CapsLock 与 Shift 组合和小键盘映射；IME 接入后抑制 ASCII text，保留物理快捷键；中文预编辑/提交系统接缝已实现，真机字形和候选框仍待验收。
- 原生 axis 输入区分触控板像素与滚轮角度，再转换为 egui 逻辑点。

2026-10-07 输入模块 6 个主机测试全部通过，覆盖 Ctrl+Shift+S/repeat、cancel、blur、
Insert/Delete、2 倍 UI zoom 的点击/滚动坐标、跨帧缩放切换及非法 zoom 回退。
runner 集成测试通过真实上游 shortcuts 验证 Ctrl+I 的 PNG 反色导出结果。本次三个包合计
15 个测试通过、0 失败，实际日志见 `/tmp/photocraft-ohos-tests-final.log`。

2026-10-07 API 26 MateBook Pro 真机已显示编辑器首帧，触屏点击 New 创建文档、触屏笔刷绘画通过。
首轮 Ctrl+N/Ctrl+I 无动作，确认 ArkTS XComponent `onKeyEvent` 未响应，现已改用 native
WithResult 回调，并增加仅一次的首个按键 code/action/modifier 日志；键盘修复的真机复验待补录。

PC 鼠标、触控板、实时跨屏 density 更新和界面缩放手测仍待设备验收记录。

压感结论目前是「API 已存在，真实硬件数据未验收」，不能写成「不可得」或「已通过」。

2026-10-08 平台扩展主机回归：真实文档 Type 与 egui TextEdit 的中文 preedit/commit/cancel、
UTF-16 surrogate 选区与连续删除、文字与 RGBA 图片剪贴板均通过。剪贴板只因显式 Copy/Paste
请求访问系统，入站使用 64 项 `try_send` 有界消息，回调不等待 Rust UI worker。
后台标题栏 X 使用带 ID 的 closeState acknowledgement，通过原未保存提示再决定退出；
Save cancel/error 保留文档/窗口，实际发布成功后才输出最终 close。
文档文字引擎单独注册 `/system/fonts` 的 HarmonyOS Latin/SC，缺字时只增加一个 Noto CJK
系统 collection；host 实测 CJK glyph ID 和文字 raster alpha 非空。上述是主机结论，
原生候选框、系统 clipboard permission/PixelMap 和鸿蒙中文字形仍由下一包真机验收。
协议见 [platform-protocol.md](platform-protocol.md)。

触控板 pinch 另接 `craft_zoom(factor)`：Native把SDK从手势开始的累计scale换为每次
update乘数，Rust校验有限正值并夹在0.1–10，进入 egui Event::Zoom。主机直接运行
RawInput→Context::zoom_delta 验证放大/缩小及非法值忽略；实际双指缩放仍待真机手测。

下一包改用用户触发的可见系统 PasteButton 临时授权，不声明受限 READ_PASTEBOARD。
Rust `clipboardAuthorize{id}` 绑定已有请求与原 editor/session，允许 native IME 暂时
detach，但不允许更换 widget/document/session 或 focus generation 后完成粘贴。
真实 TextEdit 的「甲😀乙」选中 emoji、预编辑取消、粘贴「中文」结果为「甲中文乙」；
系统取消与迟到结果、同 widget 重新聚焦后的旧请求拒绝均有通过的主机回归。
系统 PasteButton/原生 IME/实际中文外观仍待新包安装后的设备验证。
