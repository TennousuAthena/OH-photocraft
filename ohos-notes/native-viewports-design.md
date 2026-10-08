# Native document viewport design

Status: **design only, not implemented**. Reviewed against the installed DevEco SDK 26 and egui 0.36.2 on 2026-10-08. The current signed package still uses one native XComponent and embedded document viewports. This document does not authorize or claim a multiwindow implementation.

## Current behavior and evidence

`vendor/photocraft-ui-egui/src/canvas.rs::extra_windows` creates a stable egui `ViewportId` from `("docwin", w.id)` and calls `show_viewport_immediate`. Each view has its own `w.view`, while sharing the original application/session/document. Its close callback marks the view closed; it does not close the underlying document.

The OHOS runner neither disables viewport embedding nor installs an immediate viewport renderer. egui 0.36.2 defaults `embed_viewports` to `true`; `show_viewport_immediate` therefore creates an `egui::Window` inside the root viewport. Even setting embedding to `false` would continue to fall back to an embedded window without a renderer callback.

The native bridge currently owns one `activeComponent`, `inputComponent`, and `activeWindow`. A second surface would replace the first and destroy its GPU surface. The Rust runner also owns one `surface`, one `InputState`, one pending `FullOutput`, and accepts repaint requests only for `ViewportId::ROOT`. Registering a second XComponent before replacing these singletons is unsafe.

## SDK entry points

The installed `@ohos.window.d.ts` exposes:

- `WindowStage.createSubWindowWithOptions(name: string, options: SubWindowOptions): Promise<Window>` since API 11.
- `SubWindowOptions` includes `title`, `decorEnabled`, optional `isModal`, and optional `maximizeSupported` on devices supporting freeform windows. The latter is not a guarantee on other device types.
- `Window.loadContent(path: string): Promise<void>` and `Window.loadContent(path: string, storage: LocalStorage): Promise<void>`, followed by `showWindow()` and `destroyWindow()`, provide the page and window lifecycle.
- A child page can own an independent XComponent. `OH_NativeXComponent_GetXComponentId(component, id, &size)` since API 8 identifies the component; its callback arguments identify the native component and surface.

Use a nonmodal decorated Stage subwindow. Pass an opaque viewport token through the page's LocalStorage and use a unique XComponent options ID derived from that token. Use the actual window ID and the component's physical `screenOffset` for IME/cursor targeting; do not derive these from an assumed root-window offset.

No system floating-window permission or additional UIAbility is required by this proposed Stage subwindow route. Runtime capability and window-creation failures still need normal error handling and device verification.

## Ownership and thread model

Keep one `PhotocraftApp`, one egui context/session, and the existing dedicated Rust worker. A child view must not instantiate a second application or duplicate its document state.

Replace native surface globals with a registry keyed by the actual XComponent handle and an opaque viewport token plus generation. Each entry owns its native-window retain, viewport-local geometry, pointer state, frame callback, and liveness. Replacement/destruction synchronously waits for the worker to drop only that viewport's GPU surface before unreferencing its native window. Late callbacks for a retired generation are ignored.

The worker owns a viewport registry with per-viewport input, size/density, surface, output, repaint deadline, and close state. Platform IME/cursor state follows the focused/hovered window; one system IME attachment remains active at a time. File and clipboard requests retain their existing document/layer/session targets when focus changes between windows.

Do not call or wait for an ArkTS/window service while holding the native lifecycle mutex or during a synchronous worker RPC. In particular, an immediate viewport renderer must not wait for the UI thread to create a window while a UI-thread frame callback is already waiting for that worker.

## Immediate renderer and asynchronous window creation

Install `Context::set_immediate_viewport_renderer` on the worker thread only after the viewport registry exists. egui stores this renderer in thread-local state. The callback receives a borrowed `viewport_ui_cb` that must run synchronously before the callback returns; failing to call it causes egui's integration assertion to fail. The callback must not capture and re-lock the whole runner while the original application is mutably borrowed.

Use a separate host registry, such as worker-owned `Rc<RefCell<ViewportHost>>`, which contains renderer/input/window metadata but not `PhotocraftApp`. The immediate callback obtains the child's `RawInput`, sets `viewport_id` and the complete viewport information map, calls `ctx.run_ui` with the borrowed child callback, and captures its output. This preserves the original `canvas_view` and its view-state updates.

If a child native window is absent, enqueue an asynchronous create request for ArkTS and still run the child UI callback with cached/default geometry. Never wait for surface creation. ArkTS creates the subwindow, loads the child page, shows it, and reports readiness or failure. The subsequent native surface callback attaches that viewport's renderer. Use explicit generation checks when readiness arrives after a view has already closed.

Window creation failure must be visible in the original UI and leave the document intact. Embedded fallback is a deliberate integration policy, not a side effect to claim as a successful native window. The current embedded behavior remains in place until this implementation passes its gates.

## Rendering and scheduling

Reuse the existing RenderState/device/renderer across compatible surfaces only after checking each surface's format/capabilities. wgpu GLES creation for multiple OHOS windows must be tested on the target device; a root surface working does not prove this case.

Collect shapes and pixels-per-point independently for each viewport. Shared egui texture updates must be applied once in order, and texture frees must be deferred until every viewport using those outputs has been rendered/submitted. The current `GpuSurface::paint` per-output upload/free behavior cannot simply be invoked for each surface without revisiting this ownership.

Accept repaint requests from every viewport. A child Vsync must continue to drive the application's UI while the root surface is minimized or absent; the current early return when the root surface is `None` would otherwise freeze child windows. Coalesce scheduling so concurrent native frame callbacks do not update the application twice for one logical frame. GPU/application work remains on the single worker.

## Input and close routing

Native pointer, wheel/pinch, key, focus, resize, and surface messages carry the viewport token and generation. Normalize coordinates with that viewport's own density and egui zoom. Maintain per-viewport pointer/button state and release it when its focus/surface is lost. Populate egui `RawInput.viewport_id` and `viewports` for both root and children rather than projecting child input into ROOT.

A child title-bar close injects `ViewportEvent::Close` for that child. The original `extra_windows` callback sets `w.open = false`; after acknowledgment, destroy only the Stage child window. Do not reuse root `closeRequested` or `terminateSelf`, and do not close the document just because a second view is closed.

Root application close continues through the existing unsaved Save/Discard/Cancel flow. Only its final accepted close terminates the UIAbility and all child windows.

## Implementation gates

1. Host-side tests prove token/generation isolation, child-close versus document-close behavior, and asynchronous create/failure without a worker/UI wait cycle.
2. Strict native/ArkTS compilation and Rust quality checks pass before replacing the current embedded mode.
3. On device, two documents and two views of one document retain independent zoom/pan while edits remain shared.
4. Keyboard, mouse, pinch, IME candidates, and clipboard target the correct window without mixing events.
5. Closing a child leaves the document/root usable; minimizing the root leaves a visible child responsive.
6. Child surface recreation and root/child teardown preserve retain/unref ordering and shared textures.
7. Cross-screen move/resize, differing density, window decorations, and cursor visibility have explicit device results.

No native registry, Rust viewport ABI, ArkTS child page, or Stage subwindow has been added as part of this design review.
