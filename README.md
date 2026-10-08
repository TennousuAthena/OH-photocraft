# PhotoCraft for HarmonyOS

PhotoCraft 的鸿蒙 PC 原生移植，保留上游 Rust 图像引擎与 egui 编辑器，通过 ArkTS、XComponent 和原生桥接入鸿蒙窗口、输入与文件服务。

移植作者：[TennousuAthena](https://github.com/TennousuAthena) · 移植仓库：[OH-photocraft](https://github.com/TennousuAthena/OH-photocraft) · 原项目：[storytold/photocraft](https://github.com/storytold/photocraft)

## 功能与状态

- 原生 arm64 应用，使用 GLES 展示编辑器，上游 CPU 合成器处理文档图像。
- 接入系统文件选择器，支持原 Open、Open As、Save、Save As 和 Save a Copy 流程，以及 PNG、JPEG、PSD、pcraft 等原有格式。
- 接入键盘、鼠标、触控、中文输入、可见粘贴授权、系统 PDF 打印、自动恢复和目录工作流。
- 增加简体中文、繁体中文及鸿蒙壳层的对应资源。默认自动跟随系统语言，可在首选项中手动选择并保留。
- “关于 PhotoCraft”中包含鸿蒙移植作者与仓库入口。

已在 API 26 的 MateBook Pro 上验证原生首帧、基础编辑、文件打开与保存、部分目录流程、中文候选提交、文字粘贴及打印终态。完整专业工作流仍在验收；系统语言动态切换与中文布局、完整输入法边界、笔压、性能及 GPU 文档合成仍需实机验证。主机测试与构建通过不代表这些设备项目已通过。

当前实现细节见 [工程说明](docs/porting.md)，逐批功能结论与待测项目见 [验收记录](docs/photocraft-validation.md)。

## 获取源码

上游源码作为固定提交的 Git 子模块保留，构建前必须初始化：

```sh
git clone --recurse-submodules https://github.com/TennousuAthena/OH-photocraft.git
cd OH-photocraft
```

已有普通克隆可执行：

```sh
git submodule update --init --recursive
```

上游版本、来源与 overlay 说明见 [UPSTREAM.md](UPSTREAM.md)。

## 构建

需要 Rust 1.95 或更高版本、`aarch64-unknown-linux-ohos` target，以及包含 native 工具链的 DevEco SDK。当前脚本以 macOS 的 DevEco Studio 安装为默认环境。

```sh
rustup target add aarch64-unknown-linux-ohos
./scripts/doctor.sh
./scripts/build-ohos.sh
```

脚本默认查找 `/Applications/DevEco-Studio.app/Contents`。可通过 `DEVECO_STUDIO_CONTENTS`、`DEVECO_SDK_HOME` 或 `OHOS_SDK` 指定安装位置；`OHOS_SDK` 应指向包含 `llvm/` 和 `sysroot/` 的 native SDK 目录。

未签名 HAP 输出为 `harmonyos/entry/build/default/outputs/default/entry-default-unsigned.hap`。可用 `--rust-only` 或 `--hap-only` 分阶段构建；后者需要已经生成的 Rust 静态库。

设备安装需要自己的开发者账号和调试签名。使用 DevEco Studio 打开 `harmonyos/`，为 `moe.kiwi.photocraft` 与目标设备生成签名材料，保存在本地 `.signing/` 后执行：

```sh
./scripts/build-ohos.sh --signed
```

签名配置格式与操作步骤见 [本机调试签名](docs/porting.md#本机调试签名)。私有签名、缓存、构建产物、设备日志和截图均不随 Git 仓库发布。

## 测试

```sh
./scripts/test.sh check
./scripts/test.sh host
python3 scripts/check-localization.py
```

设备测试需要有效签名与已解锁的调试设备；运行前应保存并退出普通 PhotoCraft：

```sh
./scripts/test.sh device --device <serial>
```

测试隔离、报告和验收边界见 [自动测试说明](ohos-notes/automated-testing.md)。测试生成的报告保存在本地 `logs/tests/`。

## 工程结构

| 路径 | 用途 |
| --- | --- |
| `upstream/` | 固定版本的原项目 Git 子模块 |
| `apps/` | Rust 平台适配、生命周期与 FFI |
| `vendor/` | eframe 兼容接口及 PhotoCraft UI/engine 的鸿蒙 overlay |
| `harmonyos/` | Stage 工程、ArkTS 壳与 C++ NAPI/XComponent 桥 |
| `scripts/`、`tests/` | 构建与自动测试 |
| `docs/`、`ohos-notes/` | 工程说明、验收历史与平台协议 |

## 许可证

源码采用 [MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE)，保留原作者的版权与声明。第三方字体、图标和词典按各自许可证使用；完整来源见 [ATTRIBUTION.md](ATTRIBUTION.md) 与 [NOTICE](NOTICE)。
