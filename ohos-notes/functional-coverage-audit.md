# PhotoCraft OHOS 功能接缝审查

2026-10-08，只读核验原始基线 `4337a6227a823a28728e68aed844feab62b3314d`、当前 vendor UI 和 OHOS wrapper。此文记录代码与测试证据，不代表全部菜单已经在真机验收。Rust 平台批次已冻结；本次只新增本文，没有更改代码或重新构建 archive。

后续实现更新：下表保留初审时的具体触发证据。51-test 包已接 cursor、三种 root
viewport 命令、成功 Save 持久标记和同格式实际文件名；设备 CJK 字形与可见 PasteButton
授权粘贴已有结果，完整 IME 仍在修复与复验。下一打印批主机通过原 Print/One Copy 与
Actions、完整原 PDF 字节 oracle、系统七态任务协议和 Print-to-PDF 发布。主菜单不启动
`lp`，提交不等于打印完成。后续最小 engine overlay 已将嵌套脚本、Batch/Droplet
和原 Print Script Events 接入同一异步 spool service，保留原 PDF 算法和事件递归保护；
跨文档及 Batch 临时会话的 pending 归属有主机测试。系统打印机/份数预设及真实系统
任务终态仍需验收，详见 [platform-protocol.md](platform-protocol.md)。

#8 源码更新：原 Load Files into Stack、Contact Sheet II 和 Statistics 的字段行现有
Browse/Cancel 与目录请求。完成仅填原文件数组，用户确认才调用原 engine；原本层
枚举/排序复用 list_images。67 项 host/严格检查/overlay 重放/wasm 通过，详见
[Rust 验证](folder-input-rust-validation.md) 和 [目录协议](folder-input-protocol.md)。
商业 provider、实际 picker 和 UI 外观待 #8 真机 fixture；下表的目录初审条目仍适用于
尚未接线的 Batch/其他多文件输出，不应再解读成这三个输入表单尚无代码接缝。

#9 源码更新：原 Image Processor 的 Input/Output Browse、输出预授权 logical handle、
Confirm 后独占 stage、原 engine 四格式编码、完整目录 publication 已接。75 项 host、
原文件命令16项、四包 strict/fmt、两个 overlay replay 与 wasm 通过；逐文件错误、
忽略大小写的同名输出冲突、原文档 dirty/identity、取消与 delayed completion 均有
真实文件回归。只确认编码树与 SDK terminal receipt 后报告 published，目录 provider
实测仍待本批设备验收。详见 [Rust 验证](folder-processor-rust-validation.md) 和
[预授权/发布协议](folder-processor-protocol.md)。原同步编码器的抢占取消及任意嵌套
engine script 的目录准备不在此宣称已经完成。

后续设备结论（包 #7/#8）：稳定 IME attach、系统中文候选选词/提交/PSD 保存外部重开、
原 PDF 输出与系统 Print to PDF 任务 completed、OS 全屏/还原、安全保存退出、
本地 provider 的目录复制契约、原 Stack/Contact Sheet 参数与图层、两个文档交替保存
并分别外部重开均已通过。真实键盘、预编辑取消、图片粘贴、实体打印及其他 provider
继续待测。下表保留初审时的缺陷证据，已修复项不再表示当前代码仍静默忽略或调用 lp；
最终包和逐项结论统一以 [功能验收记录](../docs/photocraft-validation.md) 为准。

## 已有覆盖及其边界

| 功能 | 当前实现和证据 | 尚需真机确认的边界 |
|---|---|---|
| 原版编辑器 | `PhotocraftApp.logic/ui`、原菜单与快捷键；GLES 渲染 egui，CPU 文档合成；真实设备已出首帧与笔刷笔迹 | 复杂文档性能、持续运行、内存、GPU 文档合成不在已通过范围 |
| Open / Open As / Place | 单文件异步 picker，持久沙箱导入，完成按原意图与原目标文档处理；Open/OpenAs 已有设备验证，Place 两种意图有主机测试 | Place 的外部链接刷新见下表；复杂文件保真仍由上游能力决定 |
| Save / Save As / Save a Copy | 原 UI 编码，异步发布；选择扩展后用不可变文档快照真正重编码，8 个原版保存扩展；仅成功 Save 标记对应文档和编码版本已保存 | 已保存 PNG 的跨进程目标复用见下表；大文件和外部存储故障需更多设备验证 |
| Export As / Quick Export / Layer Export | 单文件 Services.write 或 stage 存在检测；导出与保存副本保留 dirty；Quick Export 的 sameFolder 也走发布接缝 | 对目录、多文件输出没有完整覆盖 |
| 外部发布可靠性 | 取消、编码失败、发布失败保持 dirty；外部旧文件备份/回滚；严格匹配当前 pending stage 才允许错误保留新副本 | ArkTS 故障注入通过是主机证据，真实外部 provider 行为仍需实测 |
| Recent / Revert / 恢复 | 持久 imports/staging 文件、Preferences、Autosaver 与原 UI 恢复；URI 映射持久在 `source-uris.json`；真实设备恢复过未保存笔迹 | 进程生命周期及关闭后同 PID 再启动需单独验证 |
| IME / 文本及图片剪贴板 / 安全退出 | bounded 非阻塞输入、独立输出快照；UTF-16 选区、preedit/commit/cancel/delete；显式 Copy/Paste 才访问系统；原 CloseAll/unsaved 流程与关闭请求 id；实际 App 主机测试通过 | 最新平台 HAP 正在设备验收；主机模型测试不能代替实际输入法和系统 clipboard 验证 |
| UI / 文档中文字体 | UI CJK lazy fallback；文档运行时注册 HarmonyOS Latin/SC，缺字时单个 Noto CJK fallback；字体家族和缺字探针日志，不输出用户文本 | 原版字体选择器尚非完整 OHOS 字体目录；设备中文字形需截图确认 |
| 鼠标、触屏、快捷键、HiDPI、pinch | 指针/滚轮坐标按 density 和 UI zoom 转换；键修饰符、repeat、blur 释放；`craft_zoom` 差分因子送真实 `Event::Zoom` | pinch 新包待实测；笔压、倾斜及橡皮端需要硬件 |
| Help 链接 | 所有 `OutputCommand::OpenUrl` 已转系统输出事件，ArkTS 打开浏览器；CopyText/CopyImage 同样已消费 | 浏览器唤起/返回仍需设备验收，不是未接线的网络功能 |
| Preferences / 预设 | 沙箱 prefs、preset_store；预设导入保留原类型参数；CSV、pcpresets 等固定扩展可以使用同一单文件发布协议 | 仍有上游本身未生效的设置；无权把它们称为本次新增回归 |

最新平台主机测试日志 `/tmp/photocraft-platform-tests-final.log`：43 个通过（输入适配 7、eframe facade 1、PhotoCraft 35），0 失败。严格 clippy、fmt check、688 行 overlay 逐字重放通过，原始 upstream 工作树未修改。主机覆盖取消/失败不假保存、原文档与编码版本、多格式图层保留、真实 TextEdit 和文档中文输入、UTF-16 代理对、图片 Copy/Paste、PNG cache 所有权、安全退出、真实 `ctx.zoom_delta`。这些数字不能解读为 43 项真机验收。

## 确定的剩余 OHOS 差距

| 建议顺序 | 路径 / 原功能 | 当前具体行为 | 可复用的最小下一步 |
|---|---|---|---|
| 1 | `apps/photocraft-ohos/src/platform.rs:375`，`view_cmds.rs:504`、`panels.rs:294` | root viewport commands 全部被取出，仅处理剪切/复制/粘贴和安全 Close。原生产 UI 确实发送 `Fullscreen`、`Maximized`、`StartDrag`，当前静默忽略；F 切屏幕模式只改变原 UI 状态，未切 OS 窗口 | 输出有限的明确窗口请求；UI 线程通过 Window API 应用；需要 ACK 的错误回原状态栏。不要把原控制通道的 Screenshot/resize 误称已支持 |
| 1 | `platform.rs:306`，原 `transform_tool.rs:690` | `PlatformOutput.cursor_icon` 未桥接；文本、边缘缩放、旋转等系统鼠标指针没有原 egui 指针语义 | 增加只读 cursor 快照，Native 设置指针样式；无需每次产生无限事件 |
| 1 | `file_requests.rs` 的 `published` 集合、`services.rs:save_in_place` | 成功保存 PNG 的同进程 Ctrl+S 正确复用目标；published 集合未持久化。重启后 OpenRecent 打开同一 stage PNG 时，即使 URI binding 存在，也会重新走 Save As | 仅在成功 Save 完成后持久标记、重启核验恢复；不得凭 staging 路径判断已发布，失败/Export/SaveCopy 不能授予 in-place 权限 |
| 2 | 原 `file_ui.rs:501`、engine `web_cmds.rs:709` | 无 slice 的 Save for Web 单图走所选 stage，`finish_frame` 可检测并发布。有 slice 时 UI 把所选 path 转成 parent dir，engine 写 HTML、images 和 spacer；pending 主 path 没有文件，单文件请求会取消，整套未发布 | 增加输出目录/多文件事务，而不是仅改后缀或给单文件加假成功 |
| 2 | `web_cmds.rs:1146`、vendor `lib.rs:783` | Generate Image Assets 以及成功 Save 后生成的 `<stage-stem>-assets` 只在沙箱生成；不进入外部发布协议 | 把原返回 files 列表纳入目录发布，保留失败可恢复产物 |
| 2 | engine `file_cmds.rs:599/667/687/709/832`；原 `view_cmds.rs:842` | Batch、Image Processor、Load Files into Stack、Layers/Comps/Artboards to Files、Data Sets、Contact Sheet/Statistics 等真实实现使用普通路径、`read_dir`/`read_file`/`write_file`。原表单仍接受路径，OHOS 没有 folder/multiselect 许可和多文件发布流程 | 复用原 engine 命令，把授权输入复制为 sandbox 集合，提供持久目录输出与多文件 publication。Photomerge/HDR 的 useOpenDocuments 可继续原内存路径，不应全部标为不可用 |
| 2 | engine `smart_cmds.rs:575/583/827/831`；file `placeLinked` | 基本 PlaceLinked 已保本地导入快照。外部 URI 后续修改不会自动更新这份快照；Replace/Relink/Export Contents 仍用原手输 path。Update Modified Content 只读本地 copy。Edit Contents 在内存父子文档中保存仍可用 | 保留 doc/layer/意图的异步文件 hook，以及明确的 URI 重新导入/授权发布；不能用当前 active 文档猜目标 |
| 3 | engine `print_cmds.rs:316/373`、原 `file_ui.rs:524/718` | Print/Print One Copy 真正渲染 PDF，直接写 temp/path 后执行 `lp`。这条 desktop CUPS 调用未映射 OHOS 打印服务。Print-to-PDF 也只有手输路径，未走系统发布 | 复用 layout/print_pdf，沙箱 PDF → 原生打印请求/系统保存；取消/失败可见，不伪报已发打印机 |
| 3 | 原 `file_ui.rs:116`、`view_cmds.rs:861/873`；engine `print_cmds.rs:524` | Package、多画板 PDF、Paths to Illustrator、Color Lookup Tables、Create Droplet 等输出真正存在，但原表单普通 path/dir 绕过 Services.write；通常只写沙箱，外部用户不可选目标 | 单文件先接现有 publish，多文件归目录事务。Droplet `.command` 默认调用不存在的 `photocraft-cli`，不能宣称 OHOS 可执行 droplet |
| 3 | engine `layer_menu_cmds.rs:361/373` | Reveal in Finder 在 OHOS 落入 `xdg-open <parent>` 分支；原 desktop 进程启动无法提供 OHOS 文件管理器选中文件 | 通过系统文件/URI能力定位授权源；保持无源时明确错误 |
| 3 | 原 `plugin_ui.rs:49`、engine `plugin_cmds.rs:169/190` | WASM 插件运行核心已经编译，不能据此称不能运行；Install Plug-in 和 Additional Folder 原表单仍为普通 path，尚无 `.wasm` picker、sandbox 安装/许可流程 | 显式导入 `.wasm`、持久插件目录，调用原安装/运行接口；真实插件 ABI 主机测试可复用 |
| 3 | engine `adjust_cmds.rs:124`、`color_cmds.rs:475`、`filters_ext.rs:411`、`pattern_cmds.rs:268` | LUT、用户 ICC、Displacement Map、Pattern/CSV Data Set、Preset Migration 等额外资源读原 path；现有 Open 意图集合没有全面接入它们 | 同样的异步 resource intent 和目标快照；保留错误信息。不能将内置 profile/内置算法一起标为失败 |
| 4 | `fonts.rs`、原 `text/src/fonts.rs:353`、`type_tool.rs:674` | 初始化只注册少量系统字体现用 fallback；原系统目录扫描不含 `/system/fonts`，TC/Italic/Serif/Mono/FZ 等没有完整 chooser。families() 为进程 OnceLock，一旦显示后注册新家族也不会刷新菜单 | 用公开 FontDb 字体路径加载和可刷新家族快照；后台按需注册，不能初始把全部大字体读入内存 |
| 4 | `runner.rs:61/103`、engine `display_color.rs:161` | 文档 ICC 转换与 Proof 的原逻辑仍在。OHOS 没有 desktop `monitor_profile::detect_async` 的平台 profile 注入，Auto monitor 回退 sRGB；GLES egui 输出不等于文档 compute GPU/HDR 已验证 | 单独接系统显示 profile 与屏幕变更，测量色彩/性能；GPU composer 保持独立 gate |
| 4 | 原 `canvas.rs:2362` / egui `show_viewport_immediate` | `Context` 默认 `embed_viewports=true`，New Window/Float Window 显示为同一窗口内的 egui Window。runner 没有额外 native viewport renderer，不能独立 OS 窗口或移动到另一显示器 | 明确支持嵌入视图或接多 native window/viewport 生命周期；不要错误禁用整个多文档系统 |
| 4 | 原 `prefs_ui.rs:280`、wrapper `services.rs:append_text` | History Log 文本文件直接 append 普通绝对 path，未系统许可或发布；原 UI 还忽略 append 的返回错误 | 将持久日志放沙箱并提供显式导出，或接授权日志文件。Metadata 模式继续原逻辑 |

目录、资源导入和窗口命令应作为通用平台接缝逐步覆盖，而非把 engine 算法复制到 OHOS 目录。主 HAP 的 Ability 同 PID 再创建 / worker Session 生命周期目前属于正在设备验证的问题，尚不能把未经复现的猜测登记为确定缺陷。

## 复用编辑回归，无需再造同类测试

`/tmp/photocraft-upstream-regression.log` 已记录以下原 engine 真实结果测试通过；它们断言像素、结构或历史状态，而不只是菜单 id 存在：

| 区域 | 原测试示例与源码 |
|---|---|
| Brush / Erase | `brush_cmds/tests.rs`: stroke_backwards_compatible、replay_is_deterministic、live_stroke_matches_the_committed_stroke、eraser_on_background_layer_paints_the_background_colour；`eraser_cmds/tests.rs`: 多深度/模式的 Magic/Background Eraser；`engine/tests/eraser_locked.rs` |
| History / Layers | `engine/src/tests.rs`: layer_lifecycle_with_undo、coalesced_edits_share_one_history_step、fill_clear_and_masks、translate_moves_pixels_and_respects_locks、layer_locks_are_set_and_enforced |
| Selection | `selection_cmds.rs`: magic_wand_contiguous_and_modes、lasso_polygon、modify_commands；`engine/src/tests.rs`: selection_modes、marquee_feather_and_anti_aliased_ellipse |
| Shape / Vector mask | `vector_cmds/tests.rs`: shape_create_every_kind_and_undo、shape_create_fill_stroke_and_composite、vector_mask_commands_and_compositing、translate_vectors_moves_shapes_and_masks |
| Mask | `engine/src/tests.rs`: painting_can_target_the_layer_mask；`layer_menu_cmds/tests.rs`: mask_apply_from_transparency_hide_selection；`mask_view_cmds/tests.rs`: painting_in_mask_view_paints_the_mask |
| Filters / Adjustments | `filters.rs`: every_filter_command_runs_changes_pixels_and_undoes、selection_limits_the_filter；原 adjustments 与真实 OHOS Ctrl+I → PNG 反色像素测试 |
| Transform / Move | `transform_cmds.rs`: scale_via_quad_and_undo、with_selection_only_selected_pixels_move、linked_raster_mask_moves_with_the_pixels、shapes_move_and_transform_as_vectors |

原 UI 还已有 canvas 实际拖笔、Move Auto-Select 实际拖层、transform_tool commit/cancel/selection/mask、mask thumbnail 实际点击、adjust_dialog preview/cancel/commit 的交互结果测试。它们存在于原源码，但不应把“存在”写成当前 OHOS 自测已运行。下一步优先复用这些原测试与真实设备验收；本次没有新增冗余 integration test，也没有把上游 disabled 命令或功能占位改成强制移植任务。

## 不属于本次移植新缺口

WIA scanner/camera 是原版 Windows 专属占位，原实现也未实际设备传输；云文档、Adobe Fonts/UXP/.8BF、生成式 AI、完整 Photoshop PDF/SVG/TIFF/RAW 保真等原能力边界须按 pin 的实际源码判断。上游 `roadmap.md`、`scorecard.md` 明确菜单 live 百分比不等于专业行为完成；部分生成文档比源码滞后（例如当前已存在多画板 raster PDF）。本审查不使用“菜单全部有 id”宣称全功能，也不为尚未存在的上游算法编造 OHOS 成果。
