# PDF printing on HarmonyOS PC

Status: **SDK26 integration, real system preview and a system Print to PDF task completed event passed on package #7; preview-dismiss cancellation remains unavailable through the current files API, and physical printer completion is unverified.** Updated on 2026-10-08. The original Rust Print/Print One Copy requests dispatch rendered PDF files to the system helper. The initial integration used an isolated temporary ArkTS project; root subsequently packaged, signed and tested it on the PC.

Package #7 device evidence: original Print Settings with Scale to Fit rendered the CJK/layered test
document into the system preview. Starting its explicit Print to PDF target, choosing the synthetic
output name and completing system publication returned `System print job completed` in the original
footer while preserving the PSD name and clean state. Engine PDF output also published through the
ordinary Save picker. Exact hashes, steps and evidence are in [functional acceptance](../docs/photocraft-validation.md).
This terminal PDF job does not turn a preview X into cancellation or prove a physical printer output.

## Normal application entry point

The installed public SDK exposes `print` through `@kit.BasicServicesKit`, rather than a separate `@kit.PrintKit` module:

```ts
import { print } from '@kit.BasicServicesKit';
import { fileUri } from '@kit.CoreFileKit';

const task = await print.print([fileUri.getUriFromPath(sandboxPdfPath)], context);
```

`context` is the current `common.UIAbilityContext`, used to start the system print UI. The public `print(files: Array<string>, context: Context): Promise<PrintTask>` overload is available since API 11, well below the application's API 20 compatibility floor. SDK 26 deprecates the older overload without a context. PDF is an explicitly supported input; the SDK requires storing input in the application sandbox and deriving its URI with `fileUri.getUriFromPath`.

The required `ohos.permission.PRINT` is **normal / system_grant**, available since API 10. A normal app can declare it in `module.json5`; no user authorization prompt or special signing ACL is required by this permission definition. Runtime errors, missing `SystemCapability.Print.PrintFramework`, and device-specific print service behavior remain possible. This permission is independent of the restricted clipboard permission.

Primary local declarations:

- `/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/ets/api/@ohos.print.d.ts` — `PrintTask` at line 34, context overload at line 264, document adapter at line 159.
- `/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/ets/kits/@kit.BasicServicesKit.d.ts` — `print` import/export.

Public references: [print API](https://github.com/openharmony/docs/blob/master/zh-cn/application-dev/reference/apis-basic-services-kit/js-apis-print.md), [permission definitions](https://github.com/openharmony/docs/blob/master/zh-cn/application-dev/security/AccessToken/permissions-for-all.md#ohospermissionprint), [Huawei normal permission list](https://developer.huawei.com/consumer/cn/doc/harmonyos-guides-v5/permissions-for-all-V5).

## Submission, terminal state, and cancellation

Returning a `PrintTask` means the print request has been accepted by the framework. It is not proof that output has been printed. In the official NAPI source, `CreatePrintTask` constructs the wrapper, calls `PrintTask::Start`, and returns the wrapper; `Start` invokes the spooler and `PrintManagerClient::StartPrint` separately from later event subscriptions.

Subscribe to the actual task:

| SDK event | Meaning in the helper | Terminal |
| --- | --- | --- |
| `block` | `blocked`; the task can resume | No |
| `succeed` | `succeeded`; framework job completion | Yes |
| `fail` | `failed`; framework job failure | Yes |
| `cancel` | `cancelled`; framework job cancellation | Yes |

Each event has its own `on` and `off` overload. Preserve the exact callback reference when unsubscribing. `PrintTask` exposes no public `cancel`, `stop`, `dispose`, job ID, or task status query. `off` only removes a listener. The private C++ `Stop` function is also not a public application cancel API; its current implementation clears the local task ID.

The user can operate cancellation in the system print interface. Do not treat returning to the application, preview dismissal, a timeout, or missing callbacks as a cancelled or successful task. The public SDK contains an `ApplicationEvent` enum, but no corresponding normal-app event subscription in this declaration; its existence alone does not provide a preview-close callback. The adapter's `PREVIEW_DESTROY` is a separate preview state, not successful printing. Physical paper output has not been tested here.

Source evidence: [NAPI task creation](https://github.com/openharmony/print_print_fwk/blob/7268b481b30e0a1f2b26015e92225a619b167b8d/interfaces/kits/napi/print_napi/src/napi_print_task.cpp), [task start and subscriptions](https://github.com/openharmony/print_print_fwk/blob/7268b481b30e0a1f2b26015e92225a619b167b8d/interfaces/kits/napi/print_napi/src/print_task.cpp), [modal UI lifecycle](https://github.com/openharmony/print_print_fwk/blob/7268b481b30e0a1f2b26015e92225a619b167b8d/interfaces/kits/napi/print_napi/src/print_modal_ui_callback.cpp).

## Re-layout adapter and Print-to-PDF

The alternate API 11 overload accepts `jobName`, `PrintDocumentAdapter`, `PrintAttributes`, and the context. Its `onStartLayoutWrite(jobId, oldAttrs, newAttrs, fd, writeResultCallback)` provides an empty PDF descriptor. The app must render according to the newly selected attributes and report file creation through the callback. That callback confirms PDF creation, not final job success. `onJobStateChanged` reports preview and print states separately.

For an already rendered PDF, the files overload is the smaller valid integration. Simply copying an old PDF into the adapter descriptor would not implement re-layout for new paper size/margins. The adapter should be added only if the original application's print renderer supports those attributes.

The installed normal SDK has no `printToPdf`, destination picker, or returned output-PDF path in these APIs. Supporting PDF input does not establish the existence of a virtual PDF printer on this PC's system print UI. The reviewed public spooler repository is not evidence for a particular commercial device's UI; no virtual printer name, numeric printer ID, or saved path is inferred from it.

The reliable application-controlled PDF route remains its original PDF renderer/export followed by `DocumentViewPicker` and the existing URI publication transaction. An optional system “save as PDF” choice, if offered on the device, must be checked on that device and must not be reported as an exported file without an actual file/result contract.

## Application integration and PDF ownership

`harmonyos/entry/src/main/ets/platform/PrintingBridge.ets` is imported by `PlatformBridge.ets`. Its API is:

```ts
new PrintingBridge(context, (update: PrintingUpdate) => { /* caller handles status */ });
submitPdf(id: number, pdfPath: string): Promise<boolean>;
dispose(): void;
```

Updates have `{ id, state, terminal, error }`. `submitted`, `blocked`, and `monitoringError` are nonterminal. `rejected` records failure before obtaining a task handle. Only actual `succeed`, `fail`, and `cancel` callbacks provide final task results. `submitPdf(true)` says only that a task was submitted, including the case where subsequent listener registration failed. Duplicate live IDs are rejected without emitting a terminal update for the original task. Submission is bounded to eight outstanding records.

The caller owns the immutable sandbox PDF and must keep it available while the task is outstanding. The helper neither copies nor deletes it. `dispose` detaches only this helper's callbacks and makes no assertion about outstanding jobs; disposal cannot be used as proof that it is safe to delete every staged input. No timeout fabricates a final result. Observer exceptions are contained within callback dispatch.

The frozen platform protocol sends `{kind:'printRequest', id, pdfPath}` and receives `{kind:'printingUpdate', id, state, terminal, error}`. Printing requests never send a generic `platformComplete` success acknowledgement. Rust validates each state and its terminal flag; only a known terminal task or submission rejection consumes and cleans its staged PDF. System-operation errors are also shown in the original editor UI. Page disposal only detaches this helper's listeners, so outstanding or indeterminate PDFs remain durable.

The original print dialog's explicit PDF output field uses its renderer and the existing fixed-format PDF file publication request instead of a system print job. That route preserves the working document's path, dirty flag, and revision through success, cancellation, and errors. It does not depend on a system virtual printer.

The actual application ArkTS sources and manifest compiled in a separate temporary SDK 26 project (`default@CompileArkTS` 2.075 s; complete isolated build 3.580 s). The manifest declares normal PRINT and FILE_ACCESS_PERSIST, with no READ_PASTEBOARD declaration. The independent helper compile and this integration check do not establish device installation, system preview/cancel callbacks, printer success/failure/block, or optional virtual PDF output. Those remain device acceptance cases.


## Device preview dismissal audit, 2026-10-08

Package 6 device verification displayed the rendered text and shapes correctly in the system print preview, including an offered Print-to-PDF choice. Closing the system preview with its upper-right X returned to PhotoCraft without a PrintTask cancellation callback; the application still showed its nonterminal submitted status after eight seconds. This confirms preview rendering and a missing final callback for that particular operation. It does not establish that a print job was submitted to a printer, cancelled, completed, or that a PDF destination was written. Device evidence is recorded by the parent task under `package6-system-print-cancelled`.

The current helper registers all four documented events on the returned task, keeps that task alive, and removes subscriptions only after a terminal task event or page disposal. `PrintTask::Start` assigns its generated job ID before returning the wrapper. `PrintTask::On` subscribes using that ID. The public reference also demonstrates subscription after the print promise resolves, matching the helper. The source has no replay of terminal events on subscription, so an extremely early event can race any public registration; that does not explain a later eight-second dismissal by itself. Native registration failures can also be logged without throwing in the reviewed NAPI implementation, so successful `on()` return is not a public acknowledgement that a service listener exists.

The official framework distinguishes task cancellation from closing its preview:

| Source at framework commit `7268b481b30e0a1f2b26015e92225a619b167b8d` | Actual behavior |
| --- | --- |
| `services/print_service/src/print_service_ability.cpp`, `GetPrintJobStateInfo`, lines 3689–3718 | A `cancel` event requires a completed print job with the cancelled substate. |
| Same file, `NotifyPrintService`, lines 3848–3874 | Preview closing uses a spooler-closed state with separate started/cancelled substates. |
| Same file, `UnregisterPrintTaskCallback`, lines 5557–5582 | Preview cancellation schedules removal of the four PrintTask listeners. It does not emit `cancel`. |
| Same file, `GetListeningState`, lines 3919–3960 | The adapter receives different internal states for preview closed by Cancel and by Start. |
| `frameworks/innerkitsimpl/print_impl/src/print_callback.cpp`, lines 197–222 and 375–391 | The internal adapter state is passed as an unsigned number directly to ArkTS, without conversion to a public preview-cancel enum. |
| `utils/include/print_constant.h`, lines 242–248 | Internal preview states 5 and 6 distinguish Cancel and Start. |

These are [framework source](https://github.com/openharmony/print_print_fwk/blob/7268b481b30e0a1f2b26015e92225a619b167b8d/services/print_service/src/print_service_ability.cpp), [callback conversion](https://github.com/openharmony/print_print_fwk/blob/7268b481b30e0a1f2b26015e92225a619b167b8d/frameworks/innerkitsimpl/print_impl/src/print_callback.cpp), and [internal enum definitions](https://github.com/openharmony/print_print_fwk/blob/7268b481b30e0a1f2b26015e92225a619b167b8d/utils/include/print_constant.h). They explain the observed behavior, but commercial HarmonyOS device implementation details may differ.

The installed normal SDK 26 exposes only `PrintDocumentAdapterState.PREVIEW_DESTROY = 0` (described as preview failure), `PRINT_TASK_SUCCEED`, `PRINT_TASK_FAIL`, `PRINT_TASK_CANCEL`, and `PRINT_TASK_BLOCK`. It does not expose the internal preview states 5/6. The official public SDK declaration, last print-file commit `2891501f089a64a8bb5a2469618083119c8e7a12`, likewise exposes only the five documented values. `ApplicationEvent` is an enum, while `notifyPrintService` and `notifyPrintServiceEvent` are system-only operations protected by MANAGE_PRINT_JOB, not normal-app listeners.

Consequently, switching to a public document adapter only to detect this preview cancellation does not establish a supported solution. Do not create private numeric enum aliases, treat PREVIEW_DESTROY as a task cancellation, or infer cancellation from foreground transitions. An adapter may later support attribute-aware re-layout and report documented task states, but unknown adapter states must remain nonterminal and preserve the immutable PDF. Copying and fsyncing an existing PDF to its supplied descriptor can satisfy file delivery only when the chosen attributes are already supported; it is not proof that the content has been re-laid out for changed paper size or margins.

The valid current approach is to retain the files API and the PDF until a real documented terminal event arrives. A clearer submitted message is “System print request accepted; preview status and final result are unconfirmed.” This text remains accurate without claiming that printing has started or been cancelled. The lifecycle limitation should remain visible in acceptance results. Source snapshots and hashes for this audit are saved in `logs/build/print-cancellation-audit-20261008`; this audit changed no ArkTS, Native code, HAP, or device state.
