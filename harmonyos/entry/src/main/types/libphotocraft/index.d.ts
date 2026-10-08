/** Initializes the shared native host. Density is physical pixels per ArkUI vp. */
export const initialize: (filesDir: string, cacheDir: string, densityPixels: number) => boolean;
/** Supplies the current system BCP 47 locale before initialization or on configuration changes.
 * Auto follows this locale; an explicit interface language remains selected. */
export const setSystemLanguage: (languageTag: string) => boolean;
/** Paths must point to a file in the application sandbox, never a picker URI. */
export const openDocument: (path: string) => boolean;
/** Exports a flattened PNG without changing the document path or marking the project saved. */
export const saveDocument: (path: string) => boolean;
/** Pulls one original-menu request JSON {id,kind,path,suggestedName,chooseDestination}, or ''. */
export const takeFileRequest: () => string;
/** Re-encodes the pending document snapshot to the selected filename's real format; '' means failure. */
export const prepareFileSave: (id: number, destinationName: string) => string;
/** Completes with success/cancel/error; Open uses a sandbox resolvedPath, Save uses ''. */
export const completeFileRequest: (id: number, result: string, resolvedPath: string,
  displayName: string, error: string) => boolean;
/** Reports a file or drag error in the original PhotoCraft status area. */
export const reportFileError: (message: string) => void;
/** Takes the independent IME snapshot and one-shot host events, without a render RPC. */
export const takePlatformOutput: () => string;
/** Applies IME on the UI thread. Origin is XComponent screenOffset in physical px. Returns closeRequested. */
export const applyPlatformOutput: (context: UIContext, outputJson: string,
  physicalOriginX: number, physicalOriginY: number, windowId: number) => boolean;
/** Enqueues clipboard completions and other platform inputs without blocking the render worker. */
export const platformInput: (inputJson: string) => boolean;
/** Detaches IME on page/background/window teardown. */
export const releasePlatform: () => void;
export const lastError: () => string;
/** NDK callbacks normally forward these events; exposed for platform fallbacks. */
export const onKey: (code: number, pressed: boolean, ctrl: boolean, shift: boolean,
  alt: boolean, text: string) => void;
export const onScroll: (dx: number, dy: number) => void;
/** Committed IME text; key callbacks alone cannot represent composition. */
export const textInput: (text: string) => void;
/** Available only in the opt-in Debug device-test HAP and an isolated initialized run. */
export const testSubmit: (runId: string, requestJson: string) => string;
/** '' means pending; a completed upstream control response is consumed once. */
export const testPoll: (runId: string, ticket: string) => string;
/** Non-consuming latest runtime, semantic UI and small-document pixel snapshot. */
export const testSnapshot: (runId: string) => string;
import { UIContext } from '@kit.ArkUI';
