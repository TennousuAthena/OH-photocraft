# PhotoCraft 鸿蒙 PC 功能验收

本记录保留 2026-10-07 至 2026-10-08 的移植验收历史。文中 `logs/`、HAP、截图和源码快照均指开发者本地证据，已从 Git 仓库排除；仓库发布不包含签名材料或设备日志。通过项仅对应文中明确标注的设备、源码批次和包版本，待测项不因后续主机测试通过而变为设备通过。


更新日期：2026-10-08。本文记录实际测试结果、复现步骤和剩余工作；编译成功、按钮出现和菜单注册均不算功能通过。

## 当前结论

首版签名包已在 API 26 MateBook Pro 上启动，GLES adapter 为 Maleoon 916。新建、触屏笔刷、Ctrl+N、Ctrl+I、测试按钮的 PNG 保存和未保存文档恢复已有真机证据。

用户在首版正式菜单验收中报告：Open、Open As、Save、Save As、Save a Copy 无响应，拖入文件也不能导入。首版临时 PNG 按钮没有覆盖正式菜单的系统文件服务。后续修复包已通过 PNG 打开、PSD Open As、PSD 原目标保存、PSD 另存与分层副本重开，以及单个 PNG 从系统文件管理器拖入。**尚有格式、失败恢复、输入和其余功能待测，PhotoCraft 尚未完成整体验收。**

修复包已移除临时顶部操作栏与重复状态栏，通过原 File 菜单、原快捷键和文件拖拽调用鸿蒙系统文件服务。保存先完成外部文件写入，再更新文档的已保存版本；取消、写入失败、保存后关闭和切换文档均继续单独验证。

## 测试记录规则

- 每次设备测试记录源码版本、签名包 SHA256、设备/API、操作步骤、实际结果和日志或截图。
- 区分主机自动测试、鸿蒙交叉编译、真机自动测试和用户手动验收。
- 取消或失败不能留下假成功提示、清除未保存标记、覆盖另一份文档或丢失未保存内容。
- 导出和 Save a Copy 保留原项目的路径与未保存状态；普通保存成功后才更新保存状态。
- 上游菜单覆盖数字描述命令注册情况，不代表本平台所有功能正确。未执行的项目保留“待测”。

## 文件功能优先验收

| 编号 | 操作 | 通过条件 | 当前状态 |
| --- | --- | --- | --- |
| F01 | File → Open 和 Ctrl+O，选择 PNG/JPEG | 系统选择器打开；导入后尺寸、颜色、名称一致；焦点回到编辑器 | PNG / JPEG 真机导入通过 |
| F02 | File → Open As | 系统选择器打开；执行原菜单对应的导入行为 | PSD 真机选择、导入通过 |
| F03 | 打开有图层、透明度、文字的 PSD/PSB/.pcraft | 可编辑；图层及预览不丢失；格式警告可见 | 两图层 PSD / pcraft、含中文文字与矢量矩形的三层及七层 PSD 保存并从外部重开通过；四层 PSB 另存/独立重开通过；其他样本待测 |
| F04 | 新文档 Ctrl+S / File → Save | 选择路径；文件真正写入后清除未保存标记 | 新建、绘制、首次保存 PSD 真机通过；PNG 首次保存通过 |
| F05 | 已保存项目编辑后 Ctrl+S | 写回该文档对应的原目标；不覆盖别的文档 | 重开 PSD 编辑后直接保存、两个独立文档交替保存并分别外部重开通过；重启后原目标再次保存待测 |
| F06 | Save As，改名或选择另一目录 | 新文件可重开；原文件保留；当前文档保存状态正确 | PSD/PSB 改名与独立重开通过，PSB 四层保留；TIFF/TGA/OpenEXR 实际格式保存、原警告及尺寸画面独立重开通过；PNG 工作文档切换 pcraft 保存并重开两图层通过 |
| F07 | Save a Copy | 副本可重开；当前工作文档路径及未保存标记不变 | 分层 PSD 副本重开通过，原文档 dirty 保留 |
| F08 | Export As 与 Quick Export as PNG | 导出格式/参数生效；原文档仍保持其保存状态 | Quick Export 写入通过；Export As 50% PNG 重开 960×540 通过；dirty 与其他参数待测 |
| F09 | 从文件管理器拖入一个和多个文件 | 文件顺序导入；权限/不支持格式错误可见；焦点恢复 | 单个及两个 PNG 一次跨应用鼠标拖入通过，顺序/独立 tab/尺寸正确；不支持格式及权限错误待测 |
| F10 | Open/Save 的选择器取消 | 文档内容和保存状态不变；可继续操作并再次打开选择器 | 新建 Save 取消后继续绘制/再次保存、Open 取消后保留 clean PSD 与形状通过 |
| F11 | 写入失败、权限失效、目标不可用 | 提示错误；文档仍可继续编辑；再次保存可重新授权 | 包 #5 真实保存错误保留 dirty/原路径/编辑内容；原因已修复，包 #6 新目标与重复保存通过；权限失效及其他 provider 待测 |
| F12 | 两个不同文档分别保存，再轮流 Ctrl+S | 每个文档写入自己的目标；不使用全局最后一次目标 | 包 #8 真机通过：Stack 新增笔迹、Contact Sheet 隐藏单个标题，交替保存、关闭并从系统选择器分别重开，两份修改及尺寸/图层独立保留 |
| F13 | 关闭未保存文档，选择 Save | 等外部写入成功才关闭；失败/取消后保留原文档与提示 | 文档 Close Cancel/Save、OS 标题栏 Close Cancel/保存失败保留、#6b Save 成功后安全退出/重启通过；包 #9 文档关闭选择 Save 后取消系统保存，笔画/dirty/原关闭提示保留通过；OS关窗同路径取消待测 |
| F14 | 保存过程中切文档或继续编辑 | 按原文档 ID 和编码版本完成；后续编辑仍标为未保存 | 待测 |
| F15 | Place Embedded / Place Linked | 文件选择器保留原命令意图，导入为对象而非误开新标签 | 包 #8 两种原菜单新增对象、PSD外部重开四层通过；包 #12 来源更新与Undo、替换、重链及其取消、嵌入/关联转换及其取消、关联导出、pcraft重开后来源更新均已真机通过；失败/重授权和多来源部分失败仍待验收；PSD按原警告嵌入内容，不作为链接持久化证据 |
| F16 | 重启、清理 cache、Open Recent、Revert | 保留项目副本可用；失效 URI 可重新授权；不能依赖临时 cache | 包#11首页Recent去重/友好显示、外部源更改后的鲜读及真实进程重启后重开通过；包#12 Revert实际新尺寸/内容、取消和Undo通过，Redo内容恢复但dirty保留待比较原语义；撤销授权及cache清理待测 |
| F17 | Image Processor / Batch / Lens 的目录输出 | 参数捕获正确；外部结果可独立打开；失败可恢复 | 包#10六次正常任务十二份结果逐份外部重开通过；包#11旧保留输出在取消、重启后重新授权发布并重开通过；一次异常退出原因尚未确认，新格式恢复和复杂内容仍待测 |
| F18 | 三文档关闭中间或首个标签 | 剩余文档的缩放/画布/图层归属不变 | 包#10关闭非末尾后视图/四层/像素一致；包#11三个长标签缩短切换、八文档»隐藏菜单切换及关闭后七标签重排通过；额外OS窗口待测 |

## 其余功能验收范围

| 范围 | 测试内容 | 状态 |
| --- | --- | --- |
| 画布和工具 | 新建、笔刷/橡皮、移动、形状、选区、裁剪、变换、缩放、平移 | 真机笔刷、矩形选区及选区内像素反相通过；其他待测 |
| 历史和图层 | 撤销/重做、图层增删/顺序/可见性、蒙版、混合、调整和效果 | 选区内反相后 Ctrl+Z 恢复原像素与选区真机通过；其余见主机回归及待测项 |
| 键盘和鼠标 | 原快捷键、修饰键、连续绘制、右键、滚轮、触控板、失焦恢复 | Ctrl+N/I 已测；其他待测 |
| 文字和剪贴板 | 英文及中文输入法、候选位置、文字编辑、系统文字和图片复制粘贴 | 可见 PasteButton 中文粘贴、稳定 IME attach、系统中文候选与单次选词提交/保存/外部重开真机通过；预编辑取消、真实键盘、图片剪贴板待测 |
| 窗口和显示 | 调整窗口、最大化/还原、HiDPI、跨屏、最小化恢复、surface 重建 | 分屏/还原及 F 三种屏幕模式的真实 OS 全屏/还原通过；其他待测 |
| 稳定性和性能 | 大图/多图层、重复开关文件、持续编辑、内存、错误输入和恢复 | 上游核心、库和 UI 默认 2986 个测试通过；设备压力测试待测 |
| 上游兼容 | 引擎、编解码、导入导出、项目格式、真实语料和命令失败路径 | 默认 2986 通过，27 忽略；panic_hunt 显式 1 通过；可选语料及平台端到端仍待测 |
| 发行 | 原生库完整打包、许可证、签名、安装更新、启动、卸载保留策略 | 首版签名安装/启动已测；最终包需重验 |

## 已有证据

- [G1 编译记录](../ohos-notes/g1-failures.md)
- [G2 设备图形记录](../ohos-notes/g2-gpu.md)
- [G3 编辑器与恢复记录](../ohos-notes/g3-egui.md)
- [G4 输入记录](../ohos-notes/g4-input.md)
- 首版设备日志：`logs/device-smoke/2026-10-07T16-20-07-385Z/`。

修复后的测试数量、HAP 哈希、文件验收和截图将在实际完成后追加到本文。

## 2026-10-08 主机回归

固定上游提交 `4337a6227a823a28728e68aed844feab62b3314d` 的 `photocraft-engine`、`photocraft-io`、`photocraft-codecs`、`photocraft-format` 默认测试执行完成：**1404 通过、0 失败、14 忽略**，共 56 批（含 doctest），退出码 0。命令为 `CARGO_TARGET_DIR=../target/upstream-host cargo test --locked -p photocraft-engine -p photocraft-io -p photocraft-codecs -p photocraft-format`。

完整输出保留在 `logs/regression/upstream-core-2026-10-08.log`，统计在同目录 JSON。忽略测试和可选语料没有作为通过统计；这组结果验证原版引擎与格式逻辑，不覆盖鸿蒙选择器、拖拽、输入法或设备保存。

异步文件修复的移植主机测试已执行：**29 通过、0 失败**（平台输入 6、eframe 兼容接口 1、PhotoCraft 22）。包括选择器取消/失败/成功、按文档 ID 与 revision 提交保存、Save-before-Close、Close All 顺序处理、保存过程中继续编辑、Place Embedded/Linked 保持目标以及预设/脚本文件选择。源码格式检查通过，上游 checkout 无修改，vendor overlay 重放后逐字节比较通过。日志为 `logs/regression/ohos-async-files-2026-10-08.log`。

补充执行原版全命令恶意参数检查 `cargo test --locked -p photocraft-engine --test panic_hunt -- --ignored`：1 通过、0 失败，8.91 秒。该用例在默认回归中被忽略，本次显式执行。修复包的 `cargo clippy --locked -p craft-ohos-platform -p photocraft-ohos -p eframe --all-targets -- -D warnings` 和格式检查通过，arm64 release 构建成功。

随后补跑原版 `photocraft-doc`、`geom`、`color`、`cms`、`raster`、`ops`、`psd`、`raw`、`compose`、`paint`、`algo`、`text`、`vector`、`automation`、`plugins` 的默认库测试：**1037 通过、0 失败、8 忽略**，47 批，退出码 0。与前一组不同 package 合计默认测试 2441 通过；显式 panic_hunt 另计 1。未将默认忽略项记为通过。输出及统计在 `logs/regression/upstream-libraries-2026-10-08.{log,json}`。

下一批平台接缝主机测试 **43 通过、0 失败**（平台输入 7、eframe facade 1、PhotoCraft 35），strict Clippy、fmt、688 行 vendor overlay 重放和 OHOS arm64 release 构建通过。新增覆盖真实文档/egui 中文组合与 UTF-16 删除、文字和 RGBA 图片剪贴板、文档与焦点身份、关闭等待保存、实际文件发布失败的 stage 保留、PNG 再次保存复用、真实 CJK 字形与像素，以及 RawInput → egui 双指缩放。协议见 `ohos-notes/platform-protocol.md`；这些平台项尚待下一签名包真机验证，不能将主机通过写成系统输入法通过。

外部发布状态机新增 **22 项真实临时文件故障注入测试通过**：备份失败不改目标，截断/复制/fsync 失败恢复旧内容，恢复失败保留备份与新 stage，清理失败、只读恢复扫描等；ArkTS SDK 编译通过。日志为 `logs/regression/ohos-publication-tests-2026-10-08.log` 与 `ohos-arkts-publication-build-2026-10-08.log`。这组验证可注入状态机与真实主机文件，真实 provider 故障和跨 provider 原子性不在通过结论内。

原版 UI 库补跑 `CARGO_TARGET_DIR=../target/upstream-host CARGO_INCREMENTAL=0 cargo test --locked -p photocraft-ui-egui --lib`：**545 通过、0 失败、3 忽略**，实际测试运行 8.28 秒，退出码 0。覆盖真实 canvas 笔刷/移动/选区、形状、变换提交与取消、图层蒙版、调整对话框预览与取消、文字光标和选区、菜单导航及快捷键分发。3 个忽略项均为性能测量，本次未执行。日志与统计在 `logs/regression/upstream-ui-2026-10-08.{log,json}`。与不同核心库测试合计默认 **2986 通过、27 忽略**；它们仍不代替鸿蒙系统服务和设备交互。

剪贴板授权、保存目标持久化和 Recent 名称修复批：**51 项平台主机测试通过**（PhotoCraft 43、输入 7、facade 1），strict Clippy、fmt、690 行 overlay 重放通过。另有 PasteButton 授权状态机 6 项取消、迟到、重复点击及读失败测试通过。正式安装仍待本批新包，不沿用上一包的设备结论。日志分别为 `ohos-paste-published-tests-2026-10-08.log`、`ohos-paste-published-clippy-2026-10-08.log` 和 `ohos-paste-authorization-tests-2026-10-08.log`。

## 2026-10-08 正式文件菜单真机测试

文件菜单修复包：`moe.kiwi.photocraft`，47,525,235 字节，SHA256 `001c0ed95dc4bed2f95855e6082963d8705a985f31aaa92724663a4596c5f0c9`。API 26 MateBook Pro 安装/启动及原 UI 首帧通过，临时 PNG 按钮和重复状态栏均已移除。安装日志：`logs/device-smoke/2026-10-07T18-05-48-586Z/`。

操作及结果：

1. Ctrl+O 进入系统选择器，从“常用图像”选取自建 `PhotoCraft-Smoke.png`；1920×1080 原笔迹、文档名称和尺寸导入正常。证据 `filefix/opened-png-confirmed`。
2. Ctrl+I 编辑后 Ctrl+S，另存自建 `PhotoCraft-FileFix-20261008.png`，实际完成写入并清除 dirty。证据 `filefix/save-name`。
3. Save a Copy 取消后可以再次进入保存；生成自建 PSD 副本成功。之后在原文档中新建 Layer 1 并绘制第二道笔迹，Save a Copy 生成 `PhotoCraft-FileFix-Layers-20261008.psd`，原文档仍有 dirty 星标。证据 `filefix/layered-copy-saved`。
4. 正式 Open As 选择上述 PSD，出现第二标签，Layer 1 与 Background 两层及各自笔迹保留。证据 `filefix/reopened-layered-psd`。
5. 在重开的 PSD 编辑后 Ctrl+S，没有再次出现选择器，成功写回原目标并清除该文档 dirty；另一文档仍 dirty。证据 `filefix/psd-inplace-save`。
6. Ctrl+Shift+S 进入正式 Save As，另存 `PhotoCraft-FileFix-SaveAs-20261008.psd`。证据 `filefix/save-as-psd-saved`。

截图和布局位于被忽略的 `logs/device-validation/filefix/`，只使用自建测试文件，未将桌面或个人文件截图作为公开资源。

本轮还发现首次 Open 默认仅显示项目格式，以及 Save As 不能切换已编码格式的问题；已修复为默认显示支持文件、先捕获文档版本再按所选格式重新编码。该修复的主机测试 **32 通过、0 失败**，strict Clippy、格式检查、overlay 重放及 arm64 release 均通过；新签名包设备验收仍在继续。

## 2026-10-08 格式与拖拽修复包真机测试

签名 HAP：47,587,929 字节，SHA256 `255b16ec448489d2215091b6606ee6f5f1a06cc275db0e5d824179bcf0a7921e`。安装、启动和首帧通过；日志为 `logs/device-smoke/2026-10-07T18-40-38-949Z/`。

1. 更新后恢复上一包仍未保存的两图层文档，保留两道笔迹、图层和 dirty 标记。证据 `filefix/maximized-formats`。
2. 系统文件管理器与 PhotoCraft 左右分屏，从文件管理器向画布执行真实鼠标拖动（设备 `uinput -M -g`），成功打开自建 `PhotoCraft-Smoke.png`：新增标签，1920×1080、单图层和原笔迹一致，恢复文档仍保留。证据 `filefix/mouse-dropped-png`。
3. 分屏切换回普通窗口后，编辑器、文件菜单和系统保存选择器继续响应。多文件拖入、不同保存格式和失败恢复继续测试，未把单文件成功推算为全部通过。
4. 从恢复的 PNG 工作文档（两图层）执行 Save As，系统下拉选择 PhotoCraft 格式，写入自建 `PhotoCraft-Formats-20261008.pcraft` 到桌面。Ctrl+O 默认“所有支持的图像与工程”能同时显示工程与 PNG，无需手动切换过滤器。重开该 pcraft 后两图层及各自笔迹正确，新增标签，保存/打开状态可见。证据 `filefix/saveas-pcraft-ready`、`filefix/saved-pcraft`、`filefix/reopened-pcraft`。
5. 同一 pcraft 文档 Save As 改选 JPEG，保存自建 `PhotoCraft-Formats-20261008.jpg` 成功，原 UI 显示图层被合并及有损压缩两条警告。Ctrl+O 重开后名称、1920×1080、两道笔迹正确，文件为单 Background 图层，符合 JPEG 扁平图像格式。证据 `filefix/saved-jpeg`、`filefix/reopened-jpeg`。
6. 从已打开 JPEG 的 File → Export → Quick Export as PNG 进入系统保存，写入自建 `PhotoCraft-QuickExport-20261008.png`，原状态栏显示 Exported；文档仍保持 JPEG 名称与 clean 状态。证据 `filefix/quick-export-picker`、`filefix/quick-export-saved`。该项导出文件重开仍待测。
7. File → Export → Export As 打开原参数与预览对话框，设置 PNG、50% 后导出自建 `PhotoCraft-ExportAs50-20261008.png`。重开实际文件为 **960×540**（源 1920×1080），两道笔迹正确，原 JPEG 文档保留。证据 `filefix/export-as-dialog`、`filefix/export-as-saved`、`filefix/reopened-export-as50`。
8. Ctrl+N → Create 新建 Untitled-1（1920×1080），Ctrl+S 进入真实 PSD 保存选择器。取消后文档保留，继续绘制正常；再次 Ctrl+S 写入自建 `PhotoCraft-NewDocument-20261008.psd` 成功、dirty 清除。证据 `filefix/new-doc-created`、`filefix/new-doc-save-cancelled`、`filefix/new-doc-resave-picker`、`filefix/new-doc-saved`。
9. 在已保存的新建 PSD 上再绘制一道笔迹，Ctrl+W 显示原 Unsaved changes；Cancel 后文档与编辑保留。再次 Ctrl+W → Save，成功写回该 PSD 后关闭文档并回到另一标签。证据 `filefix/close-dirty-psd-prompt`、`filefix/close-cancel-kept-document`、`filefix/close-saved-document`。
10. 应用停止后重新启动，从 Home 的 Recent 打开持久 PSD，1920×1080 与两道笔迹保留。M 工具拖出矩形选区，Ctrl+I 只反相选区内像素，Ctrl+Z 恢复原像素与选区。证据 `filefix/open-recent-psd-after-restart`、`filefix/actual-rectangle-selection`、`filefix/actual-selection-inverted`、`filefix/actual-selection-inversion-undone`。实际新文件名与 stage 的 Recent 名称不一致，已列入下一批修复；不将该命名问题记成通过。
11. Ctrl+Shift+Z 重做选区内反相，再撤销、Ctrl+D 取消选区；Ctrl+1 切到 100%，Ctrl+0 恢复适合屏幕 43.9%，文档尺寸不变。证据 `filefix/selection-redo`、`filefix/actual-pixels-zoom`、`filefix/fit-screen-zoom`。
12. U 矩形形状工具拖出黑色矩形，创建 Rectangle 1 矢量图层；Ctrl+Z 删除形状和对应图层，Ctrl+Shift+Z 恢复形状及图层。证据 `filefix/shape-created`、`filefix/shape-undo`、`filefix/shape-redo`。
13. 将上述形状文档另存为独立 `PhotoCraft-Shape-Fixture-20261008.psd`；Ctrl+O 进入系统选择器后取消，矩形、两道笔迹、图层和 clean 状态均保留，原状态栏显示 File operation cancelled。证据 `filefix/shape-fixture-saved`、`filefix/open-cancel-picker`、`filefix/open-cancel-kept-document`。

UITest 的触摸长按 drag 在系统文件管理器中触发的是上下文菜单，不构成真实鼠标文件拖入测试；本次通过证据来自系统鼠标注入。

## 2026-10-08 平台批签名与安装阻塞

43 项主机平台测试对应的统一签名 HAP：47,835,997 字节，SHA256 `7a711e80befd97c19d19e13b66ffa1ec7a6a1202b6a4de9b6e1c17a29c9fff23`；包内 SO 45,951,944 字节，SHA256 `1cd41a9705340b4688d66243f069cbe3c41d7ebc180dbda67ffee4d82995cb56`。严格 SDK 编译、ABI、依赖及公开签名配置恢复检查通过，构建证据在 `logs/build/platform-20261008/`。

**该包未安装成功。** 真机 BMS 返回 `9568289: grant request permissions failed`，指向 `ohos.permission.READ_PASTEBOARD`；当前 Profile 不包含该受限 ACL。失败报告在 `logs/device-smoke/2026-10-07T19-29-32-963Z/install.txt`。不能将编译与签名通过记成 IME、剪贴板、安全关闭的设备通过。

正在按官方安全控件路线修复：移除受限声明和普通运行时授权假设，只在原菜单或 Ctrl+V 触发粘贴时展示可见 PasteButton 单次授权及取消，再执行原文字/图像 completion。[官方权限说明](https://github.com/openharmony/docs/blob/master/zh-cn/application-dev/basic-services/pasteboard/get-pastedata-permission-guidelines.md)明确说明该权限需要 ACL，安全控件是无需常驻读取权限的路径。下一包将重新验证安装及编辑目标焦点保持。

## 2026-10-08 可见粘贴授权包真机复测

包 #5：47,911,115 字节，HAP SHA256 `6fc890fb0bf6a1712f68cca6ae9b27554c163058738027e2627e646f2ac1bb0e`，SO SHA256 `fb2d4e9ae0e78db9bf5f25f239dcc485873e87b4918d289299ac1eb37359d681`。签名安装、启动与原编辑器首帧通过；仅声明 `FILE_ACCESS_PERSIST`，移除了受限 `READ_PASTEBOARD`。构建快照 `logs/build/next-native-20261008/`，设备日志 `logs/device-smoke/2026-10-07T20-04-05-614Z/`。

1. 原 Open 重开自建 `PhotoCraft-Shape-Fixture-20261008.psd`，标题使用实际文件名，Rectangle 1 和 Background 图层、矩形及原两道笔迹保留。证据 `filefix/platform-reopened-shape`。
2. 原 T 工具创建文字层，显式粘贴请求出现可见系统 PasteButton。Cancel 后原选择及占位文字保留；再次 Ctrl+V 并点击 PasteButton 后，`PhotoCraft鸿蒙测试` 写入同一个文字层，中文正常渲染，确认按钮提交后文字图层名称更新。证据 `filefix/platform-paste-cancelled`、`platform-paste-reopened`、`platform-paste-approved`、`platform-type-committed`。UITest `uiInput text` 实际借用剪贴板，**这条记录不证明系统中文输入法提交成功**。
3. 保存复测发现回归：Save As 新 PSD 返回 `Invalid argument`；文档仍 dirty、原路径不变。随后系统标题栏 Close → Save 返回 `File exists`，原 unsaved 提示、文档及应用均保留；Cancel 可以继续编辑。证据 `filefix/platform-type-saved`（文件名如此，实际为失败截图）、`platform-window-dirty-close`、`platform-window-close-cancel`、`platform-window-close-save-failed`。本包新目标保存没有通过，不能沿用旧包的保存通过结果。
4. 找到并修复确定的 SDK 合同问题：`mkdtemp` 模板缺少尾部 `XXXXXX`，以及每次保存都对既存目录调用 `mkdir`。新代码逐层创建或核验沙箱真实目录，并添加只含固定阶段及错误码的日志；保持备份、同步、seek 预检和失败回滚。55 项 ArkTS 主机测试通过，其中 8 项 SDK 合同回归覆盖既存目录重复保存、新空目标、链接/文件祖先拒绝和失败恢复。详情见 `ohos-notes/publication-device-followup.md`；新包真实 provider 流程仍待复验。
5. Native IME attach 返回 `12802000`。检查公开实现后补齐必需 `ReceivePrivateCommand` 回调及文本配置，并恢复系统 IME 未消费的硬件按键传递。严格 SDK C++ 检查通过；记录在 `ohos-notes/native-keyboard-ime-routing.md`。本包没有 IME attach/拼音预编辑通过证据，修复后的系统行为待新包验证。
6. 移动工具状态下连续按 F，第一模式进入实际 OS 全屏（窗口 `[0,17][3120,2070]`），第二模式隐藏原编辑器栏只显示画布，第三模式还原普通窗口 `[344,255][2776,1775]`。中文字、形状、笔迹及图层均保留。证据 `filefix/platform-screen-mode-first`、`platform-screen-mode-second`、`platform-screen-mode-restored`。SDK 失败时原菜单内部模式状态回退及跨屏仍待测。

下一打印批 Rust 平台主机测试 **56 通过、0 失败**（PhotoCraft 48、输入 7、facade 1），strict Clippy、fmt、735 行 overlay 重放通过。复用原 PDF 渲染与真实字节比较，接原 Print/Print One Copy/Actions 与系统七态任务状态、Print to PDF 发布；受理不等于打印完成。嵌套脚本打印、系统预览/取消和实际打印结果仍需另验。本批尚未安装，不能计入包 #5 真机结果。

## 2026-10-08 保存修复及系统打印预览

包 #6：47,977,815 字节，HAP SHA256 `3715fb09c2d7a602908f0738a356cc9662a14f541d8659a74b3631437732fe8b`，SO SHA256 `9c7acf5ba8edf14dce77b17afe5adb63dcaa4c30859304414b48a4ab181ac350`。权限为 `FILE_ACCESS_PERSIST` 和正常 `PRINT`，安装/启动/首帧通过；构建快照 `logs/build/package6-20261008/`，设备日志 `logs/device-smoke/2026-10-07T20-34-40-289Z/`。

1. 更新恢复上次未保存的中文文字层。Save As 自建 `PhotoCraft-Type-CJK-Fixed-20261008.psd` 成功，实际保存反馈正确，dirty 清除。系统 Open 重开该外部文件后，中文文字、Rectangle 1、Background 和原笔迹均正确，三层结构保留。证据 `filefix/package6-cjk-saveas-result`、`package6-reopened-cjk-psd`。Save As 后原标签名称未同步，下一批已修复，未将此名称问题记为通过。
2. 输入普通 `nihao` 测试文字并确认，直接 Ctrl+S 无选择器写回上述 PSD，dirty 清除；既存恢复目录、目标备份/seek/复制/fsync 流程没有再次报错。证据 `filefix/package6-hardware-latin-visible`、`package6-cjk-repeat-save`。该英文结果来自模拟按键，不证明真实键盘或中文候选。
3. Ctrl+P 打开原 Print Settings，勾选 Scale to Fit 后点击 Print，真实系统 spooler 出现一页预览；中文、笔迹、矩形和可见英文文字均正确，默认目标为系统 Print to PDF。证据 `filefix/package6-print-dialog`、`package6-system-print-preview-spooler`。本次未发送到实体打印机。
4. 系统预览右上 X 返回编辑器，文档内容与 clean 状态保留，但 SDK 没有回传任务 `cancel`，状态仍等待系统结果。**仅窗口返回通过，任务取消/完成不计通过。** 官方 SDK/框架审查证实 PrintTask.cancel 仅对应打印任务终态取消，预览关闭走另一条路径；public adapter 枚举也不提供完整的预览开始/取消区分。保持未确认 PDF，下一批改为明确“请求已受理，最终结果未确认”。证据 `filefix/package6-system-print-cancelled` 及 `ohos-notes/printing-sdk26.md`。

## 2026-10-08 稳定输入法连接及安全关闭

包 #6b：47,968,135 字节，HAP SHA256 `56de2d36b6bac47f36180dc4c219cae9b806a4b42bff3588a34fbdeb6110f04a`，SO SHA256 `6240ab91beb2f431ec533f39567f730043beec2d80d8523083cb0e789b608b9e`。仍链接同一 56-test Rust archive，ArkTS/资源/manifest 与 #6 相同，只更新 Native 上下文身份。安装/启动/首帧通过：`logs/device-smoke/2026-10-07T20-53-40-457Z/`。

#6 的 Native 实际成功 attach，但每 200ms 将新建的 opaque ArkUI context wrapper 指针当成窗口变化，反复 detach/attach。#6b 改为公开稳定实例标识与实际 windowId，API20 使用受管理 NAPI reference 退路，调用正常 Attach 并移除无法公开释放的 wrapper。19 项上下文合同检查及严格 SDK C++ 检查通过。真机限定 PID/标签实时日志显示本次同一编辑目标仅一次 attach（window 1417、instance 100000），不再每帧重连。证据 `filefix/package6b-ime-live-hilog.txt`。两个模拟输入途径均落字 `nihao`；中文模式、候选、preedit/commit、真实键盘仍待有事件证据的单独验证，不将英文落字推算为 IME 全通过。

系统标题栏 Close 显示原 unsaved 提示；选择 Save 后真实外部保存完成，窗口消失，旧 PID 54498 不再存在。重新启动为新 PID 61543，正常出现 Home 与 Recent，未误触立即关闭；证据 `filefix/package6b-window-close-prompt`、`package6b-window-save-closed`、`package6b-after-safe-close-relaunch`。本设备本次没有保持同 PID，因此缓存同 PID 再建窗口仍不能计通过。`package6b-input-method-switcher` 是关闭提示尚在时的无效切换尝试，不作为输入法证据。

下一源码批平台主机测试 **60 通过、0 失败**（PhotoCraft 52、输入 7、facade 1）；engine overlay 原测试 **635 通过、13 忽略**，显式 panic_hunt 1 通过。它们包含原引擎回归重跑，**不能再加到原版默认 2986 的独立总数上**。新增 spool hook 保留原 Print Script Events、防递归和 Batch scratch session 归属，Save As 成功同步名称；four-package strict Clippy、fmt、20 个 PhotoCraft path 包分层检查、实际 engine wasm 检查及 178 行 engine/736 行 UI overlay 精确重放均通过，上游 checkout clean。60-test OHOS release 构建及 archive 复制完成，新的签名包/设备结论仍另记。

目录输入与多文件输出正在接线。FolderPublication 独立 helper 的 25 项真实临时树/SDK 合同主机测试和隔离 ArkTS 编译通过，采用全新随机输出子目录并在失败或取消时保留 stage/journal；尚未接原菜单或执行真实 provider fixture。不能把 helper 编译或复制布局源代码检查记为原 Batch、Save for Web slices 或 Generate Image Assets 通过。

## 2026-10-08 引擎打印接缝、另存名称及中文候选

包 #7：47,999,063 字节，HAP SHA256 `80d0b58e1e2024f64627cc7b1540acf454ce7242537cdb8f942a4863356f44bc`，SO SHA256 `2a56cd640684c3eee2f8cc653d6a988c09c003413f4d10af14ae71de60f7bd67`。链接上述 60-test archive，加入原引擎 spool hook、Save As 名称同步和有限、无输入内容的 IME 诊断。安装/启动/首帧通过；构建快照 `logs/build/engine-spool-ime-20261008/`，设备报告 `logs/device-smoke/2026-10-07T21-19-18-978Z/`。

1. 从原 Open 的系统选择器重新选择外部 `PhotoCraft-Type-CJK-Fixed-20261008.psd`，实际七层文档与上次 OS Close → Save 后的四个英文文字层、中文文字、Rectangle 1 和 Background 一致。证据 `filefix/package7-external-seven-layer-reopen`，补足安全退出写入的外部文件重开验证。
2. 原 Save As 另存独立 `PhotoCraft-SaveAs-Title-20261008.psd`；真实写入完成后，标题栏、标签和状态反馈均同步新名称，dirty 清除。证据 `filefix/package7-save-as-name-ready`、`package7-save-as-title-updated`。
3. 原 Type 工具内模拟按键输入 `nihao`，限定本应用 PID/标签日志记录有效 TextConfig（inputType 1、window 1419、previewSupport 1）、一次 attach 与稳定连接，以及系统 IME 的 insert 回调 `valid=1 queue=1`。因此这次英文输入确实经过系统 IME 提交，不是仅靠 Native ASCII fallback。日志为 `filefix/package7-ime-live-hilog.txt`。同一回调最多记录四次，不能按缺少后续日志推断没有回调。
4. 截取系统输入法自身浮条，确认当时显示“英”；Shift/Meta+Space 的模拟切换不足以证明中文模式。显式点击英/中文状态后浮条显示“双”（本设备现有双拼模式），原文字编辑器 Ctrl+A，再模拟输入 `n`、`i`，系统候选框显示 `ni` 和“你、呢、尼…”；候选框位于测试文字插入位置附近。Space 选择第一候选，替换为单个“你”，确认文字层后 Ctrl+S 写回自建 PSD 成功。证据 `filefix/package7-system-ime-switcher-ime`、`package7-ime-language-toggle-ime`、`package7-ime-ni-candidate-ime-1`、`package7-ime-cjk-committed`、`package7-ime-cjk-saved`。
5. 关闭 clean 文档，再用系统 Open 选择外部 `PhotoCraft-SaveAs-Title-20261008.psd`，单个“你”及其他图层/图形保留，标题和 clean 状态正确。证据 `filefix/package7-cjk-external-reopened`。本项证明系统中文候选、选词提交、持久保存与外部重开；不替代真实硬件键盘、画布内 preedit 更新、候选取消、组合期间选区删除、跨屏候选位置或所有输入法验收。

6. 原 Print Settings 勾选 Scale to Fit，填入独立 `PhotoCraft-Engine-Print-20261008.pdf`，Print 进入系统保存选择器，外部发布完成后显示 Exported，原 PSD 名称与 clean 状态保留。再次 Ctrl+P 保留 Scale to Fit，Save as PDF 字段为空，未残留 stage 路径或上次 PDF 目标。证据 `filefix/package7-print-pdf-name-ready`、`package7-engine-pdf-save-picker`、`package7-engine-pdf-published`、`package7-print-after-pdf`。PDF 内容的独立外部查看仍待补证。
7. 上述空输出字段的普通 Print 通过引擎 spool hook 进入真实系统打印预览，一页包含中文标题、单字“你”、图形及英文测试文字。系统目标明确为 Print to PDF，点击开始打印后进入系统选择器，另存 `PhotoCraft-System-Print-20261008.pdf`；系统提示任务可在快捷栏查看，返回应用后原状态栏显示 **System print job completed**，项目内容/名称/clean 保留。证据 `filefix/package7-original-print-spooler-spooler-0`、`package7-system-pdf-name-ready`、`package7-system-pdf-finished-spooler-0`、`package7-print-job-final`。本次 PDF 任务终态通过，不证明实体打印机输出，也不改变包 #6 预览 X 没有取消回调的结论。

截图仍只保存在被忽略的设备证据目录；系统输入法证据只裁切本次自建输入的浮条或候选窗口，不发布整屏桌面。包 #7 的嵌套脚本/Batch/Print One Copy 打印、独立 PDF 重开和实体打印仍待设备复测，不能因引擎回归通过而记为真实打印完成。

## 2026-10-08 文件夹输入与独立保存目标

包 #8：48,134,555 字节，HAP SHA256 `1e463acedabf1c99ceccf8da92b746f6083dc5dca1bbf7166b263c971947cdd9`，SO SHA256 `36bca9164ad4422f52ef62dbe8d60059586fd10f749a838b40f4ac80f1354709`。安装/启动/首帧通过；完整包快照 `logs/build/folder-input-20261008/`，设备报告 `logs/device-smoke/2026-10-07T21-45-14-607Z/`。

本批平台主机测试 **67 通过、0 失败**（PhotoCraft 59、输入 7、facade 1）；新增真实 Load Stack、Contact Sheet、Statistics 与过期/取消/恶意清单检查。原 engine 针对 Stack/Image Processor/Batch 的回归、strict Clippy、fmt、两个 overlay 精确重放及实际 UI/engine wasm 检查通过。ArkTS **105 项通过**，其中目录输入合同的 **44 项是子集**，不另加总；SDK 编译通过。日志及源码哈希见 `logs/regression/folder-input-20261008/`，协议见 [目录输入协议](../ohos-notes/folder-input-protocol.md) 与 [Rust 目录输入验证](../ohos-notes/folder-input-rust-validation.md)。

仅使用自建桌面目录 `PhotoCraft-Folder-Fixture-20261008`：本层两个 PNG（960×540 与 1920×1080）、一个不支持的 PDF，以及子目录中的另一 PNG。没有把整个桌面作为导入目录。

1. 原 File → Scripts → Load Files into Stack，Browse 调用系统目录选择器并选取该目录。返回原表单显示 2 image files，尚未创建文档；再次 Browse 后取消，上一份选择及表单参数保留。确认后原引擎生成 1920×1080、两个具名像素图层，PDF 与嵌套 PNG 均未误入。证据 `filefix/package8-stack-folder-imported`、`package8-folder-cancel-kept-selection`、`package8-stack-created`。本设备本地 provider 的目录复制布局契约实际通过，其他 provider 尚待测。完成后残留上一次“Folder selection cancelled”状态文本已列为后续界面修正，不影响本次像素/图层结果。
2. 原 File → Automate → Contact Sheet II 设置 2 列、1 行，保留 8×10 英寸、300ppi、文件名标题、非扁平等原参数；同目录返回 2 张图像。确认生成 2400×3000、300ppi、两个图像层、两个标题文字层和 Background 共 5 层，Stack 标签保留。证据 `filefix/package8-contact-ready`、`package8-contact-sheet-created`。
3. 分别首次保存到自建 `PhotoCraft-ContactSheet-20261008.psd` 和 `PhotoCraft-LoadStack-20261008.psd`。Stack 上添加一笔并 Ctrl+S，Contact Sheet 上隐藏右侧标题文字层并 Ctrl+S，两次均直接写回各自目标、清除相应 dirty。关闭两份文档，再从系统 Open 分别选择外部文件：Stack 的新增笔迹、两层和 1920×1080 保留；Contact Sheet 的右标题隐藏、左标题显示、五层与 2400×3000 保留。**F12 双文档目标隔离真机通过**。证据 `filefix/package8-stack-saved`、`package8-contact-saved`、`package8-two-doc-stack-repeat-save`、`package8-two-doc-contact-repeat-save`、`package8-stack-independent-external-reopen`、`package8-contact-independent-external-reopen`。
4. 原 File → Scripts → Statistics，保留 Median、Align 关闭，Browse 同目录返回 2 image files，确认前未创建结果。确认生成 1920×1080 单 Smart Object Layer，两个不同尺寸输入的原 Median 合成画面可见。保存独立 `PhotoCraft-Statistics-20261008.psd`，关闭并从系统 Open 重开，智能对象、合成预览、尺寸与 clean 状态保留。证据 `filefix/package8-statistics-folder-imported`、`package8-statistics-created`、`package8-statistics-saved`、`package8-statistics-external-reopen-confirmed`。Mean 等其他模式与自动对齐尚未设备测试。
5. 切回 Stack，原 Place Embedded 系统选择器选择自建 1920×1080 PNG，新增原 Smart Object Layer，原标签和其他文档保留，dirty 正确。原 Place Linked 选择另一张 960×540 PNG，在同文档新增居中的 Smart Object，尺寸 960×540、位置 (480,270)，合计四层。另存独立 `PhotoCraft-Place-Fixture-20261008.psd` 时明确显示原格式警告：PSD 不保留外部链接，Linked 对象内容被嵌入。关闭并从系统选择器重开，四层、两种尺寸的对象与合成图像、clean 保留。证据 `filefix/package8-place-embedded-result`、`package8-place-linked-result`、`package8-place-fixture-saved`、`package8-place-external-reopen`。本项不证明后续外部源变化能刷新，也不把原 PSD 的明确嵌入行为记作链接持久化通过。

连续关闭/新开后的非活动标签缩放/图层摘要出现视图缓存错位。只读对比已确认原版 `sync_views` 仅按数量 resize/truncate，关闭非末尾文档后 `docs.remove(index)` 左移，后继文档继承旧 index 的 View，额外视图窗口也只按数量 retain。保存 completion 已按 DocId 查找，目前没有 F12 实际文件串写证据。下一批将按 DocId 重映射视图/窗口并验证关闭首个和中间文档，设备修复尚未通过。

本批目录输入不能代替 Image Processor、Batch、slices 等多文件外部输出验收；目录发布接线与实际输出仍在继续。

## 2026-10-08 Image Processor 输出批

包 #9：48,376,698 字节，HAP SHA256 `7bef6ae6cf8d03f7ae86f7d7db9be72a1d8406b2d18e6bb04d4cfe658c7268ea`，SO SHA256 `65994b6f50f01e365e32e0110c313917066aee7be9f32b14dc823fe7c748c592`。arm64 release 7m22s、完整签名构建30.881s、安装/启动/首帧通过；快照 `logs/build/folder-processor-20261008/`，设备报告 `logs/device-smoke/2026-10-07T22-45-53-983Z/`。公开签名配置恢复后的哈希与前批逐字节一致。

平台主机批 **75 项通过**（PhotoCraft67、input7、facade1），新增8项原 Image Processor 真实格式/像素/分层/错误/取消/归属测试。原 file_cmds 针对回归16项通过，是原引擎回归子集，不累加到默认2986。strict Clippy、fmt、engine216/UI1001行 overlay 精确重放、锁定 UI/engine wasm probe22.95s通过。Rust日志与289个源码哈希见 `logs/regression/folder-processor-rust-20261008/`。ArkTS新增工作流24与受影响兼容44共68项通过；隔离SDK编译通过，日志和11个文件哈希见 `logs/regression/folder-processor-20261008/`。

原表单保留格式、quality、width/height、sRGB和原确认行为；预先选择真实输出目录，确认后为捕获参数分配独占stage，调用原引擎完成编码，再向所选目录发布全新随机子目录。只有实际复制成功回执才报告发布完成；失败/取消保留完整结果与journal。协议见 [原 Image Processor](../ohos-notes/folder-processor-protocol.md) 与 [Rust 验证](../ohos-notes/folder-processor-rust-validation.md)。本批真实 provider 输出正在验收；安装/首帧本身不算多文件输出通过。


### Image Processor 真机发现与修复中

包 #9 在首页无文档时，以原 Image Processor 选择自建 `PhotoCraft-Folder-Fixture-20261008` 作为输入/输出目录；原表单正确显示 2 image files、Png、480×270、quality 8、sRGB，重新选择输出后取消仍保留这份参数及已选 handle。证据 `filefix/package9-processor-input-imported`、`package9-processor-output-selected`、`package9-processor-output-cancel-retained`。

确认后未发布成功：仅本应用进程/标记日志记录 `folder-publication stage=folder-permission-persist code=13900001`。原表单已经持久授权，发布桥再次对同一选择调用 `persistPermission`，在当前设备上遭拒；不能将此次操作记为输出通过。原 Pro 首页状态栏无文档分支提前返回，也隐藏了该失败。输入/输出原文件未作为覆盖目标；编码产物按协议保留，实际外部写入尚待修复包复验。

修正发布桥：表单所选 handle 只重新检查并激活既有持久授权；只有发布桥自身拉起的新 picker 才建立授权。新增两项生产 helper 回归：已有 grant 在第二次 persist 被拒条件下仍发布完整树；grant 撤销时不写 provider 并保留 stage/journal。目录 helper **27 通过**（旧25+新增2），属于平台合同验证，不代替真机结果。首页错误/进度/成功状态的原 footer 修复同时进入后续 #10 批。

SDK26 本地 `@ohos.fileshare.d.ts` 和[华为授权持久化指南](https://developer.huawei.com/consumer/cn/doc/harmonyos-guides-v5/file-persistpermission-V5)区分持久化与后续按需激活；错误原因判断结合本设备实际阶段日志，不能据此断言所有 provider 的重复 persist 都失败。

包 #9 的源码独立快照已按构建时两个 manifest 校验 **300/300 文件**，包括从固定 upstream+已归档 overlay 重放得到的 vendor 文件；记录 `logs/build/folder-processor-20261008/source-snapshot-verification.json`。当前后续源码变更与已安装 #9 的验收证据分开归档。


### 包 #9 单文件格式真机补充

原 Save As 的系统格式列表可操作。在系统格式选为 Photoshop PSB 后，保存 `PhotoCraft-Format-PSB-Actual-20261008.psb`，原标题/标签同步且 clean；关闭文档，通过系统 Open 独立重开，1920×1080、四层（两个原 Smart Object）与画面保留。证据 `filefix/package9-psb-actual-saved`、`package9-psb-actual-external-reopen`。

同一分层资料选择系统 TIFF 格式另存 `PhotoCraft-Format-TIFF-20261008.tif`。保存成功且原格式警告明确四层将合并，不保留层/蒙版/混合模式；关闭后独立 Open 重开，1920×1080、单锁定 Background 与合成画面正确。证据 `filefix/package9-tiff-saved`、`package9-tiff-external-reopen`。格式有损边界属于原 TIFF 编码器行为，原 PSB/PSD 测试文件仍保留。

系统 save picker 以当前格式补后缀；单改文件名扩展名而不选相应格式会成为 `.psb.psd`，本次初次操作未记作 PSB 通过。后续均先选实际格式再确认文件名；应用根据返回的最终后缀重新编码。


Targa 真实系统格式输出 `PhotoCraft-Format-TGA-20261008.tga` 成功、原标题 clean；原编码器提示 resolution(DPI) 不支持。关闭后系统 Open 重开，1920×1080、RGB/8、单 Background 与三笔合成画面正确。证据 `filefix/package9-tga-saved`、`package9-tga-external-reopen`。本例是从已合并 TIFF 继续输出，不能据此声称 Targa 保留分层。


OpenEXR 真实系统格式输出 `PhotoCraft-Format-EXR-20261008.exr` 成功、clean，原分辨率元数据警告可见。关闭后独立 Open 重开，1920×1080、RGB/32、单 Background 与相同三笔合成画面正确。证据 `filefix/package9-exr-saved`、`package9-exr-external-reopen`。本例验证普通测试图像的原 EXR 编码/解码及外部发布，不覆盖超范围 HDR、其他通道布局或大图性能。


### 包 #9 关闭中取消保存

新建1920×1080 RGB/8自建文档绘制一笔，Ctrl+W 显示原 Unsaved changes；选择 Save 确实拉起系统保存器。取消系统保存后，文档/笔画/dirty 标记及原关闭确认仍保留，footer 显示 File operation cancelled，没有误清 dirty 或关闭文档。证据 `filefix/package9-close-savecancel-unsaved-dialog`、`package9-close-savecancel-picker`、`package9-close-savecancel-retained`。此项为文档关闭，不冒充 OS 标题栏关闭的相同取消路径已测。


随后取消原关闭确认，正常 Ctrl+S 保存为 `PhotoCraft-Close-Save-Cancel-20261008.psd`，原标题/标签同步且 clean；关闭后系统 Open 独立重开，1920×1080、Background 中该斜向笔画保留，证明取消未破坏后续保存。证据 `filefix/package9-close-savecancel-followup-saved`、`package9-close-savecancel-external-reopen`。

### 包 #9：一次拖入两个文件（设备 F09）

在专用自建 `PhotoCraft-Folder-Fixture-20261008` 中先选 `PhotoCraft-ExportAs50-20261008.png`，用 Shift+向下选择相邻的 `PhotoCraft-QuickExport-20261008.png`。只查看这两行的局部截图确认双选；从文件管理器一次跨应用鼠标拖到 PhotoCraft 首页，原界面按顺序生成两个独立 tab，第二张为 1920×1080，切回第一张为 960×540，两个测试画面正确、无 dirty 标记。没有修改或丢弃外部原文件。

证据 `filefix/package9-drag-two-png-own-source-only.png`、`package9-multi-file-drag-result-crop-0.png`、`package9-multi-file-drag-first-document-crop-0.png` 及对应布局。此项只覆盖两个有效 PNG，不能代替格式错误、撤销授权、混合目录拖入或笔压输入测试。完整设备截图为私有测试证据，不发布桌面/文件管理器内容。

### 包 #10：签名构建与 PNG 目录发布真机复测

82/82相关Rust测试（Photo74、共用输入7、facade1）、17项受影响原engine测试、四包all-targets与直接UI --lib strict、fmt、两份原patch精确重放与wasm probe均通过；ArkTS74/74及API26隔离编译通过。原2986项全量回归是前批结果，不将本批受影响测试重复累加。

OHOS release 9m32s、共享签名Hvigor 8.604s成功。永久构建目录 `logs/build/folder-batch-lens-20261008` 保留签名HAP、SO、静态库、375份精确源码、manifest、ELF与日志；编译前后375份源均0 mismatch。HAP 48,383,025 bytes、SHA256 `4fd3813bfe331ceedcd353b22d02934e161f9026ddbca60e96b59526fc38b9fa`；SO 46,188,104 bytes、SHA256 `beabf2d6abfd0db56feafcc2a660d2de7aa390f5ee6976edf3bafcee6b47bdeb`。19个既有ABI/7个NEEDED不变；公用配置恢复到原无签名材料的hash。设备安装/启动/首帧通过，report `2026-10-07T23-42-30-593Z`，新PID43411。

在原 Image Processor 选择自建根目录为输入和输出，输入2个顶层PNG；参数Png、480×270、quality8、sRGB。Confirm后首页原footer明确显示 `Published 2 processed files; 0 inputs failed`，此前包#9的再次persist错误没有重现。原系统Open进入自建根目录，确认新增 `PhotoCraft-Export-9f8575c5-b7ed-4a3e-b722-d7306a46832d`；进入后确有两份PNG，并逐一从这个外部child独立重开。两者均480×270/RGB8/Background，测试笔画画面正确、clean。原根目录的两份PNG和嵌套fixture仍列出；此Open的图像过滤器未列出PDF marker，不能据此声称PDF被删或其hash已核对。

证据 `filefix/package10-processor-png-ready-form-only.png`、`package10-processor-png-publication-footer-only.png`、`package10-processor-png-owned-output-root-layout.json`、`package10-processor-png-output-files-layout.json`、`package10-processor-png-first-external-reopen-crop-0.png`、`package10-processor-png-second-external-reopen-crop-0.png`。本项仅标PNG正常两文件目录发布通过；JPEG/PSD/TIFF、失败恢复/Retry、Batch/Lens及其他多输出入口仍分别验收。

### 包 #10：关闭非末尾文档的视图归属（设备 F18）

打开两份新输出PNG与自建四层PSB，基线分别100%、150%、25%。关闭处于中间的150% PNG后，剩余PSB仍为25%、1920×1080、四层、原Smart Object选中；第一PNG切回仍100%/480×270。再关闭第一PNG，PSB仍保持上述状态。两次关闭后的PSB私有画布区域 `[520,510][2110,1690]` 与25%基线RGB逐像素比较均无差异，证明这份设备样本未发生缩放/中心错位。

证据 `filefix/package10-view-remap-psb-25-baseline`、`package10-view-remap-middle-150-baseline`、`package10-view-remap-after-middle-close`、`package10-view-remap-first-survives`、`package10-view-remap-after-first-close` 与 `package10-view-remap-canvas-comparison.json`。此处关闭的都是已保存、未编辑文档，不替代同时保存回执/新增编辑 F14 或独立OS窗口验收。

三份较长文件名同时打开时，原Pro第三个标签标题仅露出少量字符；原tab滚动/活动标签可见性正在检查和修复。已通过视图身份回归不等于此UI可用性问题已解决。

### 包 #10：JPEG 目录发布（设备 F17）

原 Image Processor 重选自建输入/输出根目录，Jpg、640×360、quality8、sRGB、2个输入。首页明确 Published 2 processed files; 0 inputs failed；新增独立child `PhotoCraft-Export-e6f38770-3bf2-466b-889e-2c8d98f042de`，此前PNG child仍列出。两份`.jpg`均从此child独立Open重开，640×360/RGB8/单锁定Background/clean，两笔测试画面正确。此例验证正常JPEG编码/目录复制和解码，不声称压缩后与PNG逐像素一致。

证据 `filefix/package10-processor-jpeg-ready-form-only.png`、`package10-processor-jpeg-publication-footer-only.png`、`package10-processor-jpeg-output-files-layout.json`、`package10-processor-jpeg-first-external-reopen-crop-0.png`、`package10-processor-jpeg-second-external-reopen-crop-0.png`。

### 包 #10：PSD 目录发布（设备 F17）

原 Image Processor 对同两份顶层PNG设置 Psd、1600×900、quality8、sRGB，选择自建根目录作为输出。首页显示 Published 2 processed files; 0 inputs failed；系统 Open 确认新增独立 child `PhotoCraft-Export-d80f419e-3249-49fe-8c36-633bdd743f8e`，此前PNG/JPEG child仍列出。两份真实`.psd`逐一从此child重开：ExportAs50 保留960×540，QuickExport由1920×1080缩小至1600×900，均RGB8、单Background、两笔测试画面正确且clean。该结果同时验证原处理器不放大小图；PNG输入本身为单层，本例不作为多层PSD保留能力的证据。

证据 `filefix/package10-processor-psd-ready-form-only.png`、`package10-processor-psd-publication-footer-only.png`、`package10-processor-psd-output-files-layout.json`、`package10-processor-psd-first-external-reopen-crop-0.png`、`package10-processor-psd-second-external-reopen-crop-0.png` 与 `package10-processor-psd-observation.json`。

### 包 #10：TIFF 目录发布（设备 F17）

原 Image Processor 对同两份顶层PNG设置 Tiff、width0、height0、quality8、sRGB，输入/输出均为专用自建目录。首页显示 Published 2 processed files; 0 inputs failed；新增独立child `PhotoCraft-Export-23f16936-b067-45b0-9943-b5f2ae3cc99e`，原三份格式输出child仍列出。两份`.tiff`分别从此child外部重开，尺寸各为960×540和1920×1080，RGB8、单Background、两笔测试画面正确且clean。零尺寸参数按原处理器语义保留原大小；本项不覆盖多层TIFF、其他provider或写入故障。

证据 `filefix/package10-processor-tiff-ready-form-only.png`、`package10-processor-tiff-publication-footer-only.png`、`package10-processor-tiff-output-files-layout.json`、`package10-processor-tiff-first-external-reopen-crop-0.png`、`package10-processor-tiff-second-external-reopen-crop-0.png` 与 `package10-processor-tiff-observation.json`。

### 包 #10：原 Actions → Batch 的真实内容处理（设备 F17）

打开测试TIFF，Window → Actions，原圆点开始录制，Ctrl+I执行反相，原停止按钮结束；Actions列出Action 1、1 steps、Invert。录制用工作文档另存独立 `PhotoCraft-Batch-Invert-Working-20261008.psd`，clean。原File → Automate → Batch 捕获Action 1，Same格式，专用fixture输入2PNG与输出目录；确认后Published 2 processed files; 0 inputs failed。

新增child `PhotoCraft-Export-48b44839-a365-4619-8078-adb0fa8802d5` 中两份真实PNG分别从系统Open重开：960×540与1920×1080，RGB8、单Background、clean，黑底白笔画，确实执行原反相动作。Batch完成后工作PSD名称、尺寸、图层与clean保留；私有画布区域`[520,510][2110,1690]`与执行前RGB逐像素一致。第一次比较仅在确认按钮退出后的38×38笔刷光标处不同，保留该证据；指针移至Actions标签后重新比较无差异，没有删掉差异样本或把带光标的图误称相同。

证据 `filefix/package10-batch-action-recording`、`package10-batch-working-saved-baseline`、`package10-batch-ready-form-only.png`、`package10-batch-publication-footer-only.png`、`package10-batch-working-after-pointer-clear`、`package10-batch-working-canvas-final-comparison.json`、`package10-batch-output-files-layout.json`、`package10-batch-first-external-reopen-crop-0.png`、`package10-batch-second-external-reopen-crop-0.png`。本项只覆盖一个Invert步骤和两张正常PNG，复杂Action、打印、部分输入失败和恢复另测。

### 包 #10：原 Lens Correction 目录流程（设备 F17）

原File → Automate → Lens Correction，Auto scale关闭、Correct CA/Distortion/Vignette开启、Edge extension、Png、Generic；选择专用输入2PNG和输出目录。Confirm后首页Published 2 processed files; 0 inputs failed，新增独立child `PhotoCraft-Export-d2cde8ff-19f3-4b84-aecc-ccf038cbfe79`。两份PNG逐一从此child外部Open重开，分别960×540和1920×1080、RGB8、单Background、clean，原黑白笔画画面可用。此无EXIF、纯黑白测试样本验证原参数捕获、处理/编码/发布/解码流程，不作为明显几何畸变、真实镜头profile、色差或暗角补偿效果通过的证据。

证据 `filefix/package10-lens-ready-form-only.png`、`package10-lens-publication-footer-only.png`、`package10-lens-output-files-layout.json`、`package10-lens-first-external-reopen-crop-0.png`、`package10-lens-second-external-reopen-crop-0.png` 与 `package10-lens-observation.json`。目前六次正常目录任务（Image Processor四格式、Batch、Lens）的十二份结果均外部逐份重开；失败、权限撤销、恢复重试另行测试。

### 包 #11：Recent、保留输出恢复与原 Pro 标签

本批90项Rust（wrapper82、输入7、facade1）、138项production SDK/平台helper Node回归全部通过，Home Recent实际绘制断言、两组strict lint、fmt、原patch精确重放、实际wasm与API26编译通过；这些是本批回归，不累加到上游默认2986独立测试总数。四张原App离屏PNG已由实现者查看；不当作设备通过。日志及377源清单见 `logs/regression/recent-folder-recovery-20261008`。

根封存377份产品源码，构建前后与独立快照均0 mismatch，source manifest SHA256 `6591867e1171a63f71db70f80bd5c93dd82ebfe172598909ed40846d6223a693`。OHOS release6m09s、签名Hvigor12.119s（CompileArkTS4.351s、Sign2.674s）通过。不可变包目录 `logs/build/recent-folder-recovery-20261008`：HAP48,518,769 bytes，SHA256 `d5c40746dc7295902d3700f35cc18c8e53af439744cbac377f26bd1df832c8b8`；SO46,328,104 bytes，SHA256 `98c4d713c902652abd39d83080360da2cbfa76b1e27ba57a2339d45e1c930635`；19既有ABI/7个NEEDED不变，公用build profile已恢复。

Recent以外部URI逻辑身份去重并显示友好名称，打开时从既存授权重新导入，失败不退回旧cache。保留目录输出重试核对owner、清单、长度和BLAKE3摘要，发布原编码字节且不重跑engine/Action；无Rust sidecar的旧输出严格验证SDK journal并要求新授权，未知输入失败数不称0。原Pro文档条复用既有fit/overflow，活动文档可见且所有隐藏文档可切换。实际设备安装、Recent/恢复/标签验收结果另行追加；详情 [本批验证与边界](../ohos-notes/recent-folder-recovery-rust-validation.md)。

### 包 #11：旧目录输出恢复的设备结果

安装后的真实首帧通过。首页 Recent 显示友好文件名和 System file，内部导入路径不再显示；同名但不同外部 URI 的文件仍是独立条目，不能据此判定去重失败。旧 Image Processor 的两份保留文件在原状态栏入口可见。直接 Retry 明确要求重新选择目录；取消系统选择器后提示 output retained，记录保留。无文档时用系统窗口 X 退出，确认原进程消失，再启动新进程，待恢复的两份文件仍在。

此后一次 Choose another folder 操作出现窗口及进程消失，未出现选择器；自有目录安全检查拒绝继续选择，未记作发布成功。仅本应用旧 PID 的 PhotoCraft 日志查询没有返回行；本应用 fault 文件目录不可读，故原因尚未确认。保留失败证据 `filefix/package11-recovery-reselection-unexpected-exit-observation.json`，不能以空日志声称未崩溃。

再次启动同一不可变包，新 PID 54506 下重新操作成功，30 秒存活采样始终是同一 PID。真实目录选择器出现，选择专用测试根目录后显示 Published 2 retained files; original input failure details unavailable，恢复入口消失。新 child 为 `PhotoCraft-Export-9eca12a0-937a-4ba9-80e9-2e4edcaa4487`；其中 ExportAs50 与 QuickExport 两份 PNG 均从系统 Open 逐份打开，均为480×270、RGB8、单 Background、clean，原白底黑色两笔画可见。两份来自旧保留任务，不能混称包#10的960×540/1920×1080正常输出。

证据 `filefix/package11-recovery-reproduction-process-observation.json`、`package11-recovery-reproduction-after-own-pid-hilog.txt`、`package11-recovery-reproduction-after-choose-layout.json`、`package11-recovery-reproduction-publication-result-crop-0.png`、`package11-recovery-first-external-reopen-crop-0.png`、`package11-recovery-second-external-reopen-crop-0.png`。结论为此旧记录的授权、取消保留、重启保留、发布与外部重开通过；一次异常退出仍待定位，包#11新 Rust sidecar 的失败恢复与权限撤销尚不能称设备通过。

### 包 #11：Recent 鲜读与标签溢出的设备结果

先从首页 Recent 打开上节 recovery child 的 ExportAs50.png，确认为白底两笔画、480×270，然后关闭未修改文档。打开独立 Batch child 的黑底白笔画 QuickExport.png（1920×1080），以原 Save a Copy → PNG 保存到 recovery child 的既存 ExportAs50.png；安全检查核对专用根目录、精确 child、既存文件名、PNG 格式和系统替换提示后才替换。Copy 后原 QuickExport 的名称、clean 和1920×1080保留，Recent 的原 ExportAs50 条目身份保留。

点击旧 Recent 条目后，得到同名 ExportAs50.png 的黑底白笔画、1920×1080、RGB8、单 Background、clean，未弹出额外选择器。它与旧480×270白底缓存明确不同。关闭文档并用系统 X 退出，确认 PID为空，再启动新 PID8943；Recent 顶部仍是这一逻辑外部文件，打开后仍得到1920×1080黑底内容。固定画布ROI黑像素98.7356%、白像素1.0814%，重启前后相同；该ROI是内容抽样，不声称整个PNG字节一致。Recent没有给这一重复打开的同URI再增加一条，其他同名文件来自不同URI，保留独立条目。证据 `package11-recent-first-owned-png-crop-0.png`、`package11-recent-replace-overwrite-result-layout.json`、`package11-recent-source-replaced-through-copy-crop-0.png`、`package11-recent-fresh-external-content-crop-0.png`、`package11-recent-fresh-restart-process.json`、`package11-recent-fresh-external-after-restart-crop-0.png`、`package11-recent-fresh-pixels.json`。

三个长文件名文档的标签均缩短并可切换，画布480×270/1920×1080与各自身份对应。再建立五个未修改空白文档，总八文档时七条可见并出现»，隐藏文档可由菜单激活且活动标签留在栏内；关闭这个空白文档后七条重新排列、»消失。随后逐项关闭所有未修改文档回到首页。原 New Document 默认名称在这些空白文档间相同，本项以创建/关闭数量和菜单行为核验，不声称单凭相同标题识别每个DocId。证据 `package11-three-long-document-tabs-crop-0.png`、`package11-three-tabs-switch-first-crop-0.png`、`package11-three-tabs-switch-second-crop-0.png`、`package11-eight-document-tabs-overflow-strip-only.png`、`package11-document-overflow-popup-only.png`、`package11-overflow-hidden-document-selected-strip-only.png`、`package11-overflow-seven-documents-strip-only.png`。

## 2026-10-08 包 #12：Revert 与链接资源来源

新签名包 `logs/build/file-sources-20261008/entry-default-signed.hap` 为48,597,434 bytes，SHA256 `5b144080f1e95a2138781137321ae3df1740023b3f3fe6ea8805c1b803990470`。379份精确产品源码、SO及293,054,768 bytes静态库已独立封存，构建前后源码零差异；19个应用ABI、7个系统NEEDED及公共profile恢复均核对通过。真实OHOS release3m28与签名构建通过；当前PC安装成功、原UI首帧通过，报告 `logs/device-smoke/2026-10-08T02-28-21-492Z`。

最终103项平台回归、13项新增来源复验、37项原引擎File/Smart/Automate回归、13项生产SDK adapter通过；新增来源复验属于103中的受影响项，不重复累加。四包all-targets strict、UI direct strict、fmt、两overlay重放、wasm、API26 ArkTS均通过。来源事务保留原文档/图层/版本，Revert、Linked刷新/替换/重链/导出/转换经授权来源重新读取；取消、失败、新编辑和原文档关闭均不能覆盖旧内容。其真机功能正在逐项验收，尚不把主机结果记为真机通过。原同步脚本/Actions/Batch异步来源接续仍在下一批处理。详细自动证据见 [来源验证](../ohos-notes/file-platform-source-rust-validation.md)。

包 #12 Revert 真机验收：通过Recent打开自建1920×1080黑底PNG，Ctrl+I产生1920×1080白底dirty修改；另一独立480×270白底文档用原Save a Copy、系统PNG格式与唯一自建文件覆盖确认改变同一外部原文件。原File → Revert仍显示未保存确认：Cancel保留1920×1080/dirty/原标签；Revert确认不打开选择器，在原标签读到新的480×270白底内容并clean，另一文档保持原状。Undo恢复此前1920×1080编辑并dirty；Redo恢复480×270内容但仍dirty，此标记行为保留观察、待与原版history语义比较，不称clean通过。记录见 `logs/device-validation/filefix/package12-revert-observation.json` 及其9份命名证据；内部DocId归属由主机回归验证，设备记录只证明可见标签/内容状态。

包 #12 关联资源真机验收：新建1920×1080白底父文档，用原Place Linked选择自建recovery child中的480×270白底PNG，生成居中于(720,405)的Smart Object，父文档两层且dirty。另一独立480×270文档反相后，经原Save a Copy、实际系统PNG格式及精确自建文件替换确认，将同一外部源改成黑底白笔画。父文档仍保留旧白底内容，原Edit Contents重新读取黑底480×270内容到子文档；关闭未编辑子文档后，父文档仍是旧预览。原Update Modified无额外选择器即将父对象更新为黑底，位置、尺寸、两层及父画布不变；Undo恢复旧白底对象；原Update All再读到黑底对象。本次只有一个关联源，不能代替多个源部分失败的测试。

记录见 `logs/device-validation/filefix/package12-linked-source-observation.json`。证据名 `package12-linked-source-updated` 实际对应误点后的Edit Contents子文档，保留原证据名并注明真实操作，不算Update成功；真正Update证据为 `package12-linked-modified-content-applied`，Undo及Update All分别为 `package12-linked-update-undo`、`package12-linked-update-all-applied`。仅使用自建文件；不声称设备截图能证明内部LayerId或整个外部PNG字节hash。

同一父文档后续原Export Contents生成 `PhotoCraft-SmartExport12-20261008.png`，为原内容字节保存器，无格式转换；从系统Open独立重开480×270黑底白笔画、单层且clean。Relink取消保留父对象/dirty；Replace Contents选择自建白底QuickExport后变白，原引擎语义为嵌入替换，图层原名保留。Relink到刚导出的黑底PNG后重新变黑；Convert to Embedded无选择器即完成、父变换保留。Convert to Linked取消保留，再确认发布新 `PhotoCraft-Linked12-20261008.png` 后才显示Linked；其外部Open仍480×270黑底、单层、clean。以上正常和取消路径各有命名设备证据，不代替发布失败或授权撤销。

父项目实际系统格式选择PhotoCraft，保存新 `PhotoCraft-LinkedParent12-20261008.pcraft` 后外部名与clean正确；关闭后从系统Open独立重开，1920×1080、两层、居中480×270对象与黑底预览保留。重开后的标题/标签显示序列化内在名称Untitled-1，footer显示真实外部文件名；只读核对固定原版io bundle loader及原UI open_file/open_bytes同样保留内在DocM.name，沿用其语义，不修改核心格式。此项项目重开并非进程重启。

随后用独立白底480×270文档的原Save a Copy及实际PNG格式，核对精确自建根目录/Link12名称/系统替换提示，将新Link12外部源变白。已重开父项目仍保留黑底预览；原Update Modified无需选择器即读到白底黑笔画，父画布1920×1080、两层与对象(720,405)/480×270保持，dirty正确。这验证转换生成的关联源在原生项目序列化/重开后仍可授权鲜读。证据 `package12-linked-roundtrip-before-update`、`package12-linked-roundtrip-fresh-update`。

### 包 #13：Actions / Scripts 的来源接续与关闭等待

实际OHOS release 3m09s、完整HAP构建9.492s及签名通过。签名包48,734,755 bytes，SHA256 `e2c4aac17c366e714d27e77ff7041ab17d9c720819af89f60a53601375e35f54`；386份源码构建前后及tar快照零差异。产物与源码分别冻结于 `logs/build/async-source-continuation-20261008`、`logs/regression/async-source-continuation-20261008`。

本批主机验证为98项wrapper、8项新增引擎接续、33项原File/Automate/Jobs通过，原24MP计时项保持ignored；170项生产Node合同通过，其中剪贴板、关闭drain和诊断观察器不重复累加。strict、fmt、两overlay重放、wasm及API26编译通过。来源操作完成后才继续原Actions/Script，Batch逐输入保留独立scratch与发布回执；关闭等待实际文件绑定/fsync、剪贴板读取/清理和目录工作结束。独立目录脚本执行与Actions持久化仍未完成。

真机已安装、启动和呈现首帧，报告 `logs/device-smoke/2026-10-08T03-59-43-629Z`。随后发现用户正在编辑文档，未发送任何点击、按键或关闭操作，保留现场并暂停真机功能测试。本批Actions/Script新功能尚未计为真机通过，启动验证不替代功能验收。更早包 #11 一次未明原因进程退出仍保留为未解释异常，新诊断观察器不作为该异常已修复的证据。
