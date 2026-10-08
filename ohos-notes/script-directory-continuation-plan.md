# PhotoCraft #14：目录脚本续行只读设计

日期：2026-10-08。基线为已冻结的 #13，386 产品文件，manifest SHA-256 `d2c0177bb50a0a0403441cf869d0d9ef02066e74c9ab1ec06491f84693aa5716`。本文件只记录代码核验与建议，不属于该清单；没有修改产品源、运行 Cargo/SDK 或构建包。

## 先明确真实入口

| 入口 | 冻结源的真实调用路径 | 当前边界 |
|---|---|---|
| 原 Batch/Image Processor/Lens 表单 | `vendor/photocraft-ui-egui/src/view_cmds.rs:908–934` 捕获原参数、Batch Action steps；原字段 Browse 由 `folder_ui.rs` 产生 dialog/field/generation；`PhotocraftApp::run` 在 `lib.rs:589` 调 Services.process_folder | 六个输入/三个输出 intent 的原表单已接。此 UI hook 不在 Session 调用栈里，脚本不会自动经过它 |
| Scripts Browse、Actions、Script Events 内的 Batch | `automation.rs:179–216` 只拦顶层 Scripts/Batch；`jobs.rs:634` 仅在 permit=None 时开始 root continuation；`automation.rs:542–575` 的普通步骤仍调用真实 Session.start | 嵌套 Batch 落入原同步 `file_cmds::batch:727`，directory_output 缺 PreparedOutput 明确失败；不能用扩大 intent 白名单消除这个缺口 |
| 脚本内 Image Processor/Lens | `file_cmds.rs:755` / `lens_cmds.rs:354` 先 batch_inputs、directory_output，再原 process_files | 无独立输入授权、输出分配、最终 publication 续行；缺 handle 时并不是已完成 |
| 原目录输出授权与执行 | `apps/photocraft-ohos/src/processor.rs:234` 要求 Selection.form.document/command；`:706` 安装一次 command+handle 的 PreparedOutput；`:712` 重新 execute 原命令 | 不应伪造 dialog=0 或用用户此刻 active tab 冒充脚本 owner。原 form 与脚本 Scope 是不同 owner |
| 真实单文件宏命令 | engine 注册了 `file.openAs`、`file.saveACopy`、`file.placeEmbedded/Linked` 等；`file_cmds.rs:300` 读文件、`:270` 真正编码写文件 | 菜单接线不等于 engine 内嵌脚本已有 SDK选择/发布；这些原注册命令仍需独立 scoped file IO |
| Shell Open/Save/Save As/Export As | `ui-egui/menus.rs:220,228,260,300` 调 app.open/save/export dialog；engine 没有 file.open/save/saveAs 的 CommandSpec（OPEN_JOB 是背景导入 label，非 registry spec） | 原 script parser 的 check_known 会拒绝这些 id；不能把这个原错误称为 OHOS 回归，也不能偷偷增加脚本语言语义 |
| Script 文件与持久 Event binding | `automate_cmds.rs:115–139` 的 script_steps/binding_steps 仍直接 read_file(path)；`:231` 保持真实事件路由 | 顶层 Browse 经过 picker→inline text 已接；嵌套 path 与 Event Manager 的脚本文件还缺 fresh private source resource 接缝 |

参数完整的 command form 最终执行 Session；缺参数的原菜单可以弹 form，但 Scripts/Actions 原本是 engine 参数调用，不会通用弹出任意菜单对话框。#14 应先调用原 parser/known-command/参数校验；缺 steps、非法 format/profile、错误参数类型等不得先开系统选择器再替用户修正。合法的外部目录路径或已记录逻辑 handle 才进入授权续行。

## 两个不能绕过的调度问题

1. `automation::tick_automation:372–390` 只推进队首 run，等待中的父 run 被 push_front。直接在它后面 enqueue 子 Batch，再让父等待子 JobId，会造成子 run 永远得不到 tick。即使局部解决 JobId，也没有保存多层 scratch 所有权。推荐同一 Run 的执行栈，而不是未经验证的子 JobId捷径。
2. Processor.State 当前只有一个 job，Platform.folder_slot 从 acquire 到外部发布结束一直被占用（`platform.rs:220`）。外层 Batch 保留自己的 output stage 时，内层目录命令无法选择/分配/发布；这不是 SDK picker 问题。必须区分 durable stage 所有权和短期平台 I/O lease。

## 推荐的最小模型

将同一 Run 的执行栈扩展为 Sequence 与 DirectoryProcess 两类 frame。DirectoryProcess 保存原 command、caller params、执行参数映射、process_files 计划、当前 scratch、parent return token 和 owned publication 状态。父 Sequence 的 next 指针在子目录结果最终返回前不前进，父 scratch Session 也不被移动到公共 app session或丢弃。

保持 RunId 不变，保留当前 Scope `{run, session, step}`。session=0仍表示 main；新增 scratch token 必须在整个 run 内单调唯一，不能各嵌套 Batch 都从 generation=1 开始。scope.step/call token 在进入目录调用时固定；内层步骤有自己的 token。每个 completion 只能满足一个仍 pending 的 token，重复、迟到、父已取消、Session 已关闭、DocId/Arc/revision/selection 已变都不能满足新的调用。

输入和输出请求通过可选、纯数据 engine callback 排队，callback 不取得可变 Session，不调用 egui/ArkTS、不阻塞 worker。结构至少包含 command、原 params、Scope、call generation 和可选原 document guard。None 保留原桌面/wasm同步路径。root 表单与脚本 owner 用显式 enum区分，避免制造不存在的 UiState dialog。没有文档的独立 Processor/Batch仍可运行；不能强制以当前 tab 作为授权目标。

复用现有 typed folderImport、folderDestination、folderStageAllocate、folderPublish 和 cancel/release 协议。wire 的 target 是该目录调用的 opaque target，各阶段 request id 独立；URI只在 ArkTS private journal。Rust保存 wire id→捕获 Scope的映射，壳层不需要知道 engine 栈。一次调用在 input、destination、allocation、publication 全程使用同一 target，Stage owner.creationId 是 allocation id，绝不是 publication attempt id。

建议状态顺序：

```text
Validate → AwaitInput → AwaitDestination → AwaitStage
         → ExecuteOriginalProcess → FreezeOutputs → AwaitPublication
         → ReturnOriginalResultToParent
```

AwaitInput 对已合法绑定的目录/文件来源重新获取新 bytes；未绑定来源由这次调用独立选择授权。不能复用上次表单的老 copied tree 当最新外部文件。当前 top-level list_images、Rust full-path排序、sourceBasenameChild、no-link、完整 manifest与500文件/1GiB限额保持。缺 binding、grant撤销或 provider异常明确失败。scope有效且 queue true 才移交新 copied owner；接受后发现过期由 Rust清理，unknown/duplicate不碰其他已接受 owner。

AwaitDestination 总是检查这次调用的 handle/target/grant；可明确重新选择目录，不能默用已用掉或属于别的 form/run 的 PreparedOutput。PreparedOutput 增加精确 Scope/call identity，以 command+handle+call单次消费，并在异常/取消 finally 撤销；原 journal保留 caller逻辑参数，stage路径只用于执行映射。对已持久授权 handle仍是 check→activate，不重复 persist。

Batch继续复用已提取的 FileProcess、原 parse_steps 和 OutputClaims。ImageProcessor/Lens只将原每输入 callback 提取成共享 helper/plan，默认同步入口与续行入口共用同一 helper；不重写 fit、sRGB、flatten、lens、save_doc或编码。过程中的源读取、Print、真实背景作业继续走 #13 的 Scope owner 和 per-step等待。

stage所有权保存在有界的 per-call job map/栈中。系统 I/O lease仅在实际 picker、copy、allocate、publish以及 receipt/清理尚未 settle期间持有；空 stage 已经 validated transfer后、纯 engine处理期间可以让嵌套调用取得自己的 lease。不能在复制仍读/写该 owner时提前释放。各 job的 allocation/attempt/recovery独立，当前原表单用户新请求仍按真实 busy策略拒绝或排队，不能覆盖脚本 job。

Publication成功以实际 SDK终态 receipt和合法清单为准，之后才父 frame.finish、原步骤 journal/event和后续步骤。编码成功只意味着 stage有 bytes。原逐文件 errors仍原样关联输入，同名冲突/失败输入不能因发布成功被清空。无输出可发布时不能制造成功 receipt。

## 取消、恢复与事件

原 Action Stop、Jobs Cancel/Esc 或关闭父文档，取消该 run所有尚未返回的子调用。已在系统执行的请求只发精确 id+target Cancel并等待 settle；销毁/关闭沿 #13 drain，不靠超时放行。未开始的后续写入/打印绝不执行。

父失败传播保持原语义：输入/编码/发布失败结束该步骤和后续步骤，不因保存了 recovery record而继续。保留已冻结 bytes、实际逐输入 failure details、owner marker/target/journal供原恢复 UI。Retry只能发布精确 bytes，不能重跑 script/engine/print，也不能在进程重建后悄悄恢复已经终止的调用栈。如果未来提供 live Retry-resume，必须在原 Scope仍 pending、用户明确 Retry且完整 guard有效时才能返回父调用；它是另一个显式状态，不能把终态失败改写为 pending。

SDK外部 copy已成功、completion queue暂满之后发生 Cancel，仍保留真实成功 receipt。可以停止父后续步骤并报告已发生的外部副作用，不能谎称无输出。partial/unknown状态保留 stage/journal，不删除仅存的新 bytes。

事件返回保留 ReturnTo::Event。递归守卫只覆盖该事件及其实际执行的子 frame，不覆盖整个系统等待期；不得全局关 Script Events。scratch保持原默认 prefs和原算法，仅继承必要平台服务；不要把 main 的所有 bindings/prefs复制到每个 scratch而凭空增加事件。源/目录/背景作业结果都使用同一显式 Session token，equal JobId不再是归属依据。

## 第一批必要回归

- 真正 Script→Batch→两张输入→每张 Revert/Invert→发布：主文档不改，原格式、claims和errors保留，父 Invert/后续写入必须等外部 ACK后才发生。
- Batch某个 scratch的 Action→内部 Processor/Lens：外层 stage仍保留，内层独立选择、分配与最终发布后回到同一个父 scratch；跨层同名 DocId/JobId/session generation不能串结果。
- 输出选择取消、Stage分配失败、源权限撤销和最终 publication失败：父后续真实 file.saveACopy不写；正控制先证明该 writer能写真实 PNG。部分 bytes进入原恢复入口且 Retry hash不变。
- 取消期间 copy尚未 settle、copy成功但 mailbox满、重复/错target receipt：原 SDK ownership不丢、不假取消，父只返回一次，关闭不提前 terminate。
- Print Event→目录调用→内层 Print：原递归守卫阻止重入，accepted print不等于系统最终打印完成；恢复只发布树不重跑Print。
- 原无hook同步 Batch/Processor/Lens结果和caller journal保持；Missing steps/unknown command/非法参数在系统调用前保持原错误。

## 同批跟进的 SDK floor 守卫

产品 `harmonyos/build-profile.json5:6` 的 compatibleSdkVersion 是 API20，target 是26。公开本地SDK `ets/api/@ohos.file.fs.d.ts:3163` 明确 `stat` 的 URI 参数从 API22 支持；这和 sandbox path/FD 的 stat 基础版本不是同一能力。当前 FolderDestinationBridge 的 `destination-stat` 与 FolderPublicationBridge 的 `folder-destination-stat` 调用 `fileIo.stat(uri)`，只有粗粒度 canIUse checks，不能据 API26编译成功宣称目录输出API20完整支持。

#14 独立补真实 `deviceInfo.sdkApiVersion` 守卫：低于22/版本无效时明确拒绝该目录URI能力，并保留已有stage/journal；不调用不支持的stat，也不在拒绝之后启动provider写入。新选择及已有handle resolver/publisher两种路径均需守卫。保留basic20供其余可用功能，不能假造canIUse成功或自动升级授权。生产SDK tests覆盖 runtime20/21 无URI stat/外部copy、26原正常流程和grant撤销。当前#13冻结源不回改；没有做低版本真机声明。

## 剩余原 File/OS入口的实施顺序

以下是 #13 当前代码事实与下一路线，不是通过清单。旧 `file-platform-remaining-audit.md`以 #11为基线，其中 Revert/Smart的最高优先缺口已由 #12/#13接入，应结合新验证文档阅读。

| 优先级 | 原入口与证据 | 下一接缝与验收 |
|---|---|---|
| P0 | Save for Web切片/HTML：`web_cmds.rs:745–797` 输出 images/HTML/spacer；UI仍以单一 pick_save parent执行 | 整树 directory publisher；外部 HTML所有相对href实际能打开，不能仅发布HTML便报成功 |
| P0 | Generate Image Assets：`web_cmds.rs:1042–1153`；deferred Save的 document_saved仍可产生内部树/错误被Saved覆盖 | 项目Save与assets job分开；独立授权/不可变revision/完整publication，asset失败不改项目saved marker，也不被项目Saved掩盖 |
| P0 | Package：`print_cmds.rs:440`复制Links并重链接，原输出可能含 stage绝对path | fresh来源、整树发布、可移出app沙箱重新打开的portable链接；光复制目录不证明工程可用 |
| P1，本 #14 | 三个目录命令的 nestedScripts/Actions/Batch执行栈 | 本文的 Scope/frame/lease接缝；不得仅扩大字符串白名单或把明确拒绝当完成 |
| P1 | 原真实 file.saveACopy/openAs/place、nestedScript.path/Event.script | scoped单文件资源read/publish和原handler完成；输入fresh，输出最终ACK，原失败/格式/父文档dirty不变。ScriptEvents Manager文件选择需原位置Browse；纯 engine不引入菜单ctx |
| P1 | Layers/Layer Comps/Artboards/Data Sets to Files：原 file/comps/artboard/variables_cmds；当前output白名单只有三项 | 按原命名/claims/错误与模型副作用分别接整树publisher，工作文档结果逐项断言 |
| P2 | Create/Run Droplet：`automate_cmds.rs:287–365`真实JSON+Unix.command shim、Run读取后调用Batch | fresh `.pcdroplet`解析＋本文目录续行；portable JSON发布可实现，OHOS无未打包CLI不能称Unix shim可运行 |
| P2 | ContactSheet/Statistics/LoadStack宏输入、Photomerge/HDR、视频图序列、变量CSV及pixel inputs | 原算法前合法输入快照/原排序、Scope归属；不能把现有图片序列算法称为MP4支持。useOpenDocuments模式应明确去掉目录paths |
| P2 | LUT/ICC/Displace/PAT/Migrate/plugin resource chooser、PDF/AI/CUBE单文件export | 原UI位置chooser与原内存parser/encoder，资源模块有界durable store、export按外部ACK；缺插件诊断不能quiet skip伪称渲染完整 |
| P2 | Actions及一般UI状态持久化 | 原UiState虽然serde但原app无Storage consumer，非退出flushbug；安全schema/限额/坏数据与重启恢复专项 |
| P3 | Reveal in Finder的xdg-open；系统Open With/Want文件关联；monitor ICC/custom fonts/history text log | 合法SDK入口或如实能力边界，真实URI映射、没有外部路径假写；原CJK字形已验不代表所有字体家族chooser完整 |
| P3 | 原control/MCP运输、automation capability IO、Screenshot/Focus/InnerSize/Visible/Minimized输出 | 独立授权控制协议与平台结果；当前菜单Close/viewport/cursor/clipboard已经接，不重复报为全静默 |

非Windows WIA与原menu_catalog未实现项另列原版边界：不能凭菜单文字编造已有scanner算法，也不能把明确unavailable记作完整总目标。

下一实施建议先完成本文三目录 command的同Run execution stack/independent grant闭环，再给单文件macro与持久script资源一个独立范围，随后处理切片/assets/package的实际数据完整性。所有产品修改必须等 root确认 #13 实际不可变包完成之后。
