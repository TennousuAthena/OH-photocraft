# PhotoCraft #13：来源自动化续行与关闭事务屏障

本批以固定上游 `4337a6227a823a28728e68aed844feab62b3314d` 和 #12 不可变源为基线。上游 checkout 不变；新增行为位于可重放的 engine/UI overlay、Rust wrapper 和现有 ArkTS 文件/平台桥。没有新增 C ABI、Native 修改、权限或编码算法。#12 归档保持不变。

## 原始调度边界

原 `Session.start/execute`、`jobs`、`file_cmds::process_files`、Scripts Browse、Actions 和 Script Events 都保留。默认禁用的 `Session.automation` 只由 OHOS wrapper 开启。没有平台 hook 的桌面/wasm仍走原同步行为。

开启后，脚本用有界 frame stack 保存当前步骤和原参数；来源操作暂停在步骤边界，真实结果完成后才继续下一步。每次来源关联 run ID、scratch Session generation 和 step ID，并验证原 DocId、文档 Arc、revision、path、图层及选择归属。用户切换 tab 不改变 logical owner；用户编辑或关闭原文档使旧结果失效。Edit Contents 后步骤归属切到原 SmartLink 的子文档，而用户当前访问的其他 tab 保留。

Action 面板的原 Play 进入同一调度器，原 Stop 取消 Action job；原作业进度、Cancel/Esc 和 footer 展示结果，不增加平台测试按钮。来源失败或取消停止后续写入。原 caller 参数在完成后才进入 journal，完成用的稳定 LayerId 不污染可跨文档播放的记录。

Script Events 保持原 prefs/binding 顺序；只有实际执行事件 frame 时启用原递归守卫，等待外部读取期间不会全局关闭事件。嵌套事件属于产生该事件的 run，而不是某个恰好拥有相同本地 JobId 的 scratch Session。Print 仍只表示系统受理，只有已实现的系统终态协议决定真实打印完成。

## Batch 输入、编码与发布

原 `process_files` 在原 command callback 边界分为输入、步骤、编码和结果收集；同步入口与异步入口共用这些实现。原 import、SavePath、格式/quality、fit、sRGB、逐文件 errors 和大小写不敏感 OutputClaims 不被复制或跳过。每个输入保留原 scratch Session 到其步骤完成，其他工作文档保持原内容、clean 和视图归属。

原表单授予的逻辑输出 handle 仍需要匹配的授权和独占空 stage。Batch 编码完成后，虚拟 engine job 持续 pending；只有验证合法 owner、清单和文件数的外部 publication receipt 才结束并记最终成功。坏 receipt 不丢弃 job/stage，失败或取消把实际已编码 bytes 留给现有恢复 UI。恢复发布不会重跑脚本或编码。

文件夹输入的来源保存为 private folder URI 与精确相对路径。重新读取复用原 FolderImportBridge、当前 SDK 的 `sourceBasenameChild` 复制契约及完整 no-link/manifest 校验，然后读取相同相对文件。不会拼 URI、猜 basename、跳任意目录层或以旧副本冒充外部新内容。权限撤销、重命名、删除、布局不符都明确失败。commercial provider 的新自动化场景仍需后续设备验证。

## 关闭与生命周期

Index 在弹出单文件请求或开始拖入工作前取得同实例的 `FileTransactionDrain` token，并在 binding 写入、文件及目录 fsync、清理的 finally 后释放。明确窗口关闭等待所有这些 token，以及原目录输入/输出/publication worker 真正收尾，再调用 `terminateSelf`。没有超时成功或假 ACK。关闭失败恢复入场，dispose 拒绝旧等待；已消失的旧 Index 不向新的 native app 实例发送迟到完成。

可见 PasteButton Cancel 仍立即拒绝粘贴结果，但 `PasteAccessRequest.settled` 仅在实际 provider read 和晚到 PNG discard 完成后 resolve。Index 保留 paste owner 和 token 到 settled；PlatformBridge 的 copy、paste、PNG enqueue 失败清理也保留事务 token。晚到读取/清理错误可见，旧实例不投递晚到成功。这样标题 X 不会在临时图像还在读取或删除时终止 UIAbility。

同批包含另一个 agent 完成的本应用 AppFaultObserver/EntryAbility 诊断，以及 root 完成的 FileTransactionDrain。它们不收集其他应用资料，也不增加权限。observer 的测试等待必须让真实 fsync/rename IO 完成；初次全套测试的时间敏感 fixture 失败日志保留，最终结果以真实队列 idle 的有界等待为准。

## 必要真实回归

engine 新增 8 项：嵌套 script fresh Revert 后 Invert/Undo 并保留其他 tab；取消后不执行文件写入；读取失败和原 owner 新编辑停止后续 Action；Batch scratch 来源和同名输出冲突及 publication pending；原 caller 参数跨文档重放；Edit Contents 子文档续行；Print Script Event 暂停及递归守卫；root/scratch 相同 JobId 不串完成。断言真实文档像素、history、文件 bytes、原 error 和最终 job 状态。

wrapper 完整 98 项通过，新增 3 项分别覆盖真实 FileRequests mailbox 的多步骤/取消、原 Batch 表单编码到合法 publication ACK/错误恢复，以及 private folder 来源持久化与恶意记录拒绝。原 engine 受影响回归 File 16、Automate 6、Jobs 11 通过；Jobs 原 24MP release 时序测试保持 1 项 ignored，没有将其计入通过。

生产 ArkTS/SDK adapter 测试覆盖 folder fresh exact bytes、持久 provenance、撤销权限、原 Index Save ACK 后延迟 binding/fsync 到 Platform terminate、旧 Index 消失后的迟到 import、Cancel 后延迟 PNG read/discard、PNG mailbox 拒绝后的实际清理，以及 opaque transaction token/关闭失败/处置行为。新增 observer 原测试也包含在最终 Node 总数中，不重复计数。

最终所有 production Node contracts 170/170 通过（顺序执行，16.585s），包括 observer 14、事务屏障 5；这两组不重复计数。Paste/目录生产关闭针对 38 项包含在总数中。AppFaultObserver fixture 修复只把事件轮数改为真实 writer idle 的有界墙钟等待，生产 observer/Ability 不改变，初次两轮失败日志仍保留。

最终证据和精确源码清单位于 `logs/regression/async-source-continuation-20261008/`，以 `verification.json` 为检查结果索引。四包 all-targets 与 UI direct strict clippy `-D warnings`、workspace/修改 UI 文件 fmt、两 overlay 逐字节 replay、实际 wasm32 probe 和 API26 CompileArkTS 均成功。wasm仍保留原 cms/file_cmds dead-code warnings。CompileArkTS 9.526s、整体 17.013s；22 个 ETS 文件与成功编译的独占 snapshot 完全一致。

产品清单 386 文件，较 #12 新增 7、修改既有 24。`product-source.sha256` 的 SHA-256 为 `d2c0177bb50a0a0403441cf869d0d9ef02066e74c9ab1ec06491f84693aa5716`；`product-source-changes.json` 列出所有 delta。全部 Cargo/rustc/wasm/SDK/Node 进程已结束，编译源和 patch 保持冻结供 root 归档、统一构建。

## 明确剩余项

原 Batch 表单的异步输入、步骤及输出完整接入；脚本直接启动另一个 Batch/Processor/Droplet 仍需要独立目录授权/分配续行。过期或未经原表单授予的逻辑输出 handle 明确报错，不能以这一 guard 声称目录脚本已完整移植。其他 Save/Export、多文件输出、资源 chooser 和 Actions 持久化继续按独立专项处理。

本批仅主机验证和源码冻结；没有构建、复制 OHOS archive、签名、安装或真机操作。新的设备场景及不可变 HAP 由 root 验证和归档。此前 #11 一次进程消失尚未解释，后续成功不能消除该记录。
