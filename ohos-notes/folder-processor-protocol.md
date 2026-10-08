# 原 Image Processor 目录输入与输出

2026-10-08 #9：ArkTS 原菜单平台接线已实现，新增24项生产 helper/coordinator/PlatformBridge 工作流测试通过；受影响的目录输入19项与旧发布25项兼容回归通过，共68项。API26隔离 CompileArkTS 8.617 s、完整隔离构建11.993 s成功。共享源码与已编译快照一致，Index、manifest、Native/d.ts未改；本次未运行共享 HAP/sign/install。商业 HarmonyOS PC 的普通 FOLDER picker、授权和整树copy仍待 root实际fixture，不将host shim当设备验收。仅连接原 `file.scripts.imageProcessor`，没有顺便启用Batch、其他命令或新ArkTS业务界面。

## 原行为与参数

原 `view_cmds` 表单保留 `input`、`output`、`format`（jpg/png/psd/tiff）、`quality`（默认8）、`width`/`height`、`convertToSrgb`。输入Browse继续沿 #8明确 `sourceBasenameChild` 的目录导入，目标字段为原 `input` 数组，本层图像枚举和排序交原引擎。子目录仍完整复制、验证并计入限额，但不暗改本层处理语义。

原引擎 `process_files` 为每个输入建立scratch session，执行原fit（dontEnlarge）和颜色逻辑，再按原format编码。原 `OutputClaims` 防止本次run忽略大小写的同名输出覆盖，`files/errors` 汇总保留。输出发布不会修改原工作文档dirty、path、saved revision，也不把源输入副本当输出目录。真实PNG/PSD处理、尺寸/像素和逐文件错误的Rust回归由配套实现独立记录。

## 先授权，再分配，再发布

无新C ABI，沿现 `takePlatformOutput` / `platformInput` JSON。`target` 是Rust捕获的原输出字段generation、dialog与document身份的opaque token，ArkTS不解析；同token贯穿三个阶段，每阶段使用独立非负JS安全整数id。原Output Browse先选择授权目录，仅填逻辑handle；Confirm后分配独有空输出树，原引擎全部编码结束并冻结owner job后，才发发布事件。

```json
{"kind":"folderDestination","id":20,"target":"opaque-original-output-generation","intent":"file.scripts.imageProcessor"}
{"kind":"folderDestinationComplete","id":20,"target":"opaque-original-output-generation","result":"success","destinationHandle":"00000000-0000-4000-8000-000000000001","error":""}
{"kind":"folderStageAllocate","id":21,"target":"opaque-original-output-generation","intent":"file.scripts.imageProcessor","destinationHandle":"00000000-0000-4000-8000-000000000001"}
{"kind":"folderStageComplete","id":21,"target":"opaque-original-output-generation","result":"success","destinationHandle":"00000000-0000-4000-8000-000000000001","ownerRoot":"/sandbox/PhotoCraft/Documents/FolderPublication/21-a1B2c3","stageRoot":"/sandbox/PhotoCraft/Documents/FolderPublication/21-a1B2c3/contents","journalPath":"/sandbox/PhotoCraft/Documents/FolderPublication/21-a1B2c3/publication.json","error":""}
{"kind":"folderPublish","id":22,"target":"opaque-original-output-generation","destinationHandle":"00000000-0000-4000-8000-000000000001","ownerRoot":"/sandbox/PhotoCraft/Documents/FolderPublication/21-a1B2c3","stageRoot":"/sandbox/PhotoCraft/Documents/FolderPublication/21-a1B2c3/contents","limits":{"maxFiles":500,"maxBytes":1073741824},"policy":"freshSubdirectory"}
```

`FolderDestinationBridge` 用真正normal FOLDER picker、persist/check/activate READ|WRITE和URI `stat` 校验目录。它用系统CSPRNG UUIDv4 handle，把原URI、请求id和target耐久写入 `filesDir/PhotoCraft/Documents/FolderDestinations/<handle>.json`；所有祖先/记录需普通无链接，记录≤64KiB、最多4096项、已存在随机名（含dangling link）跳过。URI不传Rust、不出现在用户错误或诊断日志中，不转换个人路径，不拼任何child URI。

`FolderStageBridge` 在分配前用相同handle+target重新校验记录、现有授权与目录；过期或不匹配要求重新Browse，不能偷偷换URI或空成功。随后SDK `mkdtemp(<allocationId>-XXXXXX)` 建立完整owned空job，owner.json捕获allocation ID/target，初始journal与目录已同步。allocation ID不同于publication attempt ID；Rust校验/清理使用真实owner.creationId，不根据publish ID猜目录名。

连接的publication service必须有合法handle，并再次解析原记录；禁止重新弹picker。原独立helper的未接线调用仍可自行picker，但PlatformBridge不使用该模式。完整输出树递归普通类型/路径/数量/大小校验并同步后，再按 [FolderPublication bridge](folder-publication-bridge.md) 的UUID新子目录策略rename、同步journal、copy到原选中URI。只有provider Promise成功、未取消、最终journal成功才返回success；文件数取实际冻结manifest。新目录名具有122随机位，不宣称排他创建或跨provider原子提交。

## 完成、取消和所有权

进度与发布完成沿独立helper冻结字段：

```json
{"kind":"folderPublishProgress","id":22,"target":"opaque-original-output-generation","processedBytes":4096,"totalBytes":8192,"filesDone":0}
{"kind":"folderPublishComplete","id":22,"target":"opaque-original-output-generation","result":"error","ownerRoot":"/sandbox/owned-job","stageRoot":"/sandbox/owned-job/PhotoCraft-Export-uuid","journalPath":"/sandbox/owned-job/publication.json","publicationName":"PhotoCraft-Export-uuid","publishedCount":0,"partialPossible":true,"retainStage":true,"error":"目录导出未完成，可能保留部分结果；完整输出仍保留，请检查新输出目录后重试或另选目录"}
{"kind":"folderDestinationCancel","id":20,"target":"opaque-original-output-generation"}
{"kind":"folderStageCancel","id":21,"target":"opaque-original-output-generation"}
{"kind":"folderPublishCancel","id":22,"target":"opaque-original-output-generation"}
{"kind":"folderDestinationRelease","id":23,"target":"opaque-original-output-generation","destinationHandle":"00000000-0000-4000-8000-000000000001"}
```

`FolderOutputCoordinator` 处理选择/分配，`FolderPublicationCoordinator` 处理长发布；与已有输入coordinator共享文件操作可用性门禁，但不await到普通 `running` 队列。200ms独立poll继续收取消、closeState和safeclose；进度只留最新值，终态入队false保留并重试。原file picker/Drop/Paste授权不能并发启动第二系统选择；没有永久toolbar或另写业务表单。

Destination成功入队true移交handle；Stage成功或带owned路径的cancel入队true移交空job，Rust精确matching pending验证后执行或清理。false仍由shell持有。已移交资源不受旧shell Cancel/duplicate释放；未入队的Destination Cancel可校验原target并删自己private记录。接受后，只有Rust明确 `folderDestinationRelease` 才删除对应private record；它不撤销可能共享的providergrant、不删用户外部文件。Rust仅对matching stale或unusedselection发一次Release；unknown/duplicate完成不触发它。

Stage Cancel需等当前复验/分配Promise结束；已创建目录随cancel receipt交Rust验证清理，不在仍写入时删除。退出前未交付的empty job有durable journal并保留，不冒猜另一个owner。Picker无主动关闭公开接口，Cancel只记标志并等其正常返回；picker导致ability background不撤销任务。真正page hide/dispose取消；safeclose在原unsaved flow授权后，仍等待所有provider Promise settle才terminateSelf。

发布过程借用Rust完整冻结stage，coordinator在任何结果上都不删它或外部partial。成功回执等待满mailbox时，late Cancel不能把已完成复制改成假cancel。Rust只能接受matching回执；success明确实际已复制文件数，逐文件编码失败仍独立保留；error/cancel保留完整输出和journal、partialPossible，不能以0文件数假装无副作用。retry采用新attempt ID和completion返回的实际stageRoot，再生成新UUIDchild，不能用rename前path或复用旧外部partial名。provider或进程中断恢复仍只报告材料，不自动重放或删除外部partial。

## 证据与待验收

新增 `tests/folder_processor.test.cjs` 24项运行生产ETS：完整复制真实树/marker与manifest计数、backpressure成功真实性、匹配取消与延迟settle、并发/重复、partial+fresh retry、disposal保留、错误限额/回执、私有handle授权/失效/链接/碰撞/target、原授权→空stage→publication单picker、取消未交付handle、StageCancel ownership、真实Platform控制非阻塞、背景/pagehide与安全close。SDK contract shim仅验证规定行为，不代替commercialprovider。受影响的旧输入19与旧publication25兼容检查全部通过；不重复无关的既有105项合集。

原始日志与完整11个文件SHA256位于 `logs/regression/folder-processor-20261008`，API26 isolated snapshot `/private/tmp/photocraft-processor-check`。原Index和manifest hash保持 #8；normalpermissions仍仅FILE_ACCESS_PERSIST+PRINT，无READ_PASTEBOARD。root后续验证原ImageProcessor输入输出Browse、各格式实际输出/尺寸/像素、部分编码冲突、权限取消/失效、目标根原marker不变、每次新child、partial重试及后台/关闭，商业provider成功前不标发布PASS。
