# 完整沙箱树发布到全新随机子目录

状态（2026-10-08）：#9已仅接原ImageProcessor的预授权输出选择、独有stage分配与平台发布；24项新增工作流、旧helper25项和目录输入19项兼容回归通过，API26隔离CompileArkTS 8.617 s /完整11.993 s成功。最终平台字段与owned/取消语义见 [Image Processor协议](folder-processor-protocol.md)。原独立helper25项与初次API26编译证据（1.759 s /3.174 s）保留如下；manifest未改，商业HarmonyOS PC/provider实际层级、授权与取消仍待root fixture，本记录不称设备已通过。没有自动将Batch或用户输入目录写回外部目录。

## 接口与沙箱所有权

```ts
const bridge = new FolderPublicationBridge(abilityContext);
const stage = await bridge.createStage(allocationId, opaqueOriginalJobTarget);
// Trusted worker writes the complete output into stage.stageRoot, then stops changing this entire job.
const completion = await bridge.publishFolder({
  kind: 'folderPublish', id: newAttemptId, target: opaqueOriginalJobTarget,
  ownerRoot: stage.ownerRoot, stageRoot: stage.stageRoot,
  limits: { maxFiles: 500, maxBytes: 1073741824 }, policy: 'freshSubdirectory'
}, (progress: FolderPublicationProgress) => { /* typed progress, at most every 200 ms */ });
bridge.cancel(newAttemptId, opaqueOriginalJobTarget);
const recoveries = await bridge.discoverRecovery(); // read only; never automatically replay
```

`createStage` 返回 `{ownerRoot,stageRoot,journalPath}`。根为 `filesDir/PhotoCraft/Documents/FolderPublication/<allocationId>-<six random characters>`，由真正 SDK `mkdtemp(...-XXXXXX)` 建立；初始输出树为 `ownerRoot/contents`，`owner.json` 捕获安全整数 allocation ID、opaque target 与 ownerRoot，初始恢复 journal 已同步。Rust 生产者后续若自行建立相同契约的可信 owned job，应使用相同路径与 owner 标记，不把任意个人路径或应用其他文档路径交给此 helper。当前没有新增 Rust/Native ABI 或 allocation 平台事件；实际 worker 接线由后续批次单独冻结。

`id` 必须为非负 JS 安全整数，每次尝试使用新 ID；`target` 是发起时捕获的原输出 job/dialog generation token，1–512 字符，不含 NUL。helper 对 owner 标记进行精确 target 比对，不以当前 active tab 替代。原文档/字段生命周期与完成时的 target 是否仍有效，仍须 Rust pending registry 验证；helper 不授权文档 Save clean，也不改 dirty/path/revision。

只接受这个 owned job 里的 `contents` 或合法 `PhotoCraft-Export-<UUIDv4>` 作为完整树。检查 `filesDir` 以下每个祖先、ownerRoot、tree root 是真实普通目录，逐项 `lstat` 拒绝链接/特殊文件；清单复用生产 `SandboxFolderManifest`，上限 500 个文件、1 GiB、32 层、4096 项，拒绝绝对/点段/反斜线/NUL/重复/越界相对路径，拒绝空输出。全树文件与目录先同步，安全清单落在 ownerRoot，随后才允许外部复制。

可信 worker 必须将整个 owner job 的写入权独占交给本次发布，直到 Promise settles；helper 不声称能防止同进程另一写入者故意在校验后替换树。Rust 不应在任务运行中删除、再编码、rename 或复用该树。`retainStage:true` 表示此 helper 在所有完成结果上均保留材料；即使 success，清理也只能由原 owner 在接收 completion 且 SDK Promise 已结束后进行。

## 每次尝试的完整目录布局

1. 校验并同步整个输出树，写 `ownerRoot/manifest.json`。
2. 调 `util.generateRandomUUID(true)` 生成 RFC4122 v4 UUID（CSPRNG，122 个随机位），构成可辨认的 `PhotoCraft-Export-<uuid>`。无时间戳/计数回退。最多八次随机分配；用 no-follow `lstat` 查本地已知碰撞，包括 dangling symlink，已存在的名称不被覆盖。
3. 先同步 `prepared` journal，再在同一 owned job 内 rename 完整树为新名称，同步 parent，写 `renamed` journal。每次 retry 使用新尝试 ID 和新的 UUID；caller 从前次 completion 的实际 stageRoot 继续，不重复旧外部 child 名称。
4. 普通 `DocumentViewPicker.select(FOLDER)` 只选一个已有输出目录；按 normal `FILE_ACCESS_PERSIST` persist/check/activate READ|WRITE，再通过公开 API22 URI `stat` 确認它确为目录。任一步不支持则返回明确 error，不降级到个人路径/system fileAccess。
5. 将原 picker URI 原样写入 private journal，耐久 `copying` journal 成功后，才调用 `fileIo.copy(fileUri.getUriFromPath(namedTree), selectedFolderUri, {copySignal,progressListener})`。树外的 owner/manifest/journal 不被发布；不额外复制 ownerRoot 或 `contents` 容器，不拼接 child URI。
6. 只有 SDK copy Promise 成功、未取消、最终 `complete` journal 同步成功才返回 success。最终进度包含确切 manifest 文件数。此成功表示 SDK 任务复制完成，不代表所有外部文件/目录已 `fsync`，不称跨 provider 原子或断电耐久提交。

预期目标为 `selectedFolder / PhotoCraft-Export-<uuid> / <entire tree>`。已验证的官方布局证据在 [目录工作流计划](folder-workflow-plan.md)：固定 `file_api` commit `2b6d9dacfb467e34f1ec8d79a4b94d8255290216` 的 NAPI 委托，以及固定 `dfs_service` commit `ff4b37628faa46be585f91c1f227e1b7dab6f5ad` 的 `CopyDirFunc` 与真实官方执行测试。源码及 host layout 证据归档在 `logs/regression/folder-layout-review-20261008`。helper 契约测试使用相同 basename 布局的 host provider shim，并不代替真实商业 SDK/provider fixture。

## 取消、失败、恢复与 typed completion

`publishFolder` 每实例只运行一个任务；另一个请求、重复尝试 ID 返回 error，不取消原任务。`cancel(id,target)` 两项都匹配才生效，直接标记取消并尝试 `TaskSignal.cancel()`，独立于任何 `PlatformBridge.running` 门禁。以后平台接线必须启动独立有界 job，持续轮询控制事件；不能将长目录 Promise await 到现普通串行 `runEvent` 中，从而阻断原 Cancel。

系统 picker 没有公开主动关闭方法；取消此阶段登记标志、等正常 picker 返回后 completion(cancel)。正在复制时，取消或 provider 错误都必须等 SDK Promise settle，才交付终态；此 bridge 始终不删除 stage，不竞争运行中的复制 worker，也不自动删除/回滚没有明确子 URI 的外部文件。进度只包含字节和身份，不泄露 URI/文件名；非法字节统计或非终态 progress callback 抛错触发取消信号并返回 error。最后的信息性 progress callback 不会将已确认完整复制改成假失败。

```json
{"kind":"folderPublishProgress","id":7,"target":"opaque-job","processedBytes":4096,"totalBytes":8192,"filesDone":0}
{"kind":"folderPublishComplete","id":7,"target":"opaque-job","result":"error","ownerRoot":"/sandbox/owned-job","stageRoot":"/sandbox/owned-job/PhotoCraft-Export-uuid","journalPath":"/sandbox/owned-job/publication.json","publicationName":"PhotoCraft-Export-uuid","publishedCount":0,"partialPossible":true,"retainStage":true,"error":"目录导出未完成，可能保留部分结果；完整输出仍保留，请检查新输出目录后重试或另选目录"}
```

复制开始前的 Cancel/error 为 `partialPossible:false`；进入 provider copy 后无法可靠枚举完成项，所以失败为 `publishedCount:0,partialPossible:true`，0 不等于没有外部副作用。success 文件数来自已冻结 manifest，`partialPossible:false`。用户错误说明结果与重试/另选目录行动；内部 stage/journal路径只作为 worker恢复材料，不能堆进产品 UI。SDK诊断只打印固定阶段名称/数值 BusinessError code，不打印 URI、target、用户文件名/文字。

若最终 journal 更新也失败，返回 error，保留先前已同步 journal、`.next` 与完整 owned job。rename 拒绝保留旧 tree；出现不确定位置时保留整个 ownerRoot，在 completion 中让 stageRoot 为空，prepared journal 已记录原 tree 与候选新名称，不能虚报成功或删除候选树。进程中断后的 `discoverRecovery` 仅安全读取有界（最多128个）owned job，报告保留材料与 partial 可能，不激活 permission、不重放 copy、不清理外部 URI。记录损坏/链接会给出需要检查的保留记录，而不会尝试执行它。

## 后续 worker 接线与真实 fixture

适用于 Save for Web slices、Generate Assets、Batch/Processor 多输出：引擎先完整编码到独有 tree，保留原命令/文档版本/输出任务 token，逐文件编码失败与目录发布失败分别汇总；完成时精确匹配 pending identity，success 仅标记本次导出交付，不清工作项目 dirty。输出同名冲突继续由原引擎检查，不能把用户原输入目录当输出树。completion 的当前 stageRoot/journalPath 需要被 worker 接收；重试不能仍使用 rename 前的 path。当前 `kind` schema 是 helper 内部 typed 接口，尚未冻结新的平台 JSON event 或原表单适配。

root 专用 provider fixture 仍须检查：根部预置同名图像/marker 不变；完整 Unicode/嵌套/空目录只出现在随机 child；第二次新名产生独立 child；取消/空间不足/权限拒绝/重启后 journal 保留和重试新名；不同 provider 单独验证布局与 READ|WRITE 权限。normal API 没有 EXCL/对子 URI 枚举回滚合同，随机隔离只把碰撞概率降至极低，不能声称严格零碰撞或安全合并已有子树。未支持的 provider 继续使用每文件明确系统 save URI 的保护 publisher 路线。

回归：`tests/folder_publication.test.cjs` 25 项，生产 ETS 经安装 SDK TypeScript 转译，真实临时树与 documented CoreFileKit shim 注入 permission、rename、fsync、journal、部分复制和延迟取消失败；覆盖 symlink、dangling collision、owner/路径/限额、前后 journal 时序、根部文件保持、独立新名 retry、只读 restart discovery。原始日志在 `logs/regression/folder-publication-20261008/{host-tests.log,compile-arkts.log}`。复制后共享新 ETS 与已编译隔离 snapshot 逐字节一致；Index/PlatformBridge/module SHA 校验均保持原样。本批没有共享 HAP/sign/install。
