# PhotoCraft 文件与资源平台入口剩余审计

2026-10-08。只读核验原始提交 `4337a6227a823a28728e68aed844feab62b3314d`、当前 #11 vendor overlays、Rust wrapper 与 ArkTS bridge。本文是源码证据和下一批建议，**不是新增真机通过记录**。#11 的 377 个产品文件保持冻结；本次仅新增本文，没有运行 Cargo、修改产品源、构建或覆盖 archive/HAP。原始 checkout 保持清洁。

当前源码基线清单：`logs/regression/recent-folder-recovery-20261008/source.sha256`。已有细节分别见 [功能接缝初审](functional-coverage-audit.md)、[#10 Batch/Lens 验证](folder-batch-lens-rust-validation.md)、[#11 Recent/恢复验证](recent-folder-recovery-rust-validation.md)。旧初审表中的已修复条目不能当成当前缺陷。

## 判定方法

沿原 `menu_catalog` → `menus::invoke` / 专属 UI → 实际 engine handler → Services / wrapper / ArkTS 的调用链核对。能执行原算法但只写内部目录，或返回明确的未实现错误，均不能算完整文件功能。另一方面，菜单被原版禁用、原版没有算法、原版有明确格式边界，也不能伪装成本次端口新增缺陷。

以下定位中 `U/` 为 `vendor/photocraft-ui-egui/src/`，`E/` 为 `vendor/photocraft-engine/src/`，`A/` 为 `apps/photocraft-ohos/src/`，`H/` 为 `harmonyos/entry/src/main/ets/`。行号对应冻结的 #11 源。

## 已接入口，避免重复报缺口

| 入口 | 当前事实 | 必须保留的边界 |
|---|---|---|
| New / Open / Open As / Close 系列 / Exit | 原菜单、快捷键、unsaved flow；Open/Place 按原 intent、DocId 处理异步结果；OS 标题 X 进入原关闭流程 | Open As 无参数与普通 Open 共用 picker 是原桌面 UI 行为；不据此宣称提供额外格式重解释面板 |
| Save / Save As / Save a Copy | 原快照编码、选择真实扩展后重新编码、外部 publication ACK；成功 Save 才更新对应编码 revision/name/path；取消或失败保 dirty | 单文件 8 个原版保存扩展，不等于所有多文件导出已接；实际格式保真仍受上游编码器限制 |
| Quick Export / Export As / 原图层单项 Export | 原 Services staged 发布；Quick Export 的 sameFolder 也不直接写外部路径 | sameFolder 原目录直出语义尚非完全等价：当前仍使用系统目的地选择；没有目录授权时不能凭文件授权写父目录 |
| Recent / Home Recent | #11 原 UI 显示友好名、逻辑源去重；重开走私有源 URI 再导入，**无旧缓存 fallback** | #11 设备验收仍待 root；当前实现不能自动使 Revert 或 Linked refresh 也读外部源 |
| Save for Web，无切片 | `U/file_ui.rs:500` 取 pick_save stage，原 `E/web_cmds.rs:709` 写该单一文件；`A/file_requests.rs:579` 检测 engine 写入并送发布 | **不能将整个 Save for Web 报为未接**；切片分支是下面的真实缺口 |
| Measurements CSV / Export-Import Presets | `U/analysis_ui.rs:189`、`U/prefs_ui.rs:1169` 走 pick_save/write 或明确 import intent | CSV 与预设包已有通道，不代表 PAT、任意资源路径或 Migrate Presets 已接 |
| ABR / GRD | `U/preset_files_ui.rs:10` 识别这两扩展，原 Open/拖入进入 brush/gradient importer；Brushes 面板 Import 和 Preset Manager Brushes Load 调用原 open_dialog_file | 可通达与真实第三方文件兼容性是两种证据；不能将其他资源当作普通图像 Open |
| Notes / Scripts Browse / Place Embedded / Place Linked | `A/file_requests.rs:778` 按捕获 intent dispatch；脚本 bytes 转 inline script，不再次读取外部 pathname | Place Linked 当前链接指向持久导入副本，**外部源更新**尚未解决；Script Events 的脚本文件选择未自动获得同样通道 |
| Stack / Contact Sheet / Statistics 输入；Image Processor / Batch / Lens 输入与输出 | `U/folder_ui.rs:68` 明确六个输入 intent、三个输出 intent；原算法、sort、claims/error 保留；目录 publication 与 #11 exact-byte retry | root 已报告 #10 Processor 四格式各 2 个外部重开及 Batch Invert 两输出 PASS；Lens 当时仍在设备验证；不泛化其他 provider 或其他命令 |
| Print / One Copy / Print-to-PDF；嵌套脚本、Batch scratch、Print Script Events | 可选 `Session.print_spool` 复用原 PDF/layout；OHOS session 和继承 scratch 有 hook；PDF 文件走 publisher | 系统受理不是最终打印完成。关闭系统预览 X 未提供可确认 terminal 时保 PDF、状态为最终结果未确认；不写取消 PASS |
| 纯文档 File 功能 | File Info、Fit Image、Conditional Mode、Crop and Straighten 等无需外部 I/O 的算法继续使用原 engine | 不能从菜单可点推断所有参数、性能或设备结果都已验收 |

## 内容来源与链接：最高优先的平台缺口

| 入口与复现 | 真实路径和当前结果 | 风险与所需接缝 |
|---|---|---|
| **F12 Revert**：保存文件 A；在应用外改成内容 B；当前文档编辑后 Revert | `E/file_cmds.rs:240` 直接 read_file(active.path)，替换为一条 undoable history state、保 DocId/name，再令 saved_revision=revision。OHOS active.path 是自己的持久 stage/import | 当前正确回到内部最后保存 bytes，**不会读取最新外部 B**，却成为 clean。需要捕获原 DocId/revision、通过已有源 URI binding 重新授权读取，成功才执行原 Revert；取消/失败/过期目标保持原文档，不新增 tab、不用旧缓存伪装刷新 |
| **Place Linked → Update Modified / Update All / Edit Contents**：链接外部 A；外部替换为 B；触发更新或编辑内容 | `A/file_requests.rs:843` 把 staged import path 传为 Linked；`E/smart_cmds.rs:79` 先查 PSD 内部 linked metadata，再读 Linked.path；`:293–330` 原 refresh/render 消费这些 bytes | stage 不随源 URI 变化，刷新能正常结束但仍得到 A。要在原刷新/编辑前获取对应外部源的新副本，保持原 LayerId、transform/filter/mask 和 undo；Update All 按每个源/层报告失败，不能转成“全部已更新” |
| **Replace Contents / Relink to File**：选 Smart 层，Layer › Smart Objects 点击对应菜单 | `E/smart_cmds.rs:575` replace 需要原始 path，`:827` relink 同样需要 path；`U/menus.rs:435` 无特定 picker handler，最终直接 app.run；`A/file_requests.rs:778` import intent 白名单也无这两项 | 当前原菜单缺用户可用的资源 picker，通常得到 missing path，手输外部 URI也不能被 std::fs.read 消费。加入原位置异步选择，捕获 DocId/LayerId/编辑代际，复用原 decode/set_source，不在完成时猜 active layer |
| **Export Contents / Convert to Linked**：Embedded Smart 层使用对应菜单 | `E/smart_cmds.rs:583` 直接 write_file(path, 原 source bytes)；`:850–862` convert 在原 edit 闭包内先写文件再改为 Linked | 无外部 publisher。若只让它写 stage 后立即改 Linked，后续取消/发布失败会留下与用户选择不一致的模型。先编码/保留**原源文件格式 bytes**，外部 ACK 后才提交原源变更；失败保持 Embedded，Export 不修改父文档 dirty |
| **Reveal in Finder**：Linked 层原菜单 | `E/layer_menu_cmds.rs:341–375` native 非 macOS/non-Windows 分支启动 `xdg-open(parent)`；OHOS 不是桌面 Linux 的 xdg 环境 | 明确错误，不是可用文件定位。应用内部 stage 也不应揭示成用户的外部源；需要 SDK 对真实源 URI 的合法显示/打开能力，若 SDK 无等价能力应如实提示 |

原版边界另记，不能用端口修改悄悄改语义：

* `E/smart_cmds.rs:609–633` 的 Edit Contents commit 会把 Linked 源改为 Embedded `.pcraft`，是原 engine 的行为，不是当前异步桥丢失链接。PSD writer 尚不能写外部 `lnkE`；保存 PSD 的 embed 警告是原格式边界。
* `E/smart_cmds.rs:637–645` 原 on_close 忽略 commit_child 失败，`Session.close` 无 Result。若父文档或父层已经不存在，直接 engine 关闭 child 会丢掉提交错误；这是已有上游边缘数据安全问题，需独立失败归属回归。正常原 Save-before-close 已通过适配的错误保护，不能因这段代码把已验收正常关闭判失败。
* `.pcraft` 保留 Linked.path 不等于对外可移植资源身份：当前 path 可以指向 app 沙箱。后续包/资源清单需明确外部来源与重定位，不应把私有 URI 或设备绝对沙箱路径当 portable link。

## 多文件输出：尚未走目录事务与外部回执

现在 `U/folder_ui.rs:78` 输出白名单严格只有 Image Processor、Batch、Lens Correction；engine `directory_output` hook 的消费者也仅这三个。以下原 engine 算法确实存在，不能以“目录暂不支持”的明确错误算完成。

| 原入口、复现 | 位置与实际 I/O | 当前缺口 |
|---|---|---|
| **Save for Web，切片，Save HTML and Images**：建两个用户 slice，使用原 Web 对话框 Save | `U/file_ui.rs:500–519` 用 pick_save 的 parent 当 dir；`E/web_cmds.rs:745–797` 写 images 子目录、可能 spacer.gif 和 HTML，HTML 使用相对图片链接 | pending 仅代表单一 suggested HTML。HTML 对应 stage 存在时可能只发布 HTML，依赖图片留沙箱；不输出 HTML时原 images 文件也不等于 pending.path，帧尾检测会认为没有目标文件。必须整树发布，断言外部 HTML href 都能解析，不只数一个 HTML |
| **Generate Image Assets**：图层名 `icon.png`，开启 File › Generate › Image Assets，再 Save | `E/web_cmds.rs:1042` 原 asset name/default/variant 与编码；`:1113–1153` 写 doc.path parent 下 `<stem>-assets`。`U/lib.rs:898` successful deferred Save 调用 document_saved **并忽略返回的 assets errors** | 外部 Save 成功仅说明项目文件已发布；assets 留内部 stage 目录且错误可能被 Saved 状态覆盖。需要独立目录授权、不可变版本 job、每 asset errors 和 publication 状态；项目 Save 失败不得生成“成功发布”的资产结果，也不能让 assets 失败倒写成项目未保存 |
| Layers / Layer Comps / Artboards to Files | `U/view_cmds.rs:859–875` 原 dir 字段默认 active.path parent；`E/file_cmds.rs:841`、`E/comps_cmds.rs:347`、`E/artboard_cmds.rs:339` 逐项 save_doc | 原表单无目录选择/预授权和 ACK，默认内部目录可能写成成功。复用目的地 handle→fresh stage→原算法→整树发布；保原选择、命名、格式、错误行为，不新造算法或伪造原 claims |
| **Package**：包含 Linked 层文档，File › Package | `U/file_ui.rs:107` dir 表单；`E/print_cmds.rs:440–497` 复制 Links，重链接 package 文档，再写 pcraft/psd/psb，返回 missing/warnings | 外部源刷新、完整树发布和 package 内可移植重定位均缺。特别是当前 core relink 生成绝对 stage Links path：仅复制树到外部后不能假称外部 pcraft 链接仍可解析。需真实移出 app 沙箱后的重新打开/资源加载 oracle |
| Data Sets as Files | `E/variables_cmds.rs:332–359` 克隆文档、应用每 DataSet 并直接写 dir；Pixels 参数在 `:186–193` 读 raw path | 缺原目录发布表单和像素资源授权；确认数据集不改变工作文档、逐份真实输出与失败提示 |
| Render Video | `E/video_cmds.rs:296` 写 dir，支持图片序列或 animated GIF | 缺 input/output SDK接缝和 publisher。原算法不是 MP4/H.264 编解码器，不能把引入媒体平台能力当作已存在的原功能 |

目录 stage/recovery helper 已能处理嵌套内容，但这些命令的目标捕获、结果结构、模型副作用不同，不能单纯扩大 intent 字符串白名单就宣布全部可用。任何失败重试必须发布已冻结的相同 bytes，不重跑编辑算法、脚本或打印。

## 单文件导出与资源选择

| 入口、可复现方式 | 真实位置 | 剩余适配 |
|---|---|---|
| Artboards to PDF；Paths to Illustrator；Color Lookup Tables | `U/view_cmds.rs:877–880,891–894`、`U/file_ui.rs:108` 提供内部默认 path；`E/artboard_cmds.rs:407` 原 PDF、`E/print_cmds.rs:500` 原 AI、`E/file_cmds.rs:912` 原 cube，直接 write_file | 固定格式 stage/publisher 可复用，但当前没有接；必须外部写入后才报 Exported，导出不改变 doc.path/name/dirty |
| **Color Lookup：Load 3D LUT** | `U/adjust_ui.rs:103–112` 明写“path field stands in for platform picker”；`E/adjust_cmds.rs:70–112` 支持原 data/fileName 或读 file | 内置 LUT 已工作；外部 `.cube/.3dl/.look` 无 chooser。原位置 Browse 后走已有内存 parser，捕获调整层/编辑代际；错误不得替换已有 LUT |
| **Displace map** | `E/filters_ext.rs:402–425` mapPath std::fs.read，或 mapDocument 已开文档；`U/filter_dialog.rs` schema field | 已开文档 map 路径是平台无关可用分支；外部 map chooser 未接，应保 preview/目标身份和错误回滚 |
| **ICC Assign / Convert / Proof / working spaces** | `E/color_cmds.rs:465–479` builtin 或读 `.icc` raw path；原 UI schema dropdown 主要为 builtin，`U/filter_dialog.rs:160` | builtin 和嵌入文件 ICC 不能报缺；外部 ICC 文件选择未接。OHOS wrapper 没有原 desktop `monitor_profile::detect_async`，monitor `auto` 回退 sRGB，不是系统显示 ICC 自动读取通过 |
| PAT Import/Export；Migrate Presets | `E/pattern_cmds.rs:265–292` `.pat` 读写；`E/migrate_cmds.rs:27–64` JSON raw path；migrate 是原 Edit › Presets 菜单 | PAT 无当前 Open 路由，migrate 无原 picker handler，不能误当 ABR/GRD/预设包已覆盖。复用原 reader/writer和受限 owned store，导出走 publication；保持原 merge-by-name 语义 |
| **Install Plug-in / Additional Plug-ins Folder / Reload** | `U/plugin_ui.rs:52` Install 仅生成 path dialog；`E/plugin_cmds.rs:143–172` 支持原 base64模块或 raw path；`:178–210` 加载 prefs folder | 原沙箱 WASM filter 算法已编译，不等于用户可安装。需要真实选择、模块大小约束、durable module library、按用户 opt-in 重载；URI不进入wasmi。原 `:105–110` 缺插件时 smart filter silently skip，重开依赖缺失必须有可见诊断与保 cache 测试 |
| 字体 family chooser / 自定义字体文件 | `U/type_tool.rs:674` 按进程缓存 engine families，`U/type_panels_ui.rs:497` 原搜索chooser；`A/fonts.rs:168` 手工注册 HarmonyOS Sans Latin/SC，缺字时 Noto CJK | root 已真机确认中文真实 glyph；**不是字体列表完整支持**。当前其余 `/system/fonts` 家族未全量发现到文档 engine，运行后新注册也不能刷新 OnceLock列表。后续按系统目录按需注册、可刷新原chooser；不打包字体、不复制渲染算法 |
| History Log 的 Text File/Both | `U/prefs_ui.rs:280–305` Services.append_text；`A/services.rs:158` absolute raw OpenOptions append | metadata history 可用；用户外部 log path 无选择/授权/append发布事务，仅绝对路径不代表合法可写外部 URI。应先durable内部log，再原设置中显式授权或导出，失败可见 |

## 其他原 File 输入与 automation

| 入口 | 核验与剩余范围 |
|---|---|
| Photomerge / Merge to HDR Pro | `U/view_cmds.rs:898–907` paths 默认为内部 dir；`E/photo_cmds.rs:67–84` 同时消费 useOpenDocuments 和 paths；当前 FolderImport 不含这两个 intent。外部文件夹模式未接。选择 open-documents 时应显式去掉路径输入；原 engine 不自动忽略现有 paths，不能假称只勾 checkbox 就完全避开 I/O |
| Import Variables / Data Sets 的 Pixel Replacement | `E/variables_cmds.rs:259` CSV raw path，pixels 每个值读 raw path。当前 import白名单不含该 intent；缺CSV选择和其关联图片授权/身份清单。相对 pixel path 的解释、原CSV parser行为是原算法边界，不能在适配中默默改含义 |
| Import Video Frames / New Video Layer from File / Replace Footage / Reload Frame | `E/video_cmds.rs:158–189` 读取单图或排序图片目录，`:192,:222,:247,:270` 消费 source path；当前无异步资源intent。Reload也只能重读旧stage。只承诺现有图片序列/单图功能；不能把菜单Video二字当成原MP4支持 |
| Create Droplet / Run Droplet | `U/file_ui.rs:72–86` 捕获实际Action，但 path/output 为 raw内部目录；`E/automate_cmds.rs:278–314` 写JSON+默认Unix `.command` shell，执行依赖未打包的 `photocraft-cli`。需要portable `.pcdroplet`发布及合法输入/输出授权；OHOS不得生成后称 `.command` 可执行成功 |
| Script Events Manager | `U/file_ui.rs:100` 原脚本path字符串；`E/automate_cmds.rs:218` 事件执行可读取该路径。Script Browse inline导入已接，但event持久脚本选择未接。嵌入步骤/Print事件已有engine semantics，不能重回“临时禁掉events” |
| Actions 库及一般 UI state 的持久化 | root 在 #10 录制的 Invert Action 升级 #11 后面板为空。`U/actions.rs`、`U/state.rs` 只有 serde data types；`U/lib.rs:435` 新 app 固定 UiState::default，eframe App impl 无 save/on_exit 或 Storage reader/writer；原 desktop main 同样只 PhotocraftApp::new，没 get_value/set_value。OHOS Frame.storage=None，runner也无UI序列化调用。**这是没有持久化消费者，非安全退出flush失败**；engine prefs/Recent/Recovery 保存不含 ui.actions。后续按原库真实步骤/schema安全持久化、支持坏数据错误/限额和重启恢复，不在本次来源刷新批叠加 |
| 嵌套脚本/Actions中的其他**已有 engine I/O commands** | 原 handler 直接 std::fs/read_file/write_file 的调用仍绕过 Services：例如 Revert、Save a Copy、Smart读写、多文件exports、资源imports。Print spool 已补；三个目录表单hook不能自动为任意 nested script 发起授权。需要纯engine可选IO seam与授权准备/明确pending归属，不复制engine算法。`file.save`/`file.saveAs` 属shell handler，原 engine registry没有这些base命令，不能声称原脚本已支持再误报一个平台回归 |
| 系统 Open With / 文件关联启动 | `module.json5:35` 仅home skill；`H/entryability/EntryAbility.ets` 无 onNewWant/onCreate URI输入；Services.os_events未接。当前原系统Open picker与拖拽已可用，但系统双击/其他app Open With 尚非完整集成。需Stage Want URI授权队列，复用原导入与错误显示 |
| 原 TCP/control/MCP 文件权限与截图 | wrapper未提供原desktop控制server，automation_read/write/command未配；`U/lib.rs:1004` 请求Viewport Screenshot，平台capture没有实现Screenshot。当前不应对外声明桌面control协议/MCP完整可驱动或截图成功。需要独立明确授权范围，不允许为了“能调用”开放任意沙箱路径 |

所有当前原 UI 实际产生的文件相关 viewport Commands 已逐项查：菜单 Close/CancelClose、Fullscreen、Maximized、StartDrag、RequestPaste 走现有平台；`Focus`、control 的 `InnerSize/Visible/Minimized`、`Screenshot` 未消费。普通原菜单不依赖后四项，但这是系统 Open With/control 集成时的明确边界。FullOutput 的文本/图片 Copy、Paste、OpenUrl 已有通道，不能再次报为全部静默忽略。

WIA 单独记录：`E/wia_cmds.rs:1–34` 原代码注册该菜单，但非Windows返回 `available:false`，Windows亦尚未绑定真实COM设备。它是原平台专有/未实现采集边界，**不是鸿蒙已有scanner算法漏接**。当前明确 unavailable 也不意味着以后扫描器/相机导入目标已经完成。

## 推荐 #12：只做来源刷新与 Smart 文件事务

按数据安全排序先实施本节；不在同批扩大所有资源、视频和目录 exporter。之后优先 slices/assets/包的完整树发布，再按上表分批补资源 chooser和其余File入口。

1. **Fresh source read**：复用私有 `DocumentBridge.sourceUris`、#11 importRecent 的授权检查/activate和 fresh copy，把明确 intent Revert/SmartRefresh 与捕获的 DocId/LayerId/修订及原 source身份传回来。不向Rust传外部URI，不对既有grant重复persist；缺binding/grant、用户取消、源损坏均保原内容并可见错误。未授权新路径由原位置手动选择，绝不偷偷使用旧副本。
2. **Revert**：成功取得新bytes后复用原import与undoable Revert，保DocId、现name、views和颜色状态；完成前doc关闭/重新编辑即拒旧请求。不能通过“开另一个tab然后关闭原tab”取代原语义。成功clean只表示明确选中的源版本，不假造外部Save回执。
3. **Linked update / replace / relink / edit-source**：保持当前PSDmetadata embedded-linked原优先语义，针对真实 external-linked source刷新；保持原transform/filter/mask，原undo一步。UpdateAll逐源定位并汇总实际failures。跨文档切换completion只作用原DocId+LayerId，改变source/删除layer/换编辑代际则安全拒绝。
4. **Smart Export / Convert to Linked**：使用源的原filename/实际format bytes，独占stage→单文件publisher。Export始终不改dirty；Convert仅外部success ACK后提交原源变更，失败/取消不改Embedded，不能用扩展名换皮重命名bytes。资源持久身份应支持重开恢复，不能只留随机内部path。
5. **失败可操作**：沿原footer/notices/prompt报告，保原文档和有价值的编码副本，重试不重跑编辑/脚本。原child关闭吞commit失败的边缘案例另立最小guard与oracle；若需要改engine接口，用可重放可选hook保持desktop/wasm默认原样。

必要验收，不额外重跑全部原suite来替代结果断言：

* 外部A→B，原F12真正得到B；undo得到原dirty编辑，DocId/view/layer归属保持；cancel/error/late completion不改原内容或saved marker。
* Linked A→B，Update/UpdateAll真实glyph/pixels/源bytes改变，已有transform/filter/mask保持；两个文档同名字的LayerId定位不串；关闭/删除/换源后晚到completion拒绝。
* Replace/Relink真实pickerintent及原选层，坏文件失败不变；重新启动后源授权缺失明确错误，不误读stale copy。
* Export原source字节hash与实际扩展一致、不改dirty；Convert cancelled/failed publication仍Embedded、success成为原Linked，后续外部更新可复现。
* 父层失效时child显式Save失败仍可保留/导出；不要以修改expected掩盖原on_close吞错误。
* 同一host通过必要actual App菜单/Session行为tests、strict/fmt/overlay replay/wasm一次；设备按真实source externalmodify、safeCancel/失败、重开校验，源码成功与SDK实测分别记录。

本审计没有补任何产品能力，也没有将上述明确缺口记为通过。所有未接入口仍需后续实现和真实验收。
