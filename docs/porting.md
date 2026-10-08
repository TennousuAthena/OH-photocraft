# PhotoCraft 鸿蒙移植工程说明

当前是基于 PhotoCraft `4337a6227a823a28728e68aed844feab62b3314d` 的原生移植工程。
保留上游 Rust 图像引擎和 egui 编辑器，ArkTS 负责 UIAbility、XComponent 容器与系统文件选择器。
首版 OHOS arm64 release 与签名 HAP 已在 API 26 MateBook Pro 启动；当前采用上游 CPU 文档合成器与 GLES 界面展示。
正式 Open/Open As、Save/Save As/Save a Copy、PNG/PSD/pcraft/JPEG、原导出参数、单个文件拖入、恢复与基础编辑已通过真机验证，临时 PNG 按钮已移除。系统中文候选提交与保存重开、可见文字粘贴授权、系统 PDF 打印终态、文件夹 Stack/Contact Sheet 和两份文档交替保存也已有设备证据；整体验收仍在进行，实际包与结果见 [功能验收记录](photocraft-validation.md)。

默认语言为「自动（跟随系统）」。UIAbility 在启动、系统语言变更和返回前台时同步系统语言，
编辑器自动使用对应词库；简体中文识别 `zh-Hans`、`zh-CN`、`zh-SG`，繁体中文识别
`zh-Hant`、`zh-TW`、`zh-HK`、`zh-MO`，明确的文字标记优先于地区。
文件和粘贴等系统适配提示使用鸿蒙 `base`、`zh_Hans`、`zh_Hant` 字符串资源。
可在首选项的「界面 → 语言」选择简体中文、繁体中文或恢复自动；手动选择会保留。
没有对应编辑器词库的系统语言回退英文。词库与资源检查见
`python3 scripts/check-localization.py` 和 `node --test tests/system_language.test.cjs tests/shell_localization.test.cjs`。

2026-10-08 本地化验证：1457 个必需界面词条检查、102 项原生测试、245 项 ArkTS 主机测试通过；
arm64 Rust、原生桥、ArkTS 和未签名 HAP 构建通过，并核对包内英文、简体及繁体资源。
实机系统语言切换与中文界面布局仍需设备验证。

本文保留移植过程的工程细节和历史验证记录。`logs/` 中的构建产物、设备日志、截图与私有签名材料均为本地证据，不随 Git 仓库发布；各项结论对应其记录的源码批次和设备包，不代表当前版本已完成全部实机验收。

## 工程结构

| 路径 | 用途 |
| --- | --- |
| `upstream/` | 固定提交的上游源码，保持原样 |
| `apps/craft-ohos-platform/` | egui 输入与原生图形平台适配 |
| `apps/photocraft-ohos/` | PhotoCraft 生命周期、服务与 Rust FFI |
| `vendor/eframe/` | 不依赖 winit 的应用回调兼容接口 |
| `vendor/photocraft-ui-egui/` | 可重放的异步保存与文件意图适配 overlay |
| `vendor/photocraft-engine/` | 保留原算法的可选系统打印与输出目录接缝 |
| `harmonyos/` | DevEco Stage 工程、ArkTS 与 C++ NAPI/XComponent 桥 |
| `scripts/` | 环境检查、交叉编译与 HAP 构建 |

OHOS 使用独立 Cargo workspace，通过 path dependencies 消费上游 crates。
只有该 workspace 的 `[patch.crates-io]` 选择 eframe 兼容接口；从上游目录构建桌面版仍使用原版 eframe。
版本和来源见 [UPSTREAM.md](../UPSTREAM.md)。

## 构建

需要 Rust 1.95 或更高版本、`aarch64-unknown-linux-ohos` target，及包含 native 工具链的 DevEco SDK。
构建脚本默认寻找 `/Applications/DevEco-Studio.app/Contents`，缓存与编译产物放在本项目目录。

```sh
cd OH-photocraft
rustup target add aarch64-unknown-linux-ohos
./scripts/doctor.sh
./scripts/build-ohos.sh
```

按阶段构建可使用 `./scripts/build-ohos.sh --rust-only` 或 `--hap-only`。
`--hap-only` 要求已经生成真实 Rust 静态库。
可用 `DEVECO_STUDIO_CONTENTS`、`OHOS_SDK`、`DEVECO_SDK_HOME` 指定本机安装位置；
`OHOS_SDK` 指向包含 `llvm/` 和 `sysroot/` 的 native SDK 目录。

Rust 静态库被复制到 `harmonyos/entry/libs/arm64-v8a/libphotocraft_ohos.a`。
HAP 构建输出位于 `harmonyos/entry/build/default/outputs/default/entry-default-unsigned.hap`。
脚本会检查包内存在真实 `libs/arm64-v8a/libphotocraft.so`。

## 本机调试签名

当前 bundle 为 `moe.kiwi.photocraft`。在 DevEco 打开 `harmonyos/`，连接目标 PC，进入
File > Project Structure > Project > Signing Configs，选择自动生成签名文件；
使用自己的已登录开发者账号，为当前 bundle 和设备生成调试 Profile。
不同 bundle 的旧 Profile 不能复用。开发者应为自己的设备生成授权。

签名配置仅保存在 `.signing/build-profile.json5`（目录权限 0700、文件权限 0600），该目录已被 Git 忽略。
DevEco 若把签名字段写入公开 `harmonyos/build-profile.json5`，先将完整配置保存到上述私有文件，
再删除公开配置中的 `app.signingConfigs` 和每个 product 的 `signingConfig`；不要提交密码、证书或私钥文件。

已有 Rust 静态库后，生成真实 unsigned HAP 并签名：

```sh
./scripts/build-ohos.sh --hap-only --signed
```

完整构建并签名可用 `./scripts/build-ohos.sh --signed`；单独重新签名可用 `./scripts/sign-local.sh`。
签名脚本核对 Profile 包名、为唯一 signingConfig 自动绑定 product，在构建期间临时合并签名材料，
并在 `finally` 中恢复公开配置内容和文件权限。不要与其他 HAP 构建或 DevEco 配置写入同时执行。
产物为 `harmonyos/entry/build/default/outputs/default/entry-default-signed.hap`。
安装和首次启动由设备验收步骤确认，签名文件生成本身不代表运行通过。

## 当前实现范围

- 引用真实 `PhotocraftApp`，通过 eframe 兼容接口保留其 `logic`、`ui`、`raw_input_hook` 回调。
- 用 XComponent 与 Rust 图形平台层连接原生窗口；wgpu 30.0.1 的 GLES OHOS raw-window-handle 路径负责展示 egui UI 和纹理，上游 CPU 合成器负责文档图像。
- Rust OHOS cfg 为 `target_os="linux"`、`target_env="ohos"`。键盘由 NDK 回调进入系统 IME，未消费按键转回原 UI；鼠标、触摸、笔和 axis 由 NDK 回调转发。IME 使用真实文档/文本目标、UTF-16 快照及窗口身份；基础中文候选提交已有设备证据，物理键盘及预编辑边界仍需专项测试。
- 原 File 菜单通过异步请求调用鸿蒙选择器，保留 Open / Place 等意图；导入持久沙箱副本，保存编码后向系统 URI 写入，成功后按原文档 ID 与捕获版本提交保存状态。真实新目标、原目标重复保存和多文档目标隔离已验收。
- 原 Load Files into Stack、Contact Sheet II、Statistics 输入字段可选择目录；目录复制、清单核验和原支持格式枚举完成后填回原表单，用户确认时执行原引擎。Stack/Contact Sheet/Statistics Median 的本地 provider 设备测试与结果外部重开通过，多文件输出另记。
- 平台服务复用上游导入／导出、原子保存、自动恢复与笔刷预设实现。
- 物理打印复用原 PDF/layout 算法，通过可重放的最小 [engine overlay](../vendor/photocraft-engine/README.md)
  接入异步系统任务；菜单、Actions、嵌套脚本及 Batch 共用同一 spool hook。系统受理
  不代表最终完成，未收到终态时保留持久 PDF。
- 偏好保存到 `<filesDir>/PhotoCraft/preferences.json`，恢复数据和预设分别在 `Recovery/` 和 `Presets/`。
- UI CJK 字体通过上游惰性加载插件发现 `/system/fonts` 中的 HarmonyOS/Noto 字体，缺字时逐帧读取。

字体模块必须在 `PhotocraftApp::setup_context` 前安装：egui 不会替换已注册的同类型插件。
目前该模块优先处理 UI CJK；文档文字引擎已接 HarmonyOS/Noto CJK fallback，并验证中文渲染与保存。完整系统字体目录、预编辑/取消与不同输入法仍需要专门验收。
文档 GPU compute 合成与 Vulkan OHOS surface 仍是后续专项，当前 GLES 展示不代表 `compose.wgsl` 已在设备通过。
原粘贴通过显式可见 PasteButton 授权，文字粘贴已测；图片剪贴板、多个原生视口和原生无障碍桥仍需专项工作。

## 验证

统一自动测试入口和真机隔离说明见 [自动测试](../ohos-notes/automated-testing.md)。
`./scripts/test.sh host` 执行本地回归；`./scripts/test.sh device --device <serial>` 构建、签名并运行真机测试。
运行真机测试前保存并退出普通 PhotoCraft；测试结果以生成的逐项报告为准。

从 OHOS workspace 执行相关测试和交叉编译检查：

```sh
cargo test -p craft-ohos-platform -p photocraft-ohos
cargo check --target aarch64-unknown-linux-ohos -p photocraft-ohos
```

上游测试、语料和原版 eframe UI 测试从 `upstream/` 独立执行。
本地 eframe 兼容接口只覆盖生产 UI 的应用回调，不包含原版桌面 runner 或 egui-kittest 的完整集成 API。
2026-10-07 已通过 OHOS workspace 检查、arm64 Rust release 构建和首批 15 个 host tests；2026-10-08 新增文件事务测试。数量和当前包结果统一记录在功能验收文档。
测试包含输入修饰键/重复/取消/失焦、headless 真实上游编辑器，以及 PNG 打开→Ctrl+I 反色→导出结果验证。
固定上游核心、格式、其他库与原版 UI 的默认测试合计 2986 通过、0 失败、27 忽略；显式 panic_hunt 另 1 通过。包 #8 平台批 67 通过，ArkTS 105 通过（目录输入 44 为子集）；后续源码批与设备包分别记录，重复回归不累加，可选语料仍另列。

| 设备验收项 | 当前状态 |
| --- | --- |
| API 26 PC 连接与调试 Profile 设备授权 | 已核验 |
| 真实 native unsigned/signed HAP | 首版及文件修复包已构建、签名、安装；最新平台包仍待逐项验收 |
| 实际 GLES adapter、编辑器首帧和截图 | Maleoon 916，已出帧并留存设备证据 |
| 窗口缩放、鼠标/触控板、键盘文本与快捷键 | 待设备实测 |
| PNG 打开/编辑/导出、自动恢复 | 正式菜单、实际格式重开与自动恢复通过，完整边界见验收记录 |
| 压感硬件数据、中文 IME | 基础中文候选/提交/持久重开通过；压感及输入法完整边界待专项验收 |

验收需记录真机 GPU 后端、出帧、缩放、输入、PNG 打开／编辑／保存与自动恢复结果。
执行状态见本目录的 [功能验收记录](photocraft-validation.md)。

## 许可证和发行资源

PhotoCraft 源码与本移植代码使用 MIT OR Apache-2.0；
上游第三方资源的署名和许可证见 `upstream/NOTICE`、`upstream/ATTRIBUTION.md` 和对应资源目录。
发行包应包含适用的许可证文本。

`upstream/docs/brand/` 的 ArtCraft 标记有单独条款。
修改版发行需删除或替换这些标记。当前壳采用未经修改的上游官方 512 × 512 像素
[应用图标](../upstream/assets/app-icon/hicolor/512x512/apps/ai.storyteller.photocraft.png) 作为应用图标；
该图标为上游项目所有者的原创作品，按其独立的
[MIT OR Apache-2.0 资源许可证](../upstream/assets/app-icon/LICENSE.txt) 使用。
可以用普通文字说明移植基于 PhotoCraft；上架前仍需核验最终打包资源和署名。
