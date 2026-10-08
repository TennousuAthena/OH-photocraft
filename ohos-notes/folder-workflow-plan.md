# PhotoCraft 授权目录接线计划

2026-10-08 #9进展：仅原ImageProcessor输入与预授权输出选择→独有stage→随机目录发布已接平台，24项新增工作流+44项受影响兼容回归、API26隔离Compile通过；商业provider仍待root fixture，见 [Image Processor协议](folder-processor-protocol.md)。#8原Load Stack / Contact Sheet / Statistics输入的105项host与SDK结果保留，最终输入协议见 [folder-input-protocol](folder-input-protocol.md)，通用随机输出helper见 [folder-publication-bridge](folder-publication-bridge.md)。本文其余保留初始只读计划作为背景；旧拟议schema以冻结协议为准。

## 原入口与适配边界

原应用没有 `Services.chooseDirectory` 回调；目录是原 egui 表单中的字符串字段。`default_dir` 取当前文档路径的父目录。鸿蒙端该路径是应用保留的源副本，不能据此推断用户外部目录。应在原表单的输入字段旁接系统“选择文件夹”操作，而非自动取沙箱源文件父目录执行，也不增加 ArkTS 工具栏。

| 原命令 | 原输入字段 | 原引擎行为 | 下一批最小接线 |
| --- | --- | --- | --- |
| `file.scripts.loadFilesIntoStack` | `paths` / `input` | 文件数组或目录；生成新文档 | 目录清单选择后设置 `paths` 数组，保留原确认与其他选项 |
| `file.automate.contactSheetII` | `input` / `files` | 文件数组或目录；按分页生成新文档 | 设置 `input` 数组，保留尺寸、字体、间距、分页与 caption 选项 |
| `file.scripts.statistics` | `input` / `paths` | 复用 Load Stack，再统计/对齐 | 同样设置数组；不把“选择完成”当作命令确认 |
| `file.automate.photomerge`、`mergeToHdrPro` | `paths` | 数组、目录，或 `useOpenDocuments` | 文件夹选择只替换文件输入；选择 open documents 时保留原逻辑 |
| `file.automate.batch` | `input` / `files` | 每个输入创建 scratch session，执行捕获的 action steps，再写输出 | 输入先接数组；输出必须是独有沙箱 job，用户目录发布另处理 |
| `file.scripts.imageProcessor`、`file.automate.lensCorrection` | `input` / `files` | scratch 处理及输出，逐文件汇总错误 | 同上，不能把源 URI 或用户输入字符串直接交给引擎文件 API |
| `file.automate.runDroplet` | `input` / `files`，另有 droplet 路径 | 将输入目录展开为本层文件后运行 Batch | 本批候选后续项；droplet 文件选择和输入文件夹请求分别捕获 |

源证据：`upstream/crates/ui-egui/src/view_cmds.rs` 的 `default_dir`、Batch/Processor/Load Stack/photography 表单；生产 overlay 的 `vendor/photocraft-ui-egui/src/file_ui.rs` Contact Sheet/Statistics；`upstream/crates/engine/src/file_cmds.rs` 的 `list_images`、`batch_inputs`、`load_files_into_stack`、`process_files`；`upstream/crates/engine/src/automate_cmds.rs` Contact Sheet/Statistics/Droplet。

`list_images` 是 `read_dir` 本层扫描，按完整路径字符串排序，不递归；数组输入保留数组顺序。第一批默认保持本层、按安全相对路径排序、按引擎 `OPENABLE` 清单筛选，不能借目录导入悄悄变成递归处理。真正递归作为用户显式选项再增加。引擎清单比单文件 Open 的 codec 清单更窄，继续使用其既有规则，不将每个沙箱文件都当成可解码图像。

现 helper 为复制后验证，SDK 会复制整个所选树；即使后续只消费本层图像，子目录仍计入复制的 500 文件、1 GiB、32 层与 4096 项上限。公开 normal API 无 provider 枚举入口，不能在复制前只挑本层。因此本层处理语义可保持，但大而深的输入目录可能被明确拒绝；这是首批需说明与测试的能力限制。

## 请求、目标身份与所有权

沿现非阻塞 `takePlatformOutput` / `platformInput` JSON 通道，不新增 C ABI。建议输出事件：

```json
{"kind":"folderImport","id":1,"intent":"file.automate.contactSheetII","target":"opaque-dialog-field-generation","scope":"topLevel","limits":{"maxFiles":500,"maxBytes":1073741824}}
{"kind":"folderCancel","id":1,"target":"opaque-dialog-field-generation"}
```

`id` 为唯一非负 JS 安全整数，`intent` 使用明确白名单。Rust 在发起时记录原 dialog ID、命令、字段、dialog generation、相关 document identity；必要时捕获原参数/Action steps。`target` 是对这些身份的 opaque token，不让 ArkTS 解析或重建文档身份。请求完成不能拿当前 active tab 或最新同名 dialog 猜目标；取消原 dialog、替换字段、关闭文档、切换到新 dialog generation 后，旧完成必须拒绝。

适配器给现 helper 传 `kind:'folderImport'`、`recursive:true` 及 limits；此 `recursive` 表示完整复制和验证，与业务 `scope` 分开。helper 不知道原 UI 或引擎。完成继续使用其 sandbox paths 和 opaque binding：

```json
{"kind":"folderProgress","id":1,"target":"opaque-dialog-field-generation","processedBytes":4096,"totalBytes":8192,"filesDone":0}
{"kind":"folderComplete","id":1,"target":"opaque-dialog-field-generation","result":"success","resolvedRoot":"/sandbox/contents","manifestPath":"/sandbox/manifest.json","bindingId":"opaque-job","error":""}
```

成功只表示复制、清单验证和同步完成。Rust 在工作线程重新检查 root/manifest 的合法应用目录、普通文件、相对路径、计数与大小；依据 scope 构建具体 sandbox 文件数组，再将其设置到原目标字段。空清单或没有符合引擎规则的图像应在原 UI 明确提示，不确认执行空命令。用户仍通过原表单确认其他参数。引擎执行结果、逐文件错误、后续导出分别有自己的状态，不把导入成功当成 Batch 完成。

路径和清单不能作为大 JSON 填入 worker mailbox，仍只传持久 sandbox 清单路径。成功 completion 被 `platformInput` 接受后副本所有权移交 Rust；被队列拒绝时 ArkTS 持有副本并重试，不能先删除或重复交付。过期目标要由 Rust 明确接收并拒绝、清理其已获得的副本，不能返回一个含义不明的 false 导致两端同时删除。正常业务结果不得修改原 source URI 或解除可能由别的请求共享的持久授权。

## 进度、取消与队列

不能把长目录任务直接 `await` 到当前 `PlatformBridge.processEvents` 的普通串行分支：`running` 会让 `poll` 停止拉取输出，原 UI 的 `folderCancel` 将永远等到复制结束。应增加一个有界目录 job 槽，启动后由 Promise 的 completion 处理终态；正常 poll 继续读取控制消息，处理匹配 ID/target 的 cancel、closeState 与安全 close，同时拒绝第二个导入、重复确认或冲突 file picker。其他 clipboard/print/viewport 操作遵循各自有界策略，不在 folder cancel 上建立依赖循环。

进度最多每 200 ms 合并发送最新值，terminal completion 不被进度淹没。SDK 复制阶段只给字节，`filesDone=0`；扫描完成后才是实际 fileCount。进度过大、callback 错误、复制或权限失败均必须 completion(error)，恢复原表单操作状态与焦点。Cancel 先标记匹配 job，再 `TaskSignal.cancel()`，等待 copy Promise settle 后清理本次独有目录。不能在仍运行的 copy 上竞争删除，也不能因一个旧 cancel 取消新 job。

系统 picker 没有公开的主动关闭接口；此阶段 Cancel 只登记并等待 picker 正常返回。picker 引起的 ability background 是预期用户操作，不能立即取消请求；真正 page hide、ability destroy、原 dialog 取消才撤销 UI 目标，等任务 settle 后清理。完成只恢复仍可见、可用的原编辑器焦点，不把后台窗口强行置前。输入 clean close 不得跳过原 unsaved flow。

## 普通 SDK 下目录输出的查证结论

| 能力 | 本机 API26 合约 | 安全输出含义 |
| --- | --- | --- |
| `DocumentViewPicker.select(FOLDER)` 与 persist/check/activate READ/WRITE | normal，取现有目录 URI，仍需 provider 实际支持 | 有授权不代表存在安全的子文件创建/冲突查询 API |
| `fileIo.copy(srcUri,destUri,options)` | URI 文件/目录，强制覆盖；无 collision/reject 选项 | 不能直接作为“选一个目录且绝不覆盖已有文件”的发布实现 |
| `listFile`、`mkdir`、`rename`、`rmdir` | SDK 参数仍为 application sandbox path | 不把用户目录 URI 传作 sandbox path，也不解析成个人路径 |
| `stat` / `lstat` API22+ URI、`open` URI | 现有文件/目录元数据或句柄 | 没有 parentURI + childName 定位/创建入口；查存在再 CREATE 不是排他创建 |
| `OpenMode` | public CREATE/TRUNC/DIR/NOFOLLOW 等，无 EXCL 常量 | 不注入未经公开的旗标数值，不用 CREATE 宣称 race-free collision reject |
| `FileUri.getFullDirectoryUri()` / `getUriFromPath()` | 已有对象的父 URI / sandbox path 的 URI | 均不是对子 URI 的授权创建工厂 |
| `fileAccess.createFile(parentURI,name)`、mkdir/list/iterator | system API + FILE_ACCESS_MANAGER；本机 normal namespace 为空 | 普通应用不能使用，弃用注记也不使这些方法成为 normal API |
| `DocumentViewPicker.save` | normal，返回明确文件 URI；`newFileNames` 可多个；API23 有 autoCreateEmptyFile | 显式文件保存是现可研究的合法出口；不是通用目录管理 API |

由现有公开接口，不能承诺任意 provider 下“选目录一次、合并到已有命名子树、严格 reject 同名、可恢复所有部分写入”。整树 copy 可能在失败/取消前已覆盖一部分，且 normal app 无可靠列举结果子 URI 来备份/恢复、辨认并删除本次文件。因此不能发布到可预测或复用的已有子树。但这不阻止下一节的全新随机子目录方案：源码已证明其正常本地布局，仍待真实 provider fixture。一次 provider 测试成功也不能扩大为跨 provider 合约。

已有明确公开 API 的备用路线是**显式文件发布**：引擎写独有 sandbox 输出 job，显示真实结果与错误；对每个输出启动明确 `DocumentViewPicker.save`，只使用其返回的具体 URI，经现保护 publisher 备份、写入、同步/恢复。这样不会因为选择输入目录或 Batch 输出字段而暗中写外部兄弟文件；用户明确选目标后才发布。它允许用户选择覆盖，且仍不能称 provider 原子写入或严格排他创建。

一次 `save(newFileNames:[...])` 的 flat 批量保存可做独立 fixture，但要核实返回数量、basename 映射、改名/同名冲突、用户取消、是否产生空文件与其授权。不能仅按数组序号假定对应，更不能合成缺失 URI。不同输出映射到同一 URI/name 应在实际 copy 前拒绝；不自动删除 picker 留下的空目标。嵌套目录不得用扁平名字静默摊平。单文件 archive 也可作为保留树结构的后续明确产品选项，但本批不实现。

`autoCreateEmptyFile=false` 是 API23 的真实公开选项，但只返回 URI、不预置文件；它不能提供排他创建保证，当前 publisher 要求可读目标备份也不适用不存在目标。不得为了适配它移除旧文件保护。是否能另行提供“全新文件 reservation”必须有独立 API/fixture 合约，再设计不同事务。

原生 `fstat` + `openat(dirFD,basename,O_CREAT|O_EXCL|O_NOFOLLOW)` 可作为仅真实本地目录 FD 的实验方向。它没有 URI 拼接，但公开 FilePicker/FileIO 合约尚未保证所有目录 URI 的 fd 是可由 normal app 子目录遍历的真实 POSIX fd，也未保证 provider 权限能据此扩展。当前没有把它列成已证实输出能力，不调用它、不从 `/proc/.../fd` 还原路径；只有 root 授权独立 fixture并确认权限/FD契约后才可能启用受限本地路线。

证据：[官方 FilePicker API](https://github.com/openharmony/docs/blob/master/zh-cn/application-dev/reference/apis-core-file-kit/js-apis-file-picker.md#documentsaveoptions)、[URI 文件指南](https://github.com/openharmony/docs/blob/master/zh-cn/application-dev/file-management/user-file-uri-intro.md)、[目录强制覆盖 copy](https://github.com/openharmony/docs/blob/master/zh-cn/application-dev/reference/apis-core-file-kit/js-apis-file-fs.md#fileiocopy11)、[system fileAccess 创建接口](https://github.com/openharmony/docs/blob/master/zh-cn/application-dev/reference/apis-core-file-kit/js-apis-fileAccess-sys.md#createfile)。本机对应声明在 SDK `ets/api/@ohos.file.fs.d.ts`、`@ohos.file.picker.d.ts`、`@ohos.file.fileuri.d.ts`、`@ohos.file.fileAccess.d.ts`。`FILE_ACCESS_PERSIST` 的公开范围由 [持久授权文档](https://github.com/openharmony/docs/blob/master/zh-cn/application-dev/file-management/file-persistPermission.md)约束。

## 全新随机输出子目录：可进入 fixture 的首选方案

root 提议为完整 sandbox 输出树生成不可预测的随机 basename，再把其 URI 整体复制到用户选中的已有目录 URI。已查到实际 NAPI 委托和目录布局，不能把“没有 EXCL”解释成这条方案完全不可行。

- `file_api` 固定 commit `2b6d9dacfb467e34f1ec8d79a4b94d8255290216`，`copy.cpp:386` 的 NAPI 执行调用 `FileCopyManager::Copy(srcUri,destUri,progress)`。
- `dfs_service` 固定 commit `ff4b37628faa46be585f91c1f227e1b7dab6f5ad`，`file_copy_manager.cpp:417` 确认 destination 已是目录后统一尾部 slash，`:569` 的 `CopyDirFunc` 取 source basename，再调用 `CopySubDir` 创建 `destination/source-basename`；`:591` 若该 child 已存在则复用，故重复 basename 会合并/覆盖。
- 同一固定 DFS commit 的 `file_copy_manager_test.cpp:1314` 对实际 `ExecCopy` 结果核对 `dst/test_src_dir/1.txt`；官方 File API `FsCopyTest.js:732` 也核对 `dst/src/test1` 与 `dst/src/test2`。未拿别的 `copyDir(path,mode)` API 替代 URI `copy` 的证据。
- 这些是公开 OpenHarmony 源码，不是已测商业设备二进制的身份声明；真实 HarmonyOS PC/provider 仍需同样 fixture。

固定源链接：[当前 NAPI 委托](https://github.com/openharmony/filemanagement_file_api/blob/2b6d9dacfb467e34f1ec8d79a4b94d8255290216/interfaces/kits/js/src/mod_fs/properties/copy.cpp#L386)、[当前复制布局](https://github.com/openharmony/filemanagement_dfs_service/blob/ff4b37628faa46be585f91c1f227e1b7dab6f5ad/frameworks/native/distributed_file_inner/src/copy/file_copy_manager.cpp#L569)、[实际执行的官方测试](https://github.com/openharmony/filemanagement_dfs_service/blob/ff4b37628faa46be585f91c1f227e1b7dab6f5ad/test/unittests/distributed_file_inner/copy/file_copy_manager_test.cpp#L1314)、[目录层级 JS 测试](https://github.com/openharmony/filemanagement_file_api/blob/2b6d9dacfb467e34f1ec8d79a4b94d8255290216/interfaces/test/unittest/napi_test/FsCopyTest.js#L732)。

独立 host probe 抽取**未改动**的固定 DFS `CopyDirFunc`，用真实临时目录执行其布局函数，递归后端由 host `std::filesystem` 提供。两项检查通过：随机 source basename 的完整嵌套树进入新 child，目标根的同名文件/其他文件不变；刻意重复 basename 展示已有 child 被覆盖的风险。它验证源码布局，未运行完整 DFS/SDK provider/权限和取消实现。日志在 `/private/tmp/taskfolder/official-copy/copy-layout-probe.log`，源码及官方文件也在该独立临时目录。

拟议发布流程：

1. 输出树在编码前确定随机名称，由可信 owner 创建为独有持久 sandbox 目录，例如 `PhotoCraft-Export-<random>`。随机值使用 SDK CSPRNG：`util.generateRandomUUID(false)` 是公开 API9 的 RFC4122 v4 128-bit UUID（随机位为122 bit）；若要求完整128 bit随机熵，可用 `cryptoFramework.createRandom().generateRandomSync(16)` 的字节转32位hex。失败不得回退为时间戳或递增编号。
2. 先完整编码并验证 manifest、普通文件、无链接、数量/大小/安全相对路径，同步所有 sandbox 文件/目录并冻结该 job。原 Batch/Processor 的重复 output stem 继续返回逐文件错误，不能把冲突输出随意覆盖；输出根来自本次 job，不采用源路径默认值。
3. 使用正常 FOLDER picker 获得 destination URI，按 provider 能力持久并激活 WRITE（需要时 READ|WRITE）。只调用 `fileIo.copy(fileUri.getUriFromPath(stageRoot), selectedFolderUri, options)`；不构造 `selectedFolderUri + random`，不取得外部个人路径，也不调用 system fileAccess。UI 可显示新目录名字，用户可在文管中辨认它。
4. 每次发布尝试必须使用全新不可预测 basename。若重试已有输出，可信 sandbox owner可在自己的目录内 rename 为新名并同步/更新内部 pending/journal，然后发新请求；不能再次 copy 到旧 basename 的部分结果，也不能在不知道其状态时猜是本应用唯一所有的外部 child。
5. `copy` Promise 成功才发本次 success；只表示 SDK 复制任务完成，源码未承诺目录/所有 destination FD fsync，因此不称断电原子或跨 provider 耐久事务。业务导出状态与工作文档 dirty/path/revision保持区分，成功不触发原项目 Save clean。
6. 错误、Cancel、进程退出可能留下部分新 child；保留完整 sandbox stage与 journal，明确提示“本次目录导出未完成，可能保留部分结果；请查看新输出目录并重新导出。”不悄悄报 success，不自动删除或回滚不具有明确子 URI/所有权的用户文件。启动只显示恢复材料，不自动重放 copy。

建议独立 schema，仍未冻结：

```json
{"kind":"folderPublish","id":2,"target":"opaque-output-job-generation","stageRoot":"/sandbox/PhotoCraft-Export-random","manifestPath":"/sandbox/job-manifest.json","publicationName":"PhotoCraft-Export-random","policy":"freshSubdirectory"}
{"kind":"folderPublishComplete","id":2,"target":"opaque-output-job-generation","result":"error","publishedCount":0,"partialPossible":true,"retainStage":true,"publicationName":"PhotoCraft-Export-random","error":"目录导出未完成，请检查输出目录并重试"}
```

Rust/ArkTS共同检查实际 stage basename与 publicationName一致，合法应用输出根及原 pending identity，不接收任意可改名路径。外部 URI仅留ArkTS journal/binding。`publishedCount` 只有能证明的值：成功可使用已冻结 manifest 文件数；失败无可靠 provider 枚举时保持0并标 `partialPossible:true`，0不是“没有任何外部副作用”。Cancel必须保留partial含义。运行中同一 job禁止二次提交，晚到/重复 completion不能作用到新 job。

这条路线通过随机隔离使撞到用户既有命名目录的概率极低，并保护目标根兄弟文件；它没有排他创建合约，不声称形式化零碰撞。只有 fixture 确认这一 provider 保留basename子目录布局、不会摊平/另改名且权限有效后才启用相应provider范围。保留显式每文件 URI保存作为失败/不支持provider的可用路径。

## Fixture 与验收顺序

1. root 准备专用用户目录：空目录、本层 Unicode PNG/PSD、同 stem 不同扩展图像、非图像、两层子目录及空子目录。通过真正系统 FOLDER picker 选它，记录 URI 类型但不依赖 URI 格式；验证整树 copy 是合并 contents 还是多套一层源目录名，再固定消费 root。父目录或根文件不能靠猜测跳层。
2. read-only 本地/外接盘/远端 provider分别验证 persist READ、后台/重启 activate、复制层级、最终大小与名称；另测 500/501 文件、累计 byte cap、空间不足、缺 READ/授权失效、取消与销毁。成功副本必须仍可重复读取原本层图像；失败不得产生新业务文档。
3. 先接 Load Stack、Contact Sheet/Statistics 等纯输入、新文档命令；验证原参数不被覆盖、空/无支持图像明确失败、排序不变、过期 dialog/文档与重复 completion 被拒绝、Cancel/后台/close无误执行。
4. Batch/Processor/Lens 的输出始终在新 sandbox job，复用引擎本次-run同名冲突检测；不得默认写到输入副本、已有输出目录或外部目录。逐文件引擎失败与发布失败分别汇总，partial不能报全成功；目录导入完成不清 dirty。
5. root 使用一次性输出 fixture 目录，根部预置 `image.png` 和 marker，只将完整 sandbox `PhotoCraft-Export-<random>` tree复制进去；通过实际文管/fixture检查新 child、嵌套/Unicode/空目录、根文件hash未变。第二次全新random应生成第二个child。在独立可销毁fixture里，刻意重用同basename确认overwrite风险，随后生产只允许新random。测试取消、空间不足/权限失败、重启恢复与重试新名，保留stage与partial提示；远端provider单独验，不能套用本地结论。
6. 显式每文件保存 fixture 先验证已有/新空目标、改名、取消、provider不支持read/seek/fsync与现publisher回滚。multi-file Save 的数量/映射/冲突在证明前不启用，继续用每文件明确URI的路线兜底。

阶段交付：新增 Rust typed form resource seam / folder pending registry；ArkTS 独立 directory-job 调度及完成所有权；原 egui 表单按钮和进度；manifest 沿现 normal FILE_ACCESS_PERSIST。通过 host UI/状态机回归、isolatedCompile、真实 FOLDER fixture 后再由 root 决定 HAP。随机新子目录的独立 helper 已按 root 授权实现，25项生产 adapter/事务 host测试与 API26 isolatedCompile通过；尚未接原业务入口或平台输出协议，真实provider fixture仍待验证。接口、owned stage、全新命名重试和保留journal语义详见 [FolderPublication bridge](folder-publication-bridge.md)；保留显式文件保存路线作为未支持 provider 的出口。
