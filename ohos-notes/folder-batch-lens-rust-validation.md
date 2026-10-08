# PhotoCraft #10 — Batch / Lens Correction 目录接缝

本批编译源码已冻结，主机与兼容检查全部完成。root 已构建 OHOS release candidate；HAP 签名、不可变包核对和本批真机验收由 root 继续，本文不将主机结果记为设备通过。

## 原业务与接缝

原 File › Automate › Batch / Lens Correction 的 input/output 行复用 Browse；不添加 ArkTS 业务表单或顶部按钮。原 Batch 打开表单时 clone 当前 Actions 步骤，后续授权/分配等待期间不重读已修改的 Action。original form 的参数经原 `filter_dialog::params_of` 捕获，独立 worker Job 保存 command/params。

沿 #9 typed `folderImport`、`folderDestination`、`folderStageAllocate`、`folderPublish` 及取消/回执协议；只增加两条 command intent 的白名单。相同 target 持续绑定原表单 generation/document 与 opaque destinationHandle；URI 仍只在 ArkTS 的私有授权记录中。各阶段 id 独立；接受入队才转移 ownership，错误/取消保留未完成发布的完整材料。

engine 仅把三条目录命令的 output lookup 提取为共享的可选 `Session.directory_output` resolver。None 维持原 desktop/wasm 路径；平台 grant 同时匹配 command+handle，单次消费已校验的独占空 stage，无 Session 重入。原 `process_files`、scratch session、逐文件 import/action/filter/save、OutputClaims、same/指定格式、lens flatten、色彩/采样算法均不复制。Batch scratch 的 print_spool 继承保持，目录发布不会完成系统打印任务。

普通原目录 Confirm 成功时清理之前 Folder cancelled 等旧 footer，再设置真实 pending/status。编码仍复用原同步 engine 调用，取消在编码前/后及系统复制阶段执行，不宣称可中断单次同步 codec。任意嵌套脚本的目录授权与依赖调度仍不属于这一表单接缝。

## 文档视图归属

原 upstream ui-egui `sync_views` 只对 `Vec<View>` resize/truncate、对 DocWindow 按 index<n retain；engine `Session.close` 的 docs.remove(index) 会左移。故关闭非末尾文档后后继 tab/canvas zoom、center 及 extraWindow document index 会错位。这是原 UI index 缺陷，不是已确认的文件串写。

最小 UI overlay 记录每个位置的 DocId，仅在文档身份顺序变化时按 id 迁移 View 和 DocWindow index；消失文档的窗口删除，新文档默认 camera，存活 doc 的 name/layers/active_layer 内容不变。常规帧不重建 view map。保存 completion 继续按 DocId/revision/path 绑定，改名不重置 camera。

无文档 Home 的原 Pro footer 原先在 `No document` 后立即 return，隐藏了文件夹错误/进度。overlay 在该分支复用原 status 文本/颜色/分隔线；测试运行完整 App logic/UI，检查绘制 Text shapes 里真实出现 error/progress/success，未新增调试条。

## 新增业务回归

- Batch 捕获原 Action 后再更改 Actions，依然执行已确认步骤；真实 PNG 反相和 PSD 图层结果、same 格式、scratch session 的四个独立 PDF/doc id、原工作文档 dirty/identity 及 journal 参数均检查。目录 publish ACK 不能完成这四个 print jobs。
- Lens Correction 真正处理 16×16 PNG 与两层 PSD：原 flatten、尺寸、手动畸变透明边缘、vignette 像素改变；正常 Confirm/目录发布回执前后原工作文档不变。16×16 的最外角 `(0,0)` 才处于畸变透明边缘，不能照搬大图的 `(1,1)` 几何假设。
- 两命令保留损坏输入及 case-insensitive OutputClaims 的逐文件错误，发布失败保留所有实际编码材料。取消 allocation 不执行原业务，command/handle 不匹配不能借用其他原表单的 grant；冒充 cancelled owner marker 不能删除已有目录。
- 三个真实文档分别关闭首个/中间文档，存活 zoom/center、selected layer、完整 document、extraWindow 归属保持；真实 PSD 编码/重开生成新 camera。另验证 close/open 在同一次 repaint 之前发生、文档数量不变时仍按 DocId 映射。

## 验证

ArkTS：74 项相关 production helper tests 通过（#9 的68+新4+本批修复权限持久化的2项），新增 Batch/Lens 的真实 input copy 与 authorization→allocation→publication 单 picker 原 target 流程；API26 独占 snapshot `/private/tmp/photocraft-batch-check` 最后 CompileArkTS 通过 21.025s / 整体36.418s，五个受影响 ETS 与该编译快照逐字节一致。Index、manifest、Native ABI 未改。商业 provider 与真实菜单结果待 root 本批 HAP 验收。

本批同时包含 root 对目录 publisher 的真实设备修复：表单 Output Browse 已有持久授权时，Publication 只 check/activate/stat，不再次 persist 消耗临时授权。两项 provider fixture 分别验证第二次 persist 永远被拒绝仍可使用既存 grant 发布，以及已撤销 grant 在任何 provider 写入前失败并保留完整 stage。

Rust：82/82 通过（PhotoCraft adapter 74、公共 platform 7、eframe facade 1）；原 engine 本次受影响测试 17/17 通过（file_cmds 16、Lens Correction batch 1）。没有重复执行旧 635 项全套回归。strict clippy 对四 workspace packages 的 all-targets 通过，另直接选 UI overlay 的 lib 做 `--no-deps -- -D warnings` 通过，避免 dependency cap 掩盖新 UI lint。workspace/UI fmt 检查通过。

两份 patch 均从 pinned upstream 独立重放并逐字节还原 vendor；upstream checkout clean。原固定 wasm probe 的实际 `wasm32-unknown-unknown` check 通过，保留原 `OPENABLE` cfg dead-code warning；未修改原算法或关掉 wasm 路径。

root 的 375 个冻结源文件与当前源逐一 SHA-256 复核，0 mismatch。原 Index、PlatformBridge、manifest 和 Native 在本批未改；五个受影响的目录 helper 与 API26 已编译 snapshot 相符。编译源码、patch 与测试在最终 HAP/SO 核对前保持冻结。

完整日志和 wasm probe 源/lock 位于 `logs/regression/folder-batch-lens-20261008/`；结构化结果为 `verification.json`，对应冻结源码清单为 `source.sha256`。原运行使用 `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/host` 和 `--offline --locked`。
