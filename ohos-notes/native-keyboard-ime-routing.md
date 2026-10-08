# Native keyboard routing and IME attachment

Reviewed against the installed DevEco SDK 26 and official OpenHarmony sources on 2026-10-08. These findings concern source behavior and compilation. Package #5 failed attachment; package #6 includes the callback fix and attaches successfully on the device, but exposed a repeated context-identity reset described below. Physical Chinese composition and candidate selection have not been accepted on the device.

## Stage routes hardware keys to the IME

The normal Stage window route is:

1. `PreNotifyKeyEvent` calls `UIContent::ProcessKeyEvent(event, true)` for the ArkUI `onKeyPreIme` phase.
2. For keyboard events, the window calls its internal `InputMethodController::DispatchKeyEvent`.
3. If the IME consumes the event, the window does not dispatch it to ArkUI again.
4. Otherwise, normal `UIContent::ProcessKeyEvent` reaches the focused XComponent's native key callback. Its `handled` result controls subsequent ArkUI bubbling.

The XComponent installs its callback with `SetOnKeyEventInternal`, rather than `SetOnKeyPreIme`. `FocusEventHandler::OnKeyPreIme` returns false for an ordinary XComponent without a user pre-IME handler; only UIExtension, EmbeddedComponent, and DynamicComponent have a special internal-handler fallback in that phase. PhotoCraft has no ArkTS `onKeyPreIme` handler. Its native callback returning true therefore does not prevent hardware keys from first reaching the IME.

Primary sources, pinned to the reviewed revisions:

- [Window input channel](https://github.com/openharmony/window_window_manager/blob/aab11772a5e63db9495815dda606ee8d7b37dbdc/wm/src/window_input_channel.cpp#L41): consumed events stop here; `HandleKeyEvent` calls pre-IME then the input method at lines 87–115.
- [Stage window session](https://github.com/openharmony/window_window_manager/blob/aab11772a5e63db9495815dda606ee8d7b37dbdc/wm/src/window_session_impl.cpp#L8658): `NotifyKeyEvent` also dispatches to the IME before its normal callback; `PreNotifyKeyEvent` at line 1081 explicitly sets the pre-IME flag.
- [ArkUI focus handler](https://github.com/openharmony/arkui_ace_engine/blob/515fe7b84e39eb8fa83fc45a0712294f879c81ab/frameworks/core/components_ng/event/focus_event_handler.cpp#L198): pre-IME and normal internal key handlers are separate; the pre-IME fallback is at lines 278–305.
- [XComponent focus callback](https://github.com/openharmony/arkui_ace_engine/blob/515fe7b84e39eb8fa83fc45a0712294f879c81ab/frameworks/core/components_ng/pattern/xcomponent/xcomponent_pattern.cpp): `InitFocusEvent` registers `SetOnKeyEventInternal`; `HandleKeyEvent` returns the native callback result.
- [Native custom-editor guide](https://github.com/openharmony/docs/blob/f41b9345badd47c7ab0c263344cd7f4b5a549afb/zh-cn/application-dev/inputmethod/use-inputmethod-in-custom-edit-box-ndk.md): attach, register editing callbacks, and detach on blur. It adds no application hardware-key forwarding bridge.

The installed public `inputmethod/*.h` and its [InputMethodProxy implementation](https://github.com/openharmony/inputmethod_imf/blob/7777cba9eb19085af8e7b90088531028d2937460/frameworks/ndk/src/inputmethod_inputmethod_proxy_capi.cpp) contain no `SendKeyEvent` or `DispatchKeyEvent` C API. The C++ controller function used by the window is an internal platform API, not an application SDK dependency. PhotoCraft should not call it a second time.

## Printable fallback correction

`native_bridge.cpp::DispatchKey` previously generated ASCII only when `!ImeAttached()`. That discarded printable events an attached IME deliberately passed through, including an English mode that reports the event unconsumed. The callback now generates the existing US ASCII mapping for pressed keys without Ctrl/Alt regardless of attachment. Native events delivered here have already passed through the system IME stage. Consumed Chinese composition events do not reach this fallback.

Physical key codes and modifier state still reach Rust, including shortcut keys; this change does not add a new ABI or an extra key dispatch. Non-US layout conversion remains a separate limitation of the existing ASCII fallback. The legacy XComponent key getter API does not expose layout text in the installed SDK. IME callbacks provide actual committed Unicode text.

`OH_TextConfig_SetConsumeKeyEvents` is a public API 26 addition. The SDK describes an editor capability and defaults it to false. [Config construction](https://github.com/openharmony/inputmethod_imf/blob/7777cba9eb19085af8e7b90088531028d2937460/frameworks/ndk/src/inputmethod_controller_capi.cpp#L152) copies it into `InputAttribute`, which is exposed to the IME. The reviewed framework does not use it to gate the window's dispatch sequence. It is not a replacement for a missing key-forwarding call. PhotoCraft leaves that new capability at its documented default; adding a strong API 26 import would also break the application's API 20 compatibility floor.

## Attachment validation correction

The official [CAPI validator](https://github.com/openharmony/inputmethod_imf/blob/7777cba9eb19085af8e7b90088531028d2937460/frameworks/ndk/src/inputmethod_controller_capi.cpp#L102) requires all fifteen proxy callbacks, including `receivePrivateCommandFunc`. The old bridge registered fourteen. `AttachWithUIContext` returned `IME_ERR_NULL_POINTER` (12802000) before requesting `GetTextConfig`. A no-throw private-command callback is now registered; it rejects unsupported private protocols without inspecting or retaining their payload. Registration was checked against all fifteen validator members.

`GetTextConfig` also initializes the empty placeholder/ability strings and zero text-avoid geometry explicitly. The framework constructs a stack `InputMethod_TextConfig`, and its native struct leaves some of those buffers/geometry fields without full value initialization. The bridge uses APIs at or below its API 20 compatibility floor for these defaults. The subsequent context correction uses public API 12 `Attach` with this config's real window ID, as described below.

## Stable context identity correction after package #6

The device reported successful attachment repeatedly at the 200 ms platform-poll interval. The bridge converted every `UIContext` to an `ArkUI_ContextHandle` and compared pointer addresses. Official [OH_ArkUI_GetContextFromNapiValue](https://github.com/openharmony/arkui_ace_engine/blob/515fe7b84e39eb8fa83fc45a0712294f879c81ab/interfaces/native/node/native_node_napi.cpp#L167) allocates `new ArkUI_Context({ .id = instanceId })` on every successful call. Those addresses therefore describe conversion wrappers, not stable UI instances. Pointer inequality forced detach/attach on each poll and discarded active composition. The old code also abandoned every wrapper.

The installed SDK declares that context type opaque and exposes no corresponding context disposer. PhotoCraft does not copy its private layout, guess a deallocator, or retain an unbounded set of wrappers. It now avoids this conversion entirely:

- When available, the bridge calls the public `UIContext.getId()` through NAPI. SDK 26's `@ohos.arkui.UIContext.d.ts` documents this API since 22 as the stable unique UI instance ID. A present but invalid/ambiguous ID is rejected.
- API 20/21 have no such method. The bridge checks availability and falls back to a strong NAPI object reference, compared with `napi_strict_equals`, together with the actual window ID. It deletes the reference and clears environment/identity state on detach, including focus loss, surface loss, release, and attachment cleanup.
- The service attaches using API 12 `OH_InputMethodController_Attach`. [Its implementation](https://github.com/openharmony/inputmethod_imf/blob/7777cba9eb19085af8e7b90088531028d2937460/frameworks/ndk/src/inputmethod_controller_capi.cpp#L226) reads the real window ID from `GetTextConfig` and uses the same `PerformAttach` path as `AttachWithUIContext`. The latter's additional operation is deriving and overriding that window ID from the wrapper's internal instance ID; it does not require the service to retain the wrapper.

Reattachment is limited to initial attach, a changed target generation, NAPI environment, UI instance/object, actual window ID, or an `interrupt` false→true edge. Text, selection, composition and cursor geometry notifications keep the existing attachment. A newly created JavaScript wrapper for the same public instance ID does not reset it. A held true interrupt does not retrigger on every poll.

The corrected code is frozen after SDK 26 strict syntax/PIC compilation and nineteen host contract checks using the actual production function bodies with a fake public NAPI runtime. These cover one hundred stable polls, genuine context/target/window changes, both interrupt edges, the older-API reference fallback, reference release, repeated release, and rejection of an ambiguous context. Evidence is under `logs/build/ime-context-fix-20261008`. They do not simulate the system IME or establish device composition success. Package #6's immutable HAP remains unchanged; a subsequent package must include this correction before candidate/preedit acceptance testing.

## Password purpose

The installed C enum has no separate ordinary `PASSWORD` member. Its `IME_TEXT_INPUT_TYPE_VISIBLE_PASSWORD = 7` maps directly to framework `InputAttribute::PATTERN_PASSWORD = 0x07`. [InputAttribute::GetSecurityFlag](https://github.com/openharmony/inputmethod_imf/blob/7777cba9eb19085af8e7b90088531028d2937460/frameworks/native/inputmethod_controller/include/input_attribute.h#L132) treats 7, number-password 8, lock-password 9, and new-password 11 as password types. PhotoCraft's `purpose == "password"` mapping to the API 12 visible-password enum is therefore retained. This review used no user password and logged no input contents.

## Verification and device checks

Both C++ files pass SDK 26 BiSheng `-std=c++17 -Wall -Wextra -Werror` syntax checks and AArch64 PIC object compilation. Callback/fallback evidence is saved under `logs/build/ime-attach-fix-20261008`; the later context correction is checked separately under `logs/build/ime-context-fix-20261008`. The context correction did not build, sign, install, or modify the immutable package #6.

After the next package is installed, verify with a real hardware keyboard or a device key-injection method that produces physical key events:

- Focus a document Type tool and an egui text field separately; confirm attach succeeds without 12802000.
- With Chinese IME active, type pinyin, change/cancel the preview, choose a candidate, and verify exactly one Unicode commit. Check candidate geometry and deletion around selection.
- Switch to English input while the IME stays attached; type letters, shifted punctuation, and numpad digits. Each printable event should appear once, including IME passthrough events.
- Exercise arrows, Backspace/Delete, Ctrl+A/C/V, and application shortcuts; confirm no doubled navigation or editing.
- Open a picker or switch applications, then return. Check detach/reattach, no stale composition callback, and no stuck modifiers.

`UITest uiInput text` has been observed to use clipboard plus Ctrl+V on this PC; that behavior does not establish that native preedit/commit works. Screen captures or logs for this validation should use synthetic text and avoid user clipboard contents.


## Simulated key injection audit after package 6b

The immutable package 6b fixes context reattachment and starts on the device. The parent task observed synthetic letters appearing after UITest key injection without a candidate window. That observation alone cannot distinguish an IME in English mode, an IME declining an event, a noneditable/agent dispatch error, injected event metadata differences, or a candidate/composition issue. Chinese IME input is still not accepted.

Official UITest `SysUiController::InjectKeyEventSequence` constructs MMI KeyEvent objects and calls `InputManager::SimulateInputEvent`. `uitest uiInput keyEvent` uses this path. `uinput -K` constructs the same event type and calls the same manager method. Neither reviewed caller sets an IME bypass flag. MMI `ServerMsgHandler::OnInjectKeyEvent` routes the injected key through its normal event normalization handler. `WindowInputChannel::IsKeyboardEvent` classifies keyboard events by key code, including letters, rather than physical-vs-simulated provenance, and sends them to IMC. Its post-IME callback returns immediately if consumed. The Stage session's second dispatch sets `notifyInputMethod=false` because IMC already processed the first window stage, not because the key was simulated.

Primary source revisions and locations:

- [UITest controller](https://github.com/openharmony/testfwk_arkxtest/blob/a9a8b4700b0b79ee0057f36de6e351d7e647a656/uitest/server/system_ui_controller.cpp#L895), lines 895–935; [CLI key input](https://github.com/openharmony/testfwk_arkxtest/blob/a9a8b4700b0b79ee0057f36de6e351d7e647a656/uitest/input/ui_input.cpp#L239).
- [uinput keyboard command](https://github.com/openharmony/multimodalinput_input/blob/18c19ae336aacdb17193383bef64a91151cf7131/tools/inject_event/src/input_manager_command.cpp#L813), lines 813–903.
- [MMI injection manager](https://github.com/openharmony/multimodalinput_input/blob/18c19ae336aacdb17193383bef64a91151cf7131/frameworks/proxy/event_handler/src/input_manager_impl.cpp#L1331), lines 1331–1355; [injection server](https://github.com/openharmony/multimodalinput_input/blob/18c19ae336aacdb17193383bef64a91151cf7131/service/message_handle/src/server_msg_handler.cpp#L150), lines 150–174.
- [Window key handling](https://github.com/openharmony/window_window_manager/blob/aab11772a5e63db9495815dda606ee8d7b37dbdc/wm/src/window_input_channel.cpp#L61), lines 61–120 and 216–226; [Stage session](https://github.com/openharmony/window_window_manager/blob/aab11772a5e63db9495815dda606ee8d7b37dbdc/wm/src/window_session_impl.cpp#L1064), lines 1064–1087 and 8658–8712.

The injections do differ in metadata: UITest sets keycode/pressed state and target display but does not explicitly fill key-item Unicode or down time. uinput's `-K` path fills both; its reviewed `KeyCodeToUnicode` returns a static mapping's `transitioned` character. Thus uinput is also a simulated event and cannot be assumed identical to a real keyboard. Whether these differences matter to this device's commercial IME remains a hypothesis requiring device comparison.

PhotoCraft's ordinary Type field maps to `IME_TEXT_INPUT_TYPE_MULTILINE=1`. The CAPI copies that value unchanged to `InputAttribute.inputPattern`; the framework's own ordinary text constant is 1. The bridge enables preview support and sets the SDK TextConfig window ID from the current main window's public properties ID. These source values support ordinary text entry, but only captured attachment/configuration and actual callback activity can verify which window and editor the live service considers editable. The `consumeKeyEvents` capability defaults false in public ArkTS and CAPI config; reviewed IMC dispatch does not test that capability to decide whether keys reach the agent. Commercial IME behavior cannot be inferred from that source alone, and blindly setting true would add an API 26-only dependency without an established fix.

A proposed diagnostic change, not yet applied by this audit, would log only event kind/state: attach reason categories, stable attachment retention, window/input-type configuration, IME callback kind and whether it is current/queued, plus post-IME Native key action/attachment/fallback booleans. Per-kind and global limits would prevent repeating logs. It would omit key codes, text, target tokens, selection positions, lengths and private-command contents. Existing first preedit/commit logs prove only callback receipt when they appear; absent buffered logs do not prove that the callback did not run. Source snapshots and hashes are in `logs/build/ime-injection-audit-20261008`; the audit changed no Native code, Rust, ArkTS, HAP or device state.


## Bounded Native diagnostics for the next package

Device update, package #7: the probes were packaged and installed. The actual system toolbar first
showed English; explicitly switching its language control to the device's existing double-pinyin
mode produced a `ni` candidate list beside the original Type editor. Selecting the first candidate
committed one `你`, which persisted through PSD save, close and external system-picker reopen.
The bounded log also established a valid TextConfig, one stable attach and a current queued insert
callback. Basic candidate/commit/persistence has passed; hardware keyboard equivalence, canvas
preedit updates, cancellation/deletion and other IMEs still require separate device evidence.
Details and the exact package hash are in [functional acceptance](../docs/photocraft-validation.md).
Earlier unaccepted observations below retain their historical scope.

The parent task's live package 6b log showed only one attachment (`window=1417`, `instance=100000`) while typing through both simulated-key routes. Stable context attachment has therefore passed that device observation; Chinese candidate/preedit/commit remains unaccepted. The log evidence is `logs/filefix/package6b-ime-live-hilog.txt`.

The previously proposed probes are now implemented and source-frozen in Native code, awaiting the next Rust archive and package. Their prefix is `IME diag`. The budget is shared across the process: at most 64 probe lines, four lines per IME callback/service kind, sixteen post-IME key lines, and one stable-attachment line. Atomic saturating counters remain at their limits under concurrency. Budgets do not reset on focus changes or reattachment, avoiding indefinite typing or per-frame logs.

All fifteen required IME callbacks have a no-throw diagnostic scope. It records only the callback kind, whether the received proxy matches the current proxy, and queue result. `queue=-1` means no queue attempt, `0` means at least one attempted enqueue was rejected, and `1` means every attempted enqueue was admitted. A queue result reports only Rust queue admission; it does not report worker application or successful text editing. Nested synchronous callback scopes restore their previous thread-local recorder. Configuration adds input type, actual window ID and preview-support state. Private-command payloads remain unread and unsupported commands still return -1.

Attach/detach reason bits are application-owned diagnostic categories: initial 1, target change 2, window change 4, context change 8, interrupt rising 16, no editor 32, lost focus 64, lost surface 128, explicit release 256, and attach failure 512. Changed target contents are never logged. A stable poll records once that it kept the existing attachment. Detach probes occur only if a service/editor handle existed, so idle frames consume no budget. Post-IME key probes contain only action down/up, attachment boolean and text-fallback boolean. The old keycode/modifier/caps probe and unbounded attachment-success log were removed; the former first-preedit/commit lines now use the shared bounded callback recorder.

No probe contains key codes, entered text, private data, target tokens, selection positions or text lengths. The probes do not change text processing, task queue semantics or SDK configuration. SDK 26 strict syntax and AArch64 PIC object compilation pass for both Native files. A contract compiled from the production diagnostic bodies and fake log/queue capture passes 117 assertions, including concurrent caps, exact content-free messages, stale proxies, nested scopes, rejection aggregation, private payload isolation and actual 64-line emission. The original nineteen context contracts also pass again, with the relevant production function bodies unchanged. These are Native contract checks, not additional Rust tests or system IME acceptance. Persistent evidence is `logs/build/ime-diagnostics-20261008/verification.json` and its strict/contract logs. No HAP or device action was performed for this change.
