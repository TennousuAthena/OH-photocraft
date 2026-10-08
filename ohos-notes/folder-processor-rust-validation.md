# Image Processor Rust 接缝与验证

2026-10-08，#9 源码批。仅连接原 `file.scripts.imageProcessor`，输入、输出 Browse 在原 egui 参数表单对应行；格式、quality、width、height、convertToSrgb 与原 Confirm 保持。ArkTS 的协议、SDK 授权与 provider 证据见 [folder-processor-protocol.md](folder-processor-protocol.md)。原 upstream checkout 保持 `4337a6227a823a28728e68aed844feab62b3314d` 清洁。

## 执行与归属

输入延用 #8 的明确 `sourceBasenameChild` 布局、完整 manifest/no-link 核验及原 `file_cmds::list_images` 本层排序。输出 Browse 捕获原 dialog、output field、generation、document 和原字段值，系统选择完成只写不透明 destination handle；取消、目标关闭、generation 改变或 active document 改变不更新原表单。旧目标的合法迟到 handle 仅 release 一次，duplicate/unknown receipt 不 release 已接受的 handle。

Confirm 捕获完整原参数，先验证输入数组与应用拥有的普通文件，随后请求独占空输出 stage。选择、allocation、publication 各使用独立 JS-safe id，同一捕获 target 贯穿三阶段。stage 核验固定 `FolderPublication/<allocationId>-sixRandom` 父目录、ordinary/canonical/no-link、空 contents、有限 metadata、owner.json 的 version/creationId/target/ownerRoot，以及精确 journalPath。任意发布 attempt id 不得冒充 allocation owner。

引擎 overlay 仅增加可选 `Session.directory_output`，在原 Image Processor 获取 output 参数的位置解析一次 logical handle。callback 没有 Session 引用，不能重入；只有 matching allocation 已验证后才给出空 stage。原 `process_files`、scratch session、fit/dontEnlarge、颜色转换、编码和忽略大小写的 `OutputClaims` 都未复制或改写。原 journal 留逻辑 handle，不含 stage 或外部 URI。无 callback 时 desktop/wasm 行为保持；重建 worker 后过期 handle、直接脚本中未分配 stage 的 handle 必须明确要求重新授权，不能获得任意目录写权限。当前只给原 app.run/Actions 的 Image Processor 入口排队准备 stage，未宣称任意嵌套脚本中的目录处理都已适配。

原引擎完成整个编码调用后，Rust 核对实际 ordinary 输出树与原 `files` 汇总，再冻结 stage 并请求发布。原 `errors` 保留损坏输入和大小写冲突的细节，用户只看到文件名与可理解原因，不看到沙箱路径。所有输入均失败时不会发布空目录。现有 footer 显示准备/发布进度与 Cancel；系统复制取消等待 provider settle。原引擎此命令的编码调用本身是同步的，codec 内没有新增抢占取消算法；到达 worker 的取消在编码前后处理，不宣称可中断正在执行的 codec。

只有 matching SDK success receipt、原输出文件数与实际 owned tree 归属成立才报告 published。preflight error 的空 ownership fields 不获写权限，也不删除已有编码树；error/cancel 保留 stage、journal 和 possible partial 状态。SDK 已完成复制后迟到 Cancel 不会改写真实 success。成功后的本地 cleanup 失败只报告材料保留，不撤销已完成的外部发布。所有阶段均不修改工作文档、path、revision、saved revision。一次仅有一个平台目录操作；bounded mailbox 满时保留待发布材料，Cancel 能解除尚未发送的发布请求，防止永远等待一个不存在的 SDK job。

已使用的授权在本进程保留，重复原命令再次分配新 allocation、新 owner 和 fresh publication child，不能复用上一棵输出树。unused/stale 选择会按 strict handle+target 发一次 release；只删除 private authorization record，不撤销共享 grant。

## 主机回归

8 项新增 Rust 测试直接走原菜单/表单与 command registry，使用真实 PNG/两层 PSD，不复制引擎算法：

- 四种原格式 PNG/JPEG/PSD/TIFF：核验真实 bytes、重新导入尺寸与像素、PSD 层保留、dontEnlarge、原 journal logical output；provider fixture 的原 marker 保持，完整新 child 才算成功。
- 损坏 PNG 与 `a.png`/`A.psd` 同输出名冲突：原 `files/errors` 分别保留，发布成功仍显示失败输入细节。
- Output Cancel、关闭目标及 duplicate：旧字段/合法授权保持，迟到新 handle 只 release 一次。
- allocation Cancel、publication Cancel/error 与 preflight error：工作文档和 saved 状态保持，未发布空 stage 仅验证后 cleanup，已编码材料和 journal 保留。
- Confirm 后切换 active document：处理捕获的原输入，迟到 Cancel 不把真实已复制结果改写为取消，两个工作文档都不变。
- 伪造 owner target 与过期 logical handle：不执行实际目录写入，不获 ambient filesystem authority。
- 重复原命令：复用授权且 allocation/owner 独立，两次输出真实完成。
- bounded output queue：原编码后发布回压，再 Cancel 时保留材料并解除等待。

wrapper/platform/facade 最终主机回归 75 项通过（PhotoCraft 67、platform input 7、eframe 1），含真实输出像素与原表单两处 Browse 的 egui shape 检查。原 bicubic 在有限层边缘产生部分 alpha，JPEG 会 matte；测试保留该原算法语义，没有用 opaque-edge 假设改写 resize。

四包 all-targets 严格 clippy 与 fmt 通过；engine 216 行、UI 1001 行 patch 逐字节 replay 通过，上游 checkout clean。使用已锁定的独立 webgpu wasm probe，实际 engine 与 UI overlay 的 `wasm32-unknown-unknown` 编译通过（22.95 s），只有原 `OPENABLE` 在 wasm cfg 的 unused warning。原 engine `file_cmds::tests` 16 项针对回归通过（0.32 s），覆盖未设置 callback 的 Image Processor/Batch 等原行为；未重复无关的 635 项全套。商业 HarmonyOS PC 的真实 folder picker、preauthorization、copy layout 和 publication 本批尚未真机验收，不能将 host provider fixture 记为设备 PASS。

日志：`/tmp/photocraft-processor-host-tests-final.log`、`/tmp/photocraft-processor-clippy.log`、`/tmp/photocraft-processor-wasm.log`、`/tmp/photocraft-processor-engine-regression.log`。

## 检查入口

```sh
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/host cargo test --offline --locked -p photocraft-ohos -p craft-ohos-platform -p eframe --lib
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/host cargo clippy --offline --locked -p photocraft-ohos -p craft-ohos-platform -p photocraft-engine -p eframe --all-targets -- -D warnings
cargo fmt --all --check
python3 vendor/photocraft-engine/verify-overlay.py
python3 vendor/photocraft-ui-egui/verify-overlay.py
```

本批源码检查已结束并冻结，没有构建或覆盖已签名 #8 archive/HAP；由 root 统一构建 #9 archive 并做 provider fixture。所有本任务 host Cargo 已退出，可以按 root 的统一缓存策略维护 host target。
