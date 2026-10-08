# 剪贴板授权与安装修复

2026-10-08：package #4 的签名与 HAP 编译通过，但真实 2in1 安装失败，BMS 返回 `9568289 grant request permissions failed`，指定 `ohos.permission.READ_PASTEBOARD`。这不是普通运行时弹窗可以解决的问题。

[官方访问权限指南](https://raw.githubusercontent.com/openharmony/docs/master/zh-cn/application-dev/basic-services/pasteboard/get-pastedata-permission-guidelines.md)说明，READ_PASTEBOARD 是受限 user_grant，需要先获得 ACL，再声明及请求用户授权；[安全粘贴指南](https://raw.githubusercontent.com/openharmony/docs/master/zh-cn/application-dev/security/AccessToken/pastebutton.md)提供不声明该权限的 PasteButton 路线。安全按钮必须可见、清晰，临时授权在灭屏、后台或退出时终止。已安装 API26 SDK 的 `ets/component/paste_button.d.ts` 定义默认可见控件和 `PasteButtonOnClickResult.SUCCESS`，未授权时不得读取。

当前实现移除 manifest 的 READ_PASTEBOARD 和 AtManager 请求。用户显式发起 Paste 时，先调用一次 getData：官方服务识别真实 Ctrl+V，并给对应前台应用提供短期读取授权。仅权限错误 201 才显示默认 PasteButton 和 Cancel；SUCCESS 后重试读取。其他错误保留实际原因，不诱导额外授权。没有常驻按钮、透明控件、模拟点击或空闲剪贴板读取。结果继续用原 request id 完成，不改变文档归属。[官方 Ctrl+V 权限检查](https://github.com/openharmony/distributeddatamgr_pasteboard/blob/master/services/core/src/pasteboard_service.cpp#L951)

显示前发送 `clipboardAuthorize{id}`，Rust 只允许同一待粘贴请求的合法短时平台失焦，仍核验文档与 TextEdit 会话；具体见 [平台协议](platform-protocol.md)。Cancel、页面隐藏、后台、页面销毁和标题栏关闭取消请求。读取中取消后，晚到内容不提交，PNG 清理。授权失败或系统读取失败会进入原 UI 错误路径。

[PasteAccessRequest](../harmonyos/entry/src/main/ets/platform/PasteAuthorization.ets) 的 [6 项 host 测试](../tests/paste_authorization.test.cjs)覆盖未授权不读、重复点击只读一次、取消后晚到文本/PNG不交付、授权失败和读取错误；隔离 CompileArkTS 通过。真实安全按钮授权、图片/文字粘贴与重新进入后台后的授权有效期，仍须在修复包真机验收。

人工测试发现的图片未导入问题来自缓存目录不一致：ArkTS 原写入 `cacheDir/PhotoCraft/Clipboard`，Rust 只接受 `cacheDir/Clipboard`。现已统一到后者；延迟 PixelMap 和剪贴板授权的文件 URI 图片均先检查尺寸，再生成无损 PNG。匹配请求发生读取或文件校验错误后会释放 pending，允许下一次粘贴；旧回执不能消耗新请求。画布粘贴没有可用图片时明确报错，避免误用旧内部图片。本次修复交由用户使用物理 Ctrl+V 验收，宿主与首帧检查不能替代这个结果。
