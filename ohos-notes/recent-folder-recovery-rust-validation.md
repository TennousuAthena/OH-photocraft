# PhotoCraft #11 — Recent、保留目录输出恢复与 Pro 文档标签

本批仅修改 PhotoCraft wrapper、UI overlay、DocumentBridge 与 Index 的文件请求接线。原 upstream checkout 保持清洁，engine overlay 及 PDF、codec、目录处理算法未改。没有增加 toolbar、Native ABI、manifest 权限或系统 UI 表单。#10 的不可变包和归档源码不受本批修改影响。

## Recent 的来源与显示

原 File › Open Recent 与 Home 列表通过可选 Services hook 取得 basename、友好来源文字与逻辑身份；相同外部 URI 的旧 staging/import 记录合并，列表和 tooltip 不显示内部路径或 URI。没有 hook 时仍执行原桌面路径。

选择 Recent 后沿现有异步 Open 请求发送 `intent:file.openRecent`、`chooseDestination:false`、`previousPath`。ArkTS 从自己的持久绑定记录查外部源，确认既存 READ 授权并 activate 后重新导入。Rust 只打开此次新返回的沙箱文件，不把旧 cache 内容当最新外部文件。源已撤销、删除或绑定缺失时显示错误，原文档及 dirty 状态保持。重新导入不伪造一次成功保存，不绕过原 flat import 的 Save As 保护。

`source-uris.json` 限 1 MiB、4096 项，每条 URI 限 8192 字节；两端拒绝非法目录、链接与越界 path，解析完全部记录才替换 catalog。Rust 只在初始化或收到显式 `recentSourcesChanged` 时读取；ArkTS 对该通知采用 bounded enqueue 重试，不因通知背压改写真实 Save 的成功结果。相同 URI 的身份比较不猜测 provider 的 URI 归一化规则。

## 目录输出的恢复

原 status footer 出现 `Recover folder outputs…`，打开原位置的 popup 可 Retry 或 Choose another folder。失败、取消与复制结果尚不明确的输出始终保留；重试只沿现有 typed destination/publish 协议发布精确保留字节，不调用 engine、Actions、Print 或任何编码器。每次发布独立 attempt id，原 allocation、target 和输出树身份保持。

首次发布前原编码输出写入独立 `recovery.json`，记录文件/目录清单、长度与流式 BLAKE3 摘要。记录、owner marker 和 SDK publication journal 均有大小限制；恢复验证请求归属、target、allocation、无链接祖先、清单与全部内容。SDK rename 后只解析 journal 明确记载的两个合法位置，不用任意 single-child 推断。改字节、同大小替换、链接、冒充 owner 或错配 journal 均拒绝发布并保留材料。

启动可重新发现完整恢复记录。旧 #9/#10 输出没有 Rust 内容 sidecar 时，先验证原 SDK manifest/owner/journal 与现存树，然后建立当前摘要；原输入失败数未知，UI 不称零失败，且必须明确重新选择目录后才能发布。复制可能已经产生外部部分结果时显示警告，重试使用新的外部 UUID 子目录，不宣称之前没有副作用。损坏记录只报告原 footer 错误并保留文件。

每个输出树上限沿现有协议为 500 个文件、4096 项及 1 GiB，恢复记录发现上限 128 项。当前启动验证会流式读取保留文件；大规模保留任务的启动耗时和真机恢复 UI 尚未测量。

## Pro 标签可达性

原 Pro 文档条复用既有 `tab_strip::fit` 的 shrink/elide 与 `»` overflow。保留 26px 条高、颜色、关闭按钮和 opening-job progress；活动文档保持可见，overflow 能按 DocId 切换所有隐藏文档。真实三文档的全 App UI 输入/绘制回归检查活动文字位于条内并逐个点击 overflow 切换，未用替换 UI 或改期望掩盖溢出。

## 业务回归与验收边界

Rust 业务回归覆盖 Recent 去重与原菜单请求、取消/失败保持原 dirty 文档、非法 catalog；真实目录处理产生的字节在删除原输入后仍能 Retry，重建 wrapper 后可改授权目录，拒绝内容/链接/owner/journal 篡改，兼容无 sidecar 的旧输出。Recovery 不增加 Session journal、不改变原文档或其保存状态。最终日志、lint、wasm 与 API26 编译结果在本批冻结时补入结构化 verification 文件。

DocumentBridge 的 production SDK adapter 临时文件测试验证外部源被修改后 Recent 读取新 bytes、重启恢复绑定、既存授权不重复 persist、撤销或删除源不 fallback 旧 cache、越界/链接绑定拒绝。测试不等价于商业 provider 真机验收。

本批 HAP/SO 构建、签名、安装与设备验收由 root 完成；本文不把主机、ArkTS compile 或实际 egui 文本 shape 检查记为本批设备通过。

## #11 冻结检查结果

Rust：90/90 通过（wrapper 82、公共 platform 7、eframe facade 1），原 Home Recent 的额外实际绘制断言单独通过；四个 workspace 包 all-targets strict clippy 与 UI overlay 直接 `--no-deps -- -D warnings` 均通过。workspace 与修改 UI 文件的 fmt check 通过，两 overlay 从 pinned upstream 独立重放并逐字节还原。engine 本批未改，未重复旧全套引擎回归。实际 `wasm32-unknown-unknown` probe 通过，仅保留原 `OPENABLE` cfg dead-code warning。所有 Cargo/rustc 已结束。

Node：相关七个 production helper 文件合计 138/138 通过、0 skipped，其中 DocumentBridge SDK adapter 11 项包含本批新增的三项 Recent 来源回归。最后 API26 CompileArkTS 8.549s、整体14.758s 成功，Index 与 DocumentBridge 的 live 源和独占已编译 snapshot 逐字节一致。四张离屏 Metal PNG 已生成并查看：Home 友好来源、原 footer 恢复 popup、640px 窄标签及两条 overflow 选项均可见。视觉 fixture 及 renderer 的具体边界记录在 visual/README.md；真机/provider 验收仍待 root。

全部日志、PNG、wasm probe、结构化 verification 与 source-changes 清单位于 `logs/regression/recent-folder-recovery-20261008/`。`source.sha256` 包含 377 个产品源文件（#10 的375加本批两个 Rust module）；root 可据此生成 pre-build 精确快照和新 OHOS release。编译源码、patch、ETS 和测试已冻结，仅文档与日志继续归档，#10 immutable 不改。
