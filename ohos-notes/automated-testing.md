# PhotoCraft 自动测试

统一入口在各应用的 `scripts/test.sh`。测试调用现有 Rust 应用和生产 ArkTS 文件桥，真机用例通过原菜单触发系统选择器。主机逻辑测试、合成输入、实际系统输入和真实文件提供方结果分别记录。

## 使用

在本工程目录执行：

```sh
./scripts/test.sh check
./scripts/test.sh host
./scripts/test.sh device --device 3BT0226106000273
./scripts/test.sh device --device 3BT0226106000273 --scope 'PhotoCraftCore#new_edit_undo_redo'
./scripts/test.sh device --device 3BT0226106000273 --repeat 3
./scripts/test.sh device --device 3BT0226106000273 --repeat 3 --manual-baseline-seconds 60
./scripts/test.sh device --device 3BT0226106000273 --reuse-build logs/tests/RUN_ID
./scripts/test.sh all --device 3BT0226106000273
```

运行前保存并退出普通 PhotoCraft。脚本遇到正在运行的普通应用即停止，不终止正在编辑的会话。设备必须解锁、允许 USB 调试，并具有本 bundle 的有效 Debug 签名。其他应用缺少独立签名或设备用例时会明确报告未执行。

## 执行与证据

`host` 使用 Cargo 的 host 缓存执行本应用及平台测试、Node 的生产 ArkTS 合同测试，以及生产 UTF-16 解码器的 C++ 测试。`device` 在单独构建目录生成应用和 `ohosTest` 包，复用私有签名锁，通过 `hdc -t <serial> shell aa test` 顺序执行用例。默认关闭覆盖率。

每次运行在 `logs/tests/<runId>/` 保存 JSON、JUnit 报告和命令输出。HDC 返回成功不代表用例通过：报告必须包含完成的测试结果；失败、错误、超时、零用例或缺少结束报告均判失败。跳过的用例不能算作已通过。真机失败证据包含本应用 PID/tag 的日志和测试界面布局、截图。

系统 UI 优先使用 UiTest SDK；设备上的 `Driver.create()` 不可用时，使用官方 `uitest` 命令行后端。命令行后端从当前应用或系统文件选择器的实时布局中精确匹配控件，再注入真实点击和按键；模糊匹配、重复匹配、不可见或越界控件会失败。报告记录实际后端。

API 26 PC 的测试进程不能直接读取 UiTest daemon 的布局文件。此时宿主通过官方 HDC 文件传输接收本次唯一查询文件，并用 `file send -b <bundle>` 回传到已经验证的隔离缓存。正文传输完成后才发布完成标记；应用核对 run ID、票据、随机 nonce、字节数及 SHA-256 后读取同一份字节，并清理本次回传目录。传输、解析或清理失败都会使测试失败。这个通道不增加网络权限或普通包接口；查询文件由宿主按本次 run ID 精确清理。参见官方 [HDC 文件传输](https://github.com/openharmony/docs/blob/master/zh-cn/application-dev/dfx/hdc.md)。

完整运行依次执行核心与文件用例、真实一分钟自动保存、停止测试进程后恢复验证、关闭期间取消与保存。设备用例串行运行；宿主 Node 测试默认并发 4。`--repeat 3` 只构建一次，每轮使用新目录并恢复普通包；只有三轮完整运行连续通过才记录完整设备验收。`--scope` 属于部分验证。复用构建时检查源代码指纹、bundle、测试构建标志及两份签名 HAP 的 SHA-256，过期或修改的产物会被拒绝。

测试包保留普通 bundle，使用独立目录：

```text
<filesDir>/PhotoCraftTestRuns/<runId>/files
<cacheDir>/PhotoCraftTestRuns/<runId>/cache
```

真实 `UIAbilityContext` 用于 picker、权限和窗口 API；存储根目录单独传给所有文件、目录、发布、剪贴板、打印和诊断模块。测试会话必须在初始化前确定，运行中不能切换。外部文件仅使用新建测试文件夹和唯一运行文件名。失败现场保留，清理不得触及其他运行或普通应用数据。

隔离验证同时覆盖应用级和 `entry` 模块级的 files/cache。API 26 真机上，从 ApplicationContext 创建的模块 Context 仍返回应用级路径；因此不能把二者当成模块数据证明。测试在启动前按官方沙箱布局检查模块目录，启动后要求它与真实 `EntryAbility.context` 及原生快照中的目录一致，路径变化会使测试失败。参见官方 [Context 目录说明](https://github.com/openharmony/docs/blob/master/zh-cn/application-dev/application-models/application-context-stage.md)。

隔离测试会话保留自身目录中的诊断和生命周期记录。测试标志与有效 run ID 同时满足时，编辑器初始化前关闭 HiAppEvent watcher 订阅，并调用官方 `hiAppEvent.configure({disable: true})`。该配置只影响当前进程内存，普通包不调用它；配置异常使启动失败。框架事件可能使用应用级 `hiappevent` 目录，因此仍以四目录前后指纹为准，不能仅凭禁用 API 返回成功认定隔离成立。[官方配置实现](https://github.com/openharmony/hiviewdfx_hiappevent/blob/master/frameworks/native/libhiappevent/hiappevent_config.cpp)

## 测试接口

Rust `device-tests` feature 默认关闭；C++ `PHOTOCRAFT_DEVICE_TESTS` 仅用于测试 Debug 构建；ArkTS `BuildProfile.DEVICE_TESTS` 默认 `false`。普通 Debug 和 Release 包不注册测试 NAPI。测试 archive 与正常 archive 分开保存。

测试接口是原有 `ControlRequest` 的进程内传输：提交返回票据，查询返回完成结果，快照不消费生产文件/输入队列。渲染线程在正常帧中处理原菜单、对话框和输入；主线程和 native 锁不能等待文件选择器或控制响应。

文件测试不允许用 `app.open`/`app.save` 或路径参数绕过 picker，也不能提交伪造文件完成回执。编码、发布事务以及成功/取消回执仍走生产流程。

## 验收边界

单元测试主要验证核心状态、编码字节、取消/失败、归属及隔离；设备测试验证真实 NAPI/渲染、菜单到 picker 的文件流程、输入和生命周期。进程恢复采用宿主分阶段启动，保留同一测试 run 的数据。

完整系统中文候选交互、实体笔压和全部 egui 无障碍控件仍需专项验收。普通 ArkUI 选择器不能直接定位当前 XComponent 内的 egui 控件；测试复用原程序化菜单和对话框控制，系统 UI 使用 UiTest。

官方 UiTest 的非 ASCII 文本注入使用剪贴板及真实 Ctrl+V。中文用例区分合成 Type 输入与生产桥的实际中文/emoji 粘贴，并记录直接读取或 PasteButton 授权的实际路径，断言完整文本及原生按键计数。UiTest 在开发者模式可能有专用读取放行，不能证明物理快捷键授权或直接系统 IME 提交；两者仍需人工验收。UTF-16 解码逻辑另由宿主 C++ 测试覆盖。[UiTest 输入实现](https://github.com/openharmony/testfwk_arkxtest/blob/master/uitest/core/ui_driver.cpp#L620)

验收要求是本地回归通过、首批设备用例连续三次通过，并核对正常数据未被测试修改。实际执行结果另附本次验证记录；本说明本身不是已通过证明。

人工操作的 60 秒是用户估计，尚未按相同用例范围计时。报告分别比较构建、自动执行和整体耗时；完整自动套件含真实一分钟自动保存等待及多次冷启动，比较时需保留这一覆盖范围差异。

## 官方依据

- [Instrument Test](https://developer.huawei.com/consumer/cn/doc/harmonyos-guides/ide-instrument-test)：测试模块、Native Node-API 设备测试与命令行执行。
- [aa 工具](https://github.com/openharmony/docs/blob/master/zh-cn/application-dev/tools/aa-tool.md)：Debug 签名、测试参数及等待时间。
- [JsUnit](https://github.com/openharmony/docs/blob/master/zh-cn/application-dev/application-test/unittest-guidelines.md)：Hypium、用例筛选和报告协议。
- [UiTest](https://github.com/openharmony/docs/blob/master/zh-cn/application-dev/application-test/uitest-guidelines.md)：系统界面定位、输入、布局和截图。
