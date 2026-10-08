# PhotoCraft #12：Revert 与 Smart Object 来源事务

本批只修改 Rust wrapper、可重放的 engine/UI overlay 和 DocumentBridge/Index 现有文件请求接线。不增加 Native ABI、权限、测试 toolbar 或编码算法；#11 不可变包及其归档不变。原始 checkout 仍固定 4337a6227a823a28728e68aed844feab62b3314d。

## 接缝与原行为

`Session.source_file` 可选回调捕获原文档的 Arc、DocId、LayerId、revision、path 和原命令参数。回调不取得可变 Session，不在系统选择器中阻塞 render worker；完成后校验同一文档、版本及路径，调用原 Revert、解码、Smart refresh、set_source 和 history。没有回调时保持原桌面/wasm行为。Revert 的外部来源存在性由平台验证，不把内部副本缺失判断为外部文件不存在。

F12 仍经过原未保存变更确认，测试对实际绘制且命名的原 Revert accessibility button 发 Click；没有调用私有 resume 或去掉提示。成功读取保留原 DocId/name/path，采用原 undoable Revert 状态且 clean；Undo 回到此前内容并 dirty。系统读取失败、取消、期间编辑或原文档关闭均不覆盖文档。

Replace Contents 和 Relink to File 沿原菜单选择新文件，捕获原 layer。Update Modified Content、Update All、Edit Contents、Export Contents 与 Convert to Embedded 对真正外部 Linked 源重新读取授权 URI。PSD 的原内嵌 linked-layer 数据仍由原 metadata 解析，不把它冒充可刷新外部来源。Edit Contents 保留原 SmartLink 的 parent/layer，并复用原 Save Contents 回写。

Update All 对同一外部源只读一次，保留原单次 undo 编辑；权限失效或损坏的源保留原 source 与缓存内容，其他合法来源仍更新，并在原 footer 报告部分失败。全部来源失败不增加 edit/revision 或成功 journal。不会把失败源的旧缓存再渲染后称为更新成功。

Export Contents 和 Convert to Linked 发布源的原格式、原 bytes，禁止仅换 suffix。独占 stage 可按目标 basename 安全重命名；普通 Export 不改父文档 dirty/name/path，成功可清其 stage。Convert 仅外部成功 ACK 后改为 Linked，取消/失败保留 Embedded；编码/发布期间新编辑使转换不应用，报告外部已发布且保留副本。Linked 成功 stage 留在 filesDir，URI 绑定只由 ArkTS 保存，资源 stage 不进入“父文档已保存”授权 journal。

## 沿用的协议

沿现有 `craft_take_file_request`、五参 `craft_complete_file_request` 与 `prepareFileSave`，不增加 C ABI。`intent` 保留原 Revert/Smart command。`kind:open`、`previousPath` 非空且 `chooseDestination:false` 复用 Recent 的 fresh authorized read；手动 Replace/Relink 为 `chooseDestination:true`。新可选 `retainSource` 决定成功 fresh import 或 Convert stage 是否加入 URI 绑定；临时 Revert/Edit/Replace/Export 读取不加入绑定并由 worker 消费后清理。普通 Open/Save 沿旧默认行为。

一次只允许一个单文件来源或保存请求；新请求不能替换已有 pending。ID 匹配和 delivered 检查阻止迟到/重复 completion 消耗新事务。文档切换后结果仍属于原 DocId/LayerId，保留当前 active tab。

Fresh import 必须是 UIAbility 的 `filesDir/PhotoCraft/Documents/imports/<owned-job>/<basename>`，拒绝链接、越界与非法路径；保留由 UIAbility 提供的根路径拼写，同时验证其 canonical 根不变，避免 `/var` 和 `/private/var` 等可信根别名误拒绝。staging 使用已验证的 canonical 根。每次来源操作最多 500 个外部源、累计 1 GiB；来源身份限 8192 bytes、文件名限 240 bytes、错误详情每项限 4096 characters。已绑定且保留的 fresh Linked 副本不在失败清理中被删除。

## 自动化边界

原同步 Scripts、Actions 与 Batch 目前不能等待系统授权后恢复后续步骤。OHOS source hook 存在时，这些入口明确拒绝本批异步来源命令并停止该步骤后续写入；Batch 逐文件 errors 保持原格式。没有 hook 的原桌面/wasm不受此限制。Print spool 和原 Script Events 不被全局关闭。**异步 script/action continuation 仍是必需的下一专项，当前错误守卫不是完整自动化移植。**

此前只读缺口清单 `file-platform-remaining-audit.md` 以 #11 源为基线；本文件说明本批已接的来源项。Slices/Assets、其他多文件输出、资源 chooser、外部链接工程的可移植打包及原 Actions 持久化仍按该清单分批处理。

## 验证

最终完整主机回归 103/103 通过：wrapper 95（新增13项实际来源业务回归）、公共 platform 7、eframe facade 1。新增测试核对真实 fresh 像素、Undo、dirty、原 DocId/LayerId、父子回写、切文档、取消/失败/新编辑/关闭、部分与全部来源失败、精确 PNG bytes、格式保护及链接祖先拒绝；不是只验证请求字段。

DocumentBridge production SDK adapter 13/13 通过，包括外部文件被改后 Revert/六类 Smart intent 取得新 bytes、撤销权限无缓存 fallback、手动 Replace/Relink仍选择新文件。独占 API26 snapshot CompileArkTS 32.206s、整体42.731s成功；这是 SDK 编译/主机测试，不等价于商业 provider 真机通过。

最终来源 targeted 13/13 在严格 lint 唯一要求的 `String.into()` 清理后再次通过。原引擎默认无平台 hook 的受影响回归共 37/37 通过：File 16、Smart Object 15、Automate 6，包含原 Revert Undo、linked source export/replace、PSD metadata、Edit Contents 回写、Script Events 和 Batch 行为。

四包 all-targets strict clippy 与 UI direct strict clippy 均 `-D warnings` exit 0；workspace fmt、修改 UI 文件 fmt check、两 overlay 独立逐字节 replay均通过，upstream保持 clean。实际 wasm32 probe通过，保留原 cms 的两个并行常量和 file_cmds 的 OPENABLE dead-code warnings。19 个 ETS 源与 API26 成功编译的独占 snapshot 逐字节一致。

最终证据保存在 `logs/regression/file-sources-20261008/`，`verification.json` 记录命令和结果，`product-source.sha256` 记录相对 #11 的 379 个产品文件，`product-source-changes.json` 列出17个改变/新增文件。产品清单本身 SHA-256 为 `28c5d13e404a7f33c077adaabf48e41f76b14e4fad5350ef2ddd0ce7a75e308b`。HAP构建、签名、安装、真机及新的不可变包仅由 root 完成；本批不执行这些步骤。所有产品源码与 overlay patch 在本次收口后冻结。
