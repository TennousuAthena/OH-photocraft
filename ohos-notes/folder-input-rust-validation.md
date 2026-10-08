# #8 Rust 目录输入验证

2026-10-08 Rust 源码已冻结，供 root 构建下一 archive。完整 wrapper / input / facade
67 项 host 测试通过（PhotoCraft 59、input 7、eframe 1），0 失败；4 包严格 clippy、
fmt check 和两个 overlay 精确重放通过。UI patch 909 行，engine patch 196 行；
原始 upstream pin `4337a6227a823a28728e68aed844feab62b3314d` 保持 clean。

协议与 ArkTS 证据详见 [folder-input-protocol.md](folder-input-protocol.md)。
本补充仅描述 Rust 端，不替代 SDK/provider 真机验收。

## 原表单与原引擎

Services 的可选 `browse_folder(Request)` 捕获原 dialog ID、field、generation、
command、active document ID 和原字段 Value。Browse 放在三个原输入字段旁；
完成只填数组，保留其他选项，不运行命令。原 OK、Enter 和 programmatic Confirm
共享 ready 条件。hook 未设置时保留原 desktop 表单行为。

字段改动、表单关闭、generation 变化、切换原文档和用户 Cancel 会发同请求 ID/target
的取消事件。copy 尚未 settle 时保留 pending；取消/过期的 matching completion 清理
自己的 verified job。未知或重复完成不能删除已经交给原表单/Actions 的输入树。
重新 Browse 的取消保留上一份已完成输入。成功目录选择不更改工作文档 metadata，
业务是否创建新文档由原确认命令决定。

Rust 按明确 `sourceBasenameChild` 契约核验返回的 root，不用单子目录 heuristic 猜层级。
路径必须在指定 imports 根内，owner 是本请求 `<id>-sixRandomCharacters` 直接子目录，
actualRoot 是 owner/contents 指明的唯一普通子目录。所有目录无链接，canonical 路径
一致；manifest 必须 owner/manifest.json，bindingId 必须 owner basename。

manifest JSON 限 1 MiB，严格 version 1 / entries / fileCount / totalBytes 字段。
独立检查全部真实条目、文件大小、重复与安全相对路径，最多 4096 条目、32 层、
500 文件、1 GiB，拒绝链接和特殊文件。核验后调用公开的原 `file_cmds::list_images`，
原函数只有两处可见性变化，原 top-level 枚举、支持扩展和完整 String 排序都没变。
JS manifest 排序不影响业务；空或仅嵌套支持图像明确失败，不执行原确认。

## 真实行为测试

7 个新用例覆盖：

- 原 Load Stack：真实蓝/红/中文文件名 PNG，本层排序、真实图层和像素，嵌套 PNG 与
  marker 不入数组；pending Confirm 不关闭表单；重复完成不删除已接受输入。
- 原 Contact Sheet II / Statistics：其余表单选项保持，确认后产生 48×16 页面 /
  Mean SmartObject；选目录本身不创建文档。
- 原 Cancel、closed dialog、字段替换、generation 改变、切文档的晚到完成均不执行。
- 清单大小不符、重复、越界、超过 1 MiB、错误 root/binding/mode 均拒绝并清理本 job。
- 链接与伪造 owner 不得删除外部 sentinel。
- 空和仅嵌套图像维持原 dirty 文档，不创建业务结果。
- 实际 `PhotocraftApp.logic/ui` 绘制原表单 Browse 与文件数量，没有内部 sandbox 路径；
  再选取消保留旧数组。

原 engine `load_files_into_stack_and_image_processor_and_batch` targeted 回归也通过。
UI/engine WebAssembly 检查使用独立 probe（webgpu backend feature），锁定相同版本，
成功。原 cms 的两处 wasm dead-code warning 和 engine 的 OPENABLE warning 保持原样，
没有因 warning 修改上游算法。该检查不声称 OHOS facade 是浏览器 runner。

日志：`/tmp/photocraft-folder-host-targeted.log`、
`/tmp/photocraft-folder-host-tests-final.log`、`/tmp/photocraft-folder-clippy.log`、
`/tmp/photocraft-folder-ui-wasm.log`、`/tmp/photocraft-folder-engine-regression.log`。
probe 源/锁位于 `/tmp/photocraft-folder-wasm-probe`，未加入产品或更改 OHOS archive。
实际 picker / provider copy 层级和取消 / UI 像素外观由 root #8 真机 fixture 验证。
