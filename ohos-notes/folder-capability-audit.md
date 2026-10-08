# 授权文件夹能力审查与拟议协议

2026-10-08 #8 更新：目录导入已接原 Load Stack / Contact Sheet / Statistics Browse 的typed平台通道，独立200ms任务调度、id/target与副本所有权、明确 sourceBasenameChild 实际根已实现；105项完整ArkTS host checks及API26 isolatedCompile通过。真实商业provider仍待root专用fixture，不能写device PASS。详细冻结接口与限制见 [folder-input-protocol](folder-input-protocol.md)。完整树的新随机子目录发布helper也已独立实现，但尚未接业务入口，见 [folder-publication-bridge](folder-publication-bridge.md)。以下是此前能力审查和协议背景，接口版本以这两份冻结说明为准。本机SDK API26，设备2in1；PrintKit协议独立冻结。

## 可由普通应用使用的能力

- `DocumentViewPicker.select()` 与 `DocumentSelectOptions.selectMode = DocumentSelectMode.FOLDER` 可返回文件夹 URI；先检查 `SystemCapability.FileManagement.UserFileService.FolderSelection`。第一轮固定 `maxSelectNumber=1`、不启用 API26 多文件夹选项。2in1 的 `authMode=true` 需已有 `defaultFilePathUri`，用于显式重新授权。依据 [选择用户文件](https://raw.githubusercontent.com/openharmony/docs/master/zh-cn/application-dev/file-management/select-user-file.md)和本机 `@ohos.file.picker.d.ts:310/477/489`。
- `fileShare.persistPermission/checkPersistentPermission/activatePermission` 可针对文件夹 URI 请求和重新激活 READ 或 READ|WRITE；需要已声明的 normal `FILE_ACCESS_PERSIST`，以及 FolderAuthorization 能力。授权不等于任意 provider 必然支持读写，实际 open/copy 仍是结果依据。普通 select 的授权说明在官方页面不同段落有只读/读写差异，不能据其推断可以覆盖。依据 [授权持久化](https://raw.githubusercontent.com/openharmony/docs/master/zh-cn/application-dev/file-management/file-persistPermission.md)和本机 `@ohos.fileshare.d.ts:215/261/304`。
- `fileIo.copy(srcUri,destUri,CopyOptions)` 的公开 API11 合约支持文件或目录 URI、强制覆盖复制；进度回调可用，API12 的 `TaskSignal.cancel()` 可取消，API26 不使用已废弃 `onCancel()`。可将授权目录整体复制到独有沙箱目录，再在沙箱 listFile/read/解析，而不要求 Rust 理解外部 URI。跨端最多10任务、单次最多500文件；本机/远端 provider 的目录层级、空目录、文件量及覆盖行为仍须实际验证。依据 [fileIo.copy](https://raw.githubusercontent.com/openharmony/docs/master/zh-cn/application-dev/reference/apis-core-file-kit/js-apis-file-fs.md#fileiocopy11)和本机 `@ohos.file.fs.d.ts:416/461/3804/3838`。

## 尚不能据现有证据承诺的能力

本机公开 SDK 的 listFile/mkdir/rename/rmdir 仍声明 sandbox path；open、stat(API22+)和 copy 支持 URI，不能把这些能力外推到所有目录操作。`FileUri.getFullDirectoryUri()` 只能取得现有 URI 的父 URI；`getUriFromPath()` 的合约只针对 sandbox。没有公开的 parentURI + childName 创建接口。官方 fileAccess 的 createFile/mkdir/listFile 属 system API，要求 FILE_ACCESS_MANAGER，仅供 FilePicker/FileManager；normal SDK 对应 namespace 为空，不能导入绕过。[system fileAccess 参考](https://github.com/openharmony/docs/blob/master/en/application-dev/reference/apis-core-file-kit/js-apis-fileAccess-sys.md)。

因此目录发布可研究“完整沙箱树 URI→用户授权的现有目录 URI”这一个 SDK copy 能力，但它会强制覆盖，缺少逐文件目标 URI、冲突检测与既有文件备份机制，不能直接作为安全批量输出。是否在目标内新增源目录名、如何处理已有目录/同名文件/取消后部分写入，须先做独立 provider fixture 验证。不会猜 URI 拼接，也不会把用户 URI 转个人文件路径或用沙箱 AtomicFile 处理它。[官方 URI 指南](https://raw.githubusercontent.com/openharmony/docs/master/zh-cn/application-dev/file-management/user-file-uri-intro.md)也建议不解析 URI 片段用于业务代码。

可先考虑 flat batch 输出：DocumentSaveOptions.newFileNames 提交一组真实输出名字，picker 返回明确目标文件 URI 后逐个复用现有保护 publisher。SDK 只定义名字和 URI 数组，没有正式承诺相同长度/序号映射、覆盖冲突或取消语义，必须验证并按返回 basename 及数量匹配；不会把缺少的 URI 合成出来。嵌套目录暂不以该办法模拟。

## 待 root 确认的 schema 建议

建议与普通单文件 request 独立，保持 URI 只在 ArkTS。下面是设计建议，并非已冻结 ABI：

```json
{"id":1,"kind":"folderImport","intent":"library.import","limits":{"maxFiles":500,"maxBytes":1073741824},"recursive":true}
{"id":2,"kind":"folderPublish","intent":"batch.export","stageRoot":"/sandbox/staging/job-2","manifestPath":"/sandbox/staging/job-2/manifest.json","collision":"reject"}
```

导入完成：`{kind:"folderComplete",id,result,resolvedRoot,manifestPath,bindingId,error}`。resolvedRoot/manifestPath 都是验证后的持久 sandbox path；bindingId 是 ArkTS 保存的授权句柄，Rust 不获得外部 URI。清单记录安全相对路径、类型、大小与完整复制结果，不允许绝对路径、`..`、链接越界或文件名碰撞。完成为 success 才将同一请求的导入目录交给引擎；取消/失败清理未交付目录，源目录永远只读。

发布第一轮默认 collision=reject，只有经验证的“新且空目标、不会覆盖已有文件”路径才启用；否则返回明确 unsupported/error，保留 sandbox 输出并提供单文件 picker 输出。最终完成另含 `{publishedCount,failedEntries,partial}`，partial 不得回报 success、不清原项目 dirty。进度消息使用 `{kind:"folderProgress",id,processedBytes,totalBytes,filesDone}`，取消 `{kind:"folderCancel",id}`。取消仅请求 SDK 停止，最终 completion 必须反映实际部分写入；不自动删除用户目录下无法可靠辨认的文件。

建议验收矩阵：本地与远端 provider；空/嵌套/Unicode目录；同名冲突；超过500文件；权限失效与重启再激活；不足空间；复制中取消/进程退出；部分输出保留；目录绑定撤销。上述真实目录能力未测前不开放 Light catalog 导入或 Film/Effect 批量项目/媒体写入。

## 独立导入 helper：实现与验证

[FolderImportBridge](../harmonyos/entry/src/main/ets/platform/FolderImportBridge.ets) 的接口是 `new FolderImportBridge(UIAbilityContext)`、`importFolder(request, onProgress):Promise<FolderImportCompletion>`、`cancel(id):boolean`。request/completion/progress 字段采用上面导入 schema；每个实例串行处理一项，busy/无能力/无持久READ授权返回明确 error，不调用系统目录管理特权 API。仅返回持久沙箱 root、清单路径与 opaque bindingId，源 URI 留在 job 的 ArkTS source.json。

选择 FOLDER→持久并激活 READ→独有 job/contents URI copy→[沙箱 lstat 清单验证](../harmonyos/entry/src/main/ets/platform/SandboxFolderManifest.ets)→逐文件/目录 fsync→manifest/source metadata fsync，成功才移交目录。内部导入基目录逐级 lstat，不跟随已存在的链接。复制结果的链接/特殊文件拒绝，清理时 unlink 链接而不进入其目标；只处理本次新建 job，拒绝任何非第一层名字和路径越界。错误或取消等待 SDK copy promise 完结后清理，清理失败明确升级为 error。用户授权可能继续存在，不在失败时撤销可能共享的既有目录授权。

交付上限为 500 个文件、1 GiB、32 层相对路径、4096 个目录项。复制进度超过 byte cap 时请求 TaskSignal.cancel，但 provider 可延迟进度/取消；限制最终交付，不能承诺磁盘写入从未短时超过 cap。文件数量与树深度只能在沙箱复制后验证，因为公开接口未提供普通应用的 provider 目录枚举。检查拒绝的是复制结果中的链接；SDK/provider 若解引用源链接，源链接属性不可由普通 URI 接口预检。源侧授权与链接访问隔离仍依赖 provider 的权限执行，不能据后验清单宣称源端无链接。

取消 picker 时返回 cancel；外部 `cancel(id)` 在 picker 正打开时只能标记，SDK 未提供本 helper 可用的 picker 关闭 API，需等 picker 正常返回。复制/验证期间取消不会交付部分树。取消 SDK copy 请求失败也等待 promise 结束，绝不与仍运行的复制任务竞争清理。进度的 filesDone 在复制期间为0（SDK仅提供字节），扫描完最后一条为真实 fileCount，不伪造文件进度。

[19 项真实临时文件测试](../tests/folder_manifest.test.cjs)已通过：Unicode与空目录、500/501文件、累计大小边界、文件/目录链接与根链接拒绝、不进入外部目标的清理、注入绝对/斜线/dot段/NUL路径、重复名字、前置与扫描中取消、深目录、IO失败、清理异常及非法limits。隔离 CompileArkTS通过（日志 `/private/tmp/photocraft-arkts-folder-build.log`）。真实 FOLDER picker/持久授权/URI整树层级/取消/远端 provider 仍待 root fixture 验收，结果前不接原 batch/processor。
