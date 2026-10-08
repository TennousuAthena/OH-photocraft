# PhotoCraft egui OHOS overlay

This package is a copy of `upstream/crates/ui-egui/src` at
`4337a6227a823a28728e68aed844feab62b3314d`, with the changes recorded in
[`ohos-async-files.patch`](ohos-async-files.patch). The original checkout stays
clean and desktop builds still use the original UI package and eframe.

The OHOS workspace selects this package through the application's path
dependency. Its explicit Cargo manifest replaces the upstream workspace-inherited
dependencies, and embedded resource paths refer back to `upstream/assets`.
MIT, Apache-2.0, NOTICE and ATTRIBUTION files are retained beside this manifest;
the upstream asset directories retain their own license files.

The platform needs public asynchronous file seams because a system picker and
external URI publication finish after the egui UI pass. Returning a sandbox
path alone lets the original synchronous Save path clear the dirty flag and
close a document before the user accepts a destination. Reconstructing a closed
document after that pass also loses the private unsaved-changes prompt.

The patch adds optional Services hooks and preserves the synchronous desktop
behavior when they are unset:

- `pick_open_intent(command, document_id, params)` keeps Open, Open As, Place,
  Notes, scripts and preset-import options attached to the request. An unresolved
  asynchronous picker returns without executing an engine command with a missing
  path.
- `defer_save(document_id, revision, path)` returns the actual staging path.
  The UI holds document metadata and Save-before-Close until
  `complete_deferred_save` confirms publication. Success saves the encoded
  revision by document identity. New edits stay dirty; cancellation and failure
  preserve the document and prompt. Save Document script events run after
  successful publication. Successful Save As also updates the document title
  to the actual destination basename; Copy and Export retain the original title.
- `save_copy_ready(path)` distinguishes Save a Copy from format-fixed export.
  `retarget_deferred_save` keeps the saved revision attached to a fresh staging
  path when a system picker chooses another supported format. The platform
  re-encodes a COW document snapshot with the original export settings; changing
  the destination suffix alone never changes the encoded format.
  Retargeting with unchanged format retains the existing warnings when only
  the actual destination basename changed.
- `save_in_place(path)` permits a successfully published flat staging file to
  reuse its explicit destination. Imported flat originals still use Save As.
- `clipboard_request` preserves asynchronous Paste intent before the original
  menu reads its cached RGBA result. Document Type exposes surrounding text and
  handles egui DeleteSurrounding; native UTF-16 offsets are converted by the
  wrapper. Close authorization and pending status are read from the original
  unsaved-changes state machine.
- `save_unsaved_changes` uses the original prompt's Save action, allowing a
  native shell or host integration test to drive the same state machine.
- `quick_export` lets the platform publish through system file authority even
  when the desktop preference requests an ambient same-folder write.
- `print(session, command, params)` keeps original Print/Print One Copy menus,
  dialog fields and Actions. Physical printing uses the engine overlay's
  optional spool service, including engine scripts, Batch and Print Script
  Events. PDF export renders with `send:false` to an explicit sandbox PDF. The dialog
  reports pending system submission or PDF publication instead of claiming
  rendering alone printed a document. Unset hooks retain desktop behavior.
- `browse_folder(Request)` adds Browse and Cancel beside the original Load Files
  into Stack, Contact Sheet II and Statistics input fields. The request captures
  the original dialog, field, generation and document. A completed selection
  fills the original file array without running a command; the original OK
  action runs the engine after validation. Pending/empty selections disable OK,
  including Enter and programmatic confirm. Unset hooks preserve desktop forms.
- Image Processor, Batch and Lens Correction reuse `browse_folder` for their
  original input rows and add
  `browse_folder_destination` at their original output rows. An authorized folder
  is represented by a private logical handle, displayed as a selected folder.
  `process_folder` retains the original confirmed parameters while the platform
  allocates an empty output tree; `folder_progress` uses the existing status
  footer and Cancel control. Original engine encoding finishes before external
  publication, and the final receipt preserves individual failed inputs.
- `recent_file` supplies a logical source identity and friendly labels for the
  original Home and File > Open Recent entries. `open_recent` lets the platform
  read a fresh import from that source after checking file authority. Unset hooks
  retain the original synchronous path behavior.
- `folder_recoveries` and `recover_folder` offer retained directory outputs from
  the original status footer. Retry publishes the recorded bytes without
  replaying commands or encoding inputs again; another destination can be
  authorized without changing the captured output task.

The original menus and shortcuts remain the editing UI. The OHOS bridge keeps
one pending operation, stores retained document/import files under
`filesDir/PhotoCraft/Documents`, and removes only a fresh cancelled/failed draft
or a completed export/copy. Save completion changes no other document's state.

The overlay also accepts the native host's current system language through
`i18n::set_system_language`. Auto follows that input on each frame; an explicit
language preference keeps its existing behavior. Both Chinese catalogs cover
the current UI labels, including the OHOS folder controls and progress messages.
The language dropdown names Auto as following the system. User document contents,
paths, command IDs and shortcut bindings keep their original values.

Run the non-mutating reconstruction check from the OHOS workspace:

```sh
python3 vendor/photocraft-ui-egui/verify-overlay.py
python3 scripts/check-localization.py
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/host cargo test -p photocraft-ohos -p craft-ohos-platform -p eframe
```

The check copies the pinned upstream source to a temporary directory, applies the
recorded patch and compares every source byte to this overlay. Host tests cover
real menu saves, Save As, Save a Copy, export, quick export with `sameFolder`,
PNG-to-PSD/pcraft layer preservation, snapshot/revision consistency after later
edits, changed-format cancellation and encoding failure, Open/Open As, target-preserving embedded/linked Place, preset options and script
imports. These tests do not replace native picker, URI-provider or drag/drop
verification on a HarmonyOS device.

Folder tests use real PNG files and the original three menu commands, check
resulting layers/pixels and contact-sheet/statistics documents, render the
original form's egui shapes, and cover cancellation, stale targets, malformed
manifests, copied links and duplicate completions. Actual provider layout and
visual inspection on HarmonyOS remain separate device gates.

The view overlay keeps each camera and extra document window bound to DocId.
The original UI only resized index-based view storage when closing a tab, so
closing a non-final document gave surviving tabs the removed document's camera.
Only changed document identity/order rebuilds the mapping; new documents start
with their default camera and closed documents lose their extra windows.

The original Pro status footer also renders its existing status message on the
Home screen with no documents. Folder errors, progress and confirmed publication
remain visible at the original footer position. Host tests inspect text shapes
from the full App logic/UI lifecycle, rather than checking only status storage.

The Pro document strip uses the existing tab-strip fit/elide/overflow algorithm.
Its active document stays visible in a narrow window, and the overflow menu can
select every hidden document by stable identity. The original close buttons,
colors, height and document-opening progress stay in that strip.

The OHOS Actions panel starts the engine's resumable Action job. Its original
Stop control cancels Action runs while the existing progress Cancel/Esc controls
use exact JobIds; unrelated jobs are preserved. Script results retain nested
step errors and pending print results. No new toolbar or native scripting
algorithm is introduced.
