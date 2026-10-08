# G1：OHOS 交叉编译

基线：`4337a6227a823a28728e68aed844feab62b3314d`；Rust 1.97.0；本机 DevEco SDK API 26。

已通过：

```sh
cd upstream
cargo check --locked --target aarch64-unknown-linux-ohos -p photocraft-engine
cd ..
cargo check --target aarch64-unknown-linux-ohos --workspace
```

初始 G1 使用原始引擎及库源码；后续打印的最小 engine overlay 见下方批次记录。
原版 desktop/web/CLI 不属于 OHOS
workspace，不能用上游 `--workspace` 作为本移植的验收命令；那会编译不支持 OHOS 的桌面窗口依赖。

| 断点 | 解决方式 | 上游修改 |
|---|---|---|
| eframe 0.36.2 的 native 依赖会无条件拉入 winit | 独立 workspace 的 Cargo patch 选择 `vendor/eframe` 应用接口适配器，runner 直接驱动 egui | 无 |
| path dependencies 被父 workspace 自动识别，破坏上游 package inheritance | 明确 exclude `upstream` 与其 crates/apps/xtask | 无 |
| egui 0.36 RawInput 无 modifiers 字段 | 按新版事件序列发送 `ModifiersChanged` | 无 |
| wgpu 30 surface 与纹理 API 已变化 | 使用真实 InstanceDescriptor 构造、CurrentSurfaceTexture、Queue::present 与多段 texture delta | 无 |

本次没有 getrandom/libc/引擎文件系统补丁。`target_os="linux"`、`target_env="ohos"` 是
该 Rust target 的真实 cfg，不能使用 `target_os="ohos"`。

根 Cargo.lock 固定本移植依赖；原版上游 Cargo.lock 保持不变。`cargo tree --target
aarch64-unknown-linux-ohos -p photocraft-ohos` 未包含 winit、egui-winit、rfd、arboard 或 muda。

2026-10-07 主机测试已通过：

```sh
cargo test -p craft-ohos-platform -p eframe -p photocraft-ohos
```

实际日志 `/tmp/photocraft-ohos-tests-final.log` 记录 15 个单元/集成测试通过：输入适配 6 个、
eframe 应用回调 1 个、PhotoCraft runner/services/fonts 8 个，0 失败。三个包的 doc-tests
均通过（各 0 个用例）。这是主机验证，不作为 OHOS 设备运行或 GPU 出图证据。

后续：完整原生链接、HAP 打包、真机与语料验收以其对应记录为准，cargo check 不能证明运行成功。

2026-10-08 下一平台包 Rust 已通过 51 项主机测试（photocraft-ohos 43、输入 7、
eframe 1），0 失败。新增覆盖可见 PasteButton 的合法临时 IME detach、焦点代际变化
拒绝、成功 Save 的目标跨 worker 重建恢复、损坏/冒充 journal 拒绝、保存目标实际
basename 在 Recent/重开中的一致性，以及 cursor/window 命令到真实 egui RawInput。
`fmt --check`、三包 `clippy --all-targets -- -D warnings` 与 690 行 overlay 重放均通过；
上游 checkout 保持 clean。日志：`/tmp/photocraft-paste-published-tests.log`、
`/tmp/photocraft-paste-published-clippy.log`。此条不声称该新包已完成 OHOS 链接或设备验收。

2026-10-08 打印下一批主机 56 项通过（PhotoCraft 48、输入 7、eframe 1），0 失败。
五项打印回归覆盖原 Print dialog 的 A4/CMYK/裁切标记 PDF 与未修改 engine 字节 oracle
一致、One Copy 的原布局与份数意图、Actions 不录入内部 spool 路径、系统非终态与终态
区分、PDF 的取消/失败/成功发布保持工作文档 dirty，以及失败和八任务上限。
严格 clippy、fmt 和 735 行 UI overlay 重放通过；上游保持 clean，未在此构建 archive。
日志：`/tmp/photocraft-print-tests-final.log`、`/tmp/photocraft-print-clippy.log`。
系统任务、真实打印机、终态回调和物理份数仍待 OHOS 验收；原 files API 不预选打印机
与份数，系统对话框负责最终选择。

2026-10-08 engine spool 下一批主机 60 项通过（PhotoCraft 52、输入 7、eframe 1），
0 失败。新增真实跨文档脚本、Print Script Events（含原递归保护）、Batch 临时会话
打印及 pending job 原文档身份测试。PDF 导出后再次打开原 Print dialog 和 One Copy
确实生成系统请求，原 Actions/journal 保留用户参数，last_print 不带 sandbox output。
成功 Save As 同步文档标题；取消/失败/Copy/Export 保持原标题。submitted 状态明确为
请求已受理但最终任务状态未确认，预览关闭没有公开终态时保留 PDF。

`vendor/photocraft-engine` 是独立 manifest 的第四个 OHOS workspace member，保留
原 PDF/layout 算法，只增加可选 Session spool service 并向 Batch scratch 传递它。
engine 原测试 635 项通过（616 unit、19 integration），13 项默认 ignored；另外显式
运行全注册表 `panic_hunt` 1 项通过。四包 strict clippy 与 fmt 通过；原 xtask layering
checker 对实际 20 个 PhotoCraft path packages 检查无违规。178 行 engine patch 与
736 行 UI patch 逐字重放通过，上游 checkout clean。

日志：`/tmp/photocraft-engine-spool-host-tests-final.log`、
`/tmp/photocraft-engine-overlay-tests.log`、`/tmp/photocraft-engine-overlay-panic-hunt.log`、
`/tmp/photocraft-engine-spool-clippy-final.log`、`/tmp/photocraft-engine-overlay-layers.log`。
安装官方 Rust 1.97 wasm 标准库后，实际 engine overlay 的
`cargo check --offline --locked --target wasm32-unknown-unknown -p photocraft-engine`
通过（23.02s）；日志 `/tmp/photocraft-engine-overlay-wasm.log`。存在原版 `OPENABLE`
常量在 wasm cfg 下未使用的 warning，未改动该无关代码。
本条没有构建或覆盖已冻结的 OHOS archive，系统打印终态仍需设备独立验收。

2026-10-08 #8 目录输入 Rust 源码冻结：67 项 host 通过（PhotoCraft 59、input 7、
eframe 1），包含原 LoadStack / ContactSheet / Statistics 的真实文件结果和 7 项目录
目标、取消、manifest、链接、空输入及原表单绘制回归。4 包 strict clippy、fmt 与
UI 909 行 / engine 196 行 exact replay 通过，上游 clean。engine 只公开既有
list_images 的两处 cfg 定义，不改枚举或排序算法；原 LoadStack/Processor/Batch
targeted 回归通过。独立锁定 wasm probe 显式 webgpu feature，UI 与 engine 编译通过。
详见 [Rust 验证说明](folder-input-rust-validation.md) 和
[ArkTS/JSON 接缝](folder-input-protocol.md)。这是源码与 host/wasm 验证，下一 archive、
HAP 和真实 provider fixture 由 root 继续，不在此宣称目录真机 PASS。
