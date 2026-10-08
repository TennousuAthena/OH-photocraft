# 原 Load Stack / Contact Sheet / Statistics 文件夹输入

2026-10-08 #8：ArkTS 接线已冻结，API26 isolated CompileArkTS 2.197 s、完整隔离构建 3.938 s 成功；本批 folder helper/coordinator/平台与清单测试44项通过，完整 ArkTS host 合集105项通过。Rust 配套 7 项原菜单/真实图像/目标与清单测试通过，完整回归由 Rust agent/root 独立冻结。本批未运行 shared HAP/sign/install；真正商业 HarmonyOS PC 的普通 FOLDER picker、持久READ授权与目录复制层级仍待 root #8 专用 provider fixture 验证。Batch/Processor 多输出与目录发布没有接到本轮入口。

Rust 原表单生命周期、目录验证与七项真实行为用例保留于 [Rust 目录输入验证](folder-input-rust-validation.md)；该记录包含完整67项 host、strict clippy、fmt、overlay 与 wasm 检查结果。

## 冻结 JSON 字段

沿已有 `takePlatformOutput` / `platformInput`，无新 C ABI：

```json
{"kind":"folderImport","id":7,"target":"opaque-original-dialog-field-doc-generation","intent":"file.scripts.loadFilesIntoStack","scope":"topLevel","copyLayout":"sourceBasenameChild","limits":{"maxFiles":500,"maxBytes":1073741824}}
{"kind":"folderCancel","id":7,"target":"opaque-original-dialog-field-doc-generation"}
{"kind":"folderProgress","id":7,"target":"opaque-original-dialog-field-doc-generation","processedBytes":4096,"totalBytes":8192,"filesDone":0}
{"kind":"folderComplete","id":7,"target":"opaque-original-dialog-field-doc-generation","result":"success","copyLayout":"sourceBasenameChild","ownerRoot":"/sandbox/PhotoCraft/Documents/folder-imports/7-a1B2c3","resolvedRoot":"/sandbox/PhotoCraft/Documents/folder-imports/7-a1B2c3/contents/actual-source-name","manifestPath":"/sandbox/PhotoCraft/Documents/folder-imports/7-a1B2c3/manifest.json","bindingId":"7-a1B2c3","error":""}
```

ID 是非负 JS 安全整数；同 ID 不重新发起导入，即使 target 被改。首批 intent 白名单为 `file.scripts.loadFilesIntoStack`、`file.automate.contactSheetII`、`file.scripts.statistics`。`scope:'topLevel'` 保持原引擎本层输入语义；完整复制/验证仍是递归，子目录和非图像都计入500文件、1GiB、32层、4096项限制。`target` 由 Rust 捕获原 dialog ID、字段、generation、原 document，不让 shell 根据当前 active tab 重建。

completion(cancel/error)通常 paths/binding为空；仅 sandbox 清理失败时保留安全 ownerRoot/binding 并返回可见 error，不能谎称取消已成功清理。manifest 严格 `{version:1,entries:[{relativePath,kind,bytes}],fileCount,totalBytes}`，大小最多1MiB，路径相对于准确 resolvedRoot，不含 URI。Rust 重新验证 owned containment、祖先/实际根/manifest普通类型、路径/count/bytes与请求身份，再调用原引擎 `list_images` 保持原 OPENABLE、UTF-8完整path排序及本层语义。不能根据 JS manifest 的 UTF-16顺序改变原业务排序。

## 明确复制布局与完整 owner

normal SDK 将目录 URI copy 到空的独有 `ownerRoot/contents`。本批明确指定 `sourceBasenameChild` 合约：固定官方当前 NAPI→DFS目录实现与实际官方 unit tests 已证实会在现有目标目录下创建源basename child。helper只枚举这个已owned sandbox container，严格要求恰好一个真实普通目录 child，拒绝空、flat文件、多根、链接和越界名字，再用这个实际 child 作为 resolvedRoot；不解析外部 URI、不从URI猜名称、不尝试其他跳层或扁平fallback。证据见 [目录工作流计划](folder-workflow-plan.md) 与 `logs/regression/folder-layout-review-20261008`。

这不是任意树只有一个子目录就自动展开的通用启发式。商业本地 provider 本轮 fixture 需验证该显式合约；未知或结果形状不符合的 provider 明确失败，不执行业务。原文档尚未承诺所有provider层级一致。ownerRoot保留整个mkdtemp job，resolvedRoot只能是contents下明确普通child；二者职责分开。Rust不自行遍历猜单child，只验证shell给的准确路径。

`source.json` 私有记录 requestId、target、bindingId、copyLayout及原URI授权，不传URI给Rust，也不在诊断log中打印URI或用户文字。releaseRetained必须匹配请求id、target和安全jobbasename，并重新 lstat检查 filesDir 以下所有祖先及普通source标记；只删完整本次owned job，不跟随链接，不作用于已移交结果。链接在逐项清理时只unlink它自身。

## 非阻塞调度、取消和所有权

`FolderImportCoordinator` 是独立单任务槽，不把长 copy await到普通 `PlatformBridge.processEvents/running` 中。平台直接分派 folderImport/folderCancel，200ms内部轮询保持控制消息可达；SDKcopy Promise仍未完成时可响应匹配id+target的Cancel、closeState及safeclose。进度仅保留最新值，最多每200ms提交；队列忙丢的是过期进度，终态保持到accepted，不让大量进度淹没completion。

系统folder picker引起ability background是正常授权步骤，不撤销folder任务；原Index普通foreground轮询停止后，任务自己的控制timer继续。真正onPageHide设置page不可见并取消任务，隐藏时仍可处理控制/终态，但跳过已隐藏组件geometry/IME应用；aboutToDisappear/dispose取消并等待helper异步安全清理晚到结果，不再次入队。系统picker无公开主动关闭接口，取消标志需等picker正常返回；copy阶段用TaskSignal尝试取消，但必须等provider Promise settle后才删private partial副本。成功授权close同样先cancel并await正在运行的provider，再terminateSelf；不绕过原unsaved flow。

文件picker/拖拽/Paste授权与folder任务使用同一实际文件操作可用性门禁：冲突时明确error或延后原file request，不启动第二个picker。Index不把folder复制标成阻止platform轮询的isBusy；focus恢复只在页面可见、前台且本次folder任务完成交付后发生。额外title/toolbar/ArkTS业务form没有引入，原PhotoCraftUI仍完整XComponent。

`platformInput(true)`仅表示worker有界队列接受，但这一步原子移交成功副本所有权。其后ArkTS不会删除目录；matching completion若已过期，由Rust验证owner并清理，不更新新dialog/文档。unknown/duplicate完成不能删除已接收仍使用的树。`false`只表示未入队，shell继续持有完整job和completion供下一次poll重试，不走无法观察ownership的普通字符串completion队列。pending成功但尚未accepted时的Cancel可清理仍owned副本，随后仅发cancel；accepted之后的旧Cancel不再有删除权。

本helper/coordinator不确认原业务命令执行，也不清dirty/path/revision。成功仅将安全本层图像数组设置回原表单；用户仍确认原参数。无顶层支持图像或只有nested图像明确报错并保留原表单，不暗中改成递归执行。source URI始终只读，失败不撤销可能共享的持久grant。

## 验证与 root fixture

`tests/folder_import.test.cjs` 19项执行生产ETS helper、coordinator及PlatformBridge，真实临时PNG/PSD/Unicode/两层subdir/marker与SDK-contract shim：actualRoot相对清单、flat拒绝、permission/copy错误、source links、id/target清理边界、false/true所有权、重复/晚到、独立cancel、disposal、进度coalesce/NaN、忙碌filepicker、strictscope/layout/intent/limits、polling不因copy停、background不取消、pagehide不取geometry、cleanup失败保留、safeclose等provider。`tests/folder_manifest.test.cjs`25项含6个明确container/source child契约回归，原路径/限额/链接防护继续全绿。完整105项还覆盖既有publication rollback、SDK模板、clipboardAuthorize和FolderPublication；raw日志与八个文件完整hash归档在 `logs/regression/folder-input-20261008`。

root #8 将从实际三个原菜单点击Browse，选专用目录（顶层PNG/PSD、两层子目录、marker）：确认复制布局与业务只取本层、数目排序/Stack层像素/ContactSheet/Statistics结果；再验证Cancel、背景、旧表单/文档代际和重复完成、grant失效、provider拒绝/空间不足/links/caps。真机结果前不写provider PASS，也不把本地结果外推远端provider。root负责最终Rustarchive→Native HAP→signed/device验证，本次仅隔离compile。
