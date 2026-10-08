# PhotoCraft OHOS platform protocol

The Rust render worker owns the PhotoCraft app and egui context. Native IME
callbacks enqueue inputs without waiting for that worker. UI-thread callers pull
an independent snapshot and a bounded output mailbox. System clipboard reads
happen only for a user's Paste intent; idle frames never request clipboard data.

## C ABI

- `craft_take_platform_output() -> const char*`: JSON copied into calling-thread
  storage, valid until the next call on that thread. No render-thread RPC.
- `craft_platform_input(const char* json) -> bool`: validates JSON size/kind and
  calls `try_send` on a 64-request mailbox. `true` means enqueued, not completed.
  A paste PNG then belongs to Rust; ArkTS deletes it only when enqueue returned
  `false`. Render errors are reported through PhotoCraft's status bar.

The existing five-field file-completion ABI is unchanged. File operations still
use their own request IDs and only successful external publication marks a
captured document revision saved. File-dialog format changes call
`prepareFileSave(id, destinationName)` before publishing the returned actual
stage. The formats match upstream desktop save filters: PSD, PSB, pcraft, PNG,
JPG, TIF, TGA and EXR. Export and in-place saves keep their selected format.
Choosing a different basename in the same format renames only the exclusive
request's stage, preserving encoded bytes, revision and warnings. Successful
Save adds that actual path to Recent; completed copies and exports do not.

Only a successful document Save adds retained staging authority to
`filesDir/PhotoCraft/Documents/published-saves.json`. On restart the bounded
versioned journal validates relative request/file paths, supported extensions,
regular files without symlink aliases, and unchanged positive file sizes.
Imported files and failed, cancelled or exported stages do not grant in-place
Save. Journal read/write errors appear in PhotoCraft's status bar; a failed
journal write does not undo a publication that already succeeded.

A save `error` completion may carry the actual stage in `resolvedPath` only to
retain the last encoded copy after failed destination rollback. Rust requires
an exact match to that pending request's regular staging file. This never marks
the document saved. Ordinary errors and cancellation clean fresh drafts.

## Output

```json
{
  "ime": null,
  "cursorIcon": "Default",
  "events": [],
  "closePending": false,
  "closeState": null
}
```

An active `ime` has `target`, surrounding `text`, UTF-16 `selectionStart` and
`selectionEnd`, nullable UTF-16 `compositionStart`/`compositionEnd`, logical
`rect` and `cursorRect` as `[x,y,width,height]`, `purpose` (`normal`, `password`,
`terminal`), `interrupt`, `zoom` and system `density`. The native adapter converts
rectangles to the coordinate system required by its API. Selection endpoints
retain anchor/caret direction; replacement ranges snap outwards around surrogate
pairs. Native preview partial replacements are resolved in native UTF-16
coordinates; Rust receives a complete replacement preedit string.
`cursorIcon` uses egui's stable enum variant names, including `None` to hide the
system cursor. Unknown names fall back to the normal pointer in native code.

Events have JavaScript-safe integer `id`:

- `copyText`: `text`.
- `copyImage`: `path` (sandbox PNG), `width`, `height`.
- `paste`: `textOnly` (true for a focused text editor, false for image documents).
- `openUrl`: `url`, `newTab`.
- `close`: final authorization to terminate the UIAbility.
- `viewport`: `command` fullscreen/maximized plus boolean `enabled`, or
  startDrag. Each command is acknowledged through `platformComplete`; native
  window behavior remains a separate device verification gate.
- `printRequest`: `pdfPath`, an immutable PDF under
  `filesDir/PhotoCraft/Documents/Printing/<id>/PhotoCraft.pdf`. The PDF is rendered
  by the original engine, including page layout, marks and color management.
  Additional metadata is `documentId` (a u64 string), `requestedCopies` and
  nullable `requestedPrinter`. These are intent, not confirmed system settings.
  The bounded top-level `printJobs:[{id,documentId}]` snapshot retains each
  outstanding job's original document identity through tab changes and Batch
  scratch sessions. The shell may ignore these diagnostic fields.
  The shell submits it with PrintingBridge; it must not send a successful generic
  `platformComplete` for submission alone. The system print dialog determines
  the printer and final copy count; the files API does not preselect the original
  dialog's printer name or copy count. Native physical output remains unverified.

Copy-image PNGs are removed on `platformComplete`; ArkTS may remove them after
its system clipboard operation as an additional cleanup. Outstanding image
acknowledgements and output events are limited to 64 each. Clipboard image
input/output is limited to 64 MiB; the decoder also has allocation/dimension
limits. Accepted paste PNGs must be regular files inside `cacheDir/Clipboard`
and are removed after success, stale-target rejection or decode failure.

## Input

IME messages carry the current snapshot opaque `target`, which changes across
focus loss/reacquisition even for the same widget:

- `imePreedit {target,text}`, `imeCommit {target,text}`, `imeCancel {target}`.
- `imeDelete {target,before,after}`: UTF-16 units outside the selection or
  composition. Native deletion within a preview sends an updated full preedit.
- `imeSelection {target,start,end}`: UTF-16 selection offsets.
- `imeMove {target,direction}`: left/right/up/down.
- `imeAction {target,action}`: selectAll/cut/copy/paste.
- `imeEnter {target}`.

In-flight IME messages for a detached target are ignored. Every accepted packet
runs a real egui UI pass before the next packet, refreshing surrounding text and
selection. Intermediate `FullOutput` values append their texture deltas; only
latest shapes are rendered. Texture deltas are consumed even when a surface is
lost or the runner is dropped.

Clipboard completion is
`{kind:"clipboard",id,result,text,imagePath,error}`, where `result` is
success/cancel/error. Image Paste preserves its initiating document across tab
switches. New from Clipboard activates its new document. Text Paste rejects a
changed text target. Copy completion is
`{kind:"platformComplete",id,result,error}`.

Before showing the visible system PasteButton, the shell enqueues
`{kind:"clipboardAuthorize",id}` for that existing pending Paste. This permits
temporary native IME detach only while the original egui editor or document
text session still owns the request. A new widget, document, text session or
focus generation cannot receive it. Cancel consumes the pending request and
late completion is rejected. The authorization never performs a clipboard
read; the shell reads only after the visible button's successful user action.

`{kind:"viewportState",fullscreen:boolean,maximized:boolean}` reports actual
OS window state to the next egui RawInput. Requested state is never reported as
successful before the OS confirms it. The current shell may defer these window
adapters independently of the clipboard/IME package.

Print updates are
`{kind:"printingUpdate",id,state,terminal,error}`. `submitted`, `blocked` and
`monitoringError` are nonterminal. `succeeded`, `failed`, `cancelled` and
`rejected` are terminal; the boolean must match the state. Only terminal updates
consume the outstanding request and remove its PDF. A monitoring error keeps
the PDF and reports an unknown result. Disposal, missing callbacks and timeouts
do not imply cancellation or completion. At most eight system print jobs remain
outstanding. Process exit leaves indeterminate inputs in durable storage.
`submitted` means only that the system printing request was accepted; its final
job state is unconfirmed. Closing the system preview has not produced a public
terminal callback on the tested device. Preview dismissal is not reported as
job cancellation and does not remove that PDF.

Original Print-to-PDF (nonempty `output` field) uses the original engine with
`send:false` and publishes the real PDF through the existing fixed-format save
transaction. Same-format destination naming keeps the same PDF bytes. Print,
PDF export and every system task outcome preserve the working document's
path, revision, saved revision and dirty state. Actions keep original print
parameters, so playback cannot mistake a temporary spool path for PDF export.

The minimal replayable engine overlay adds an optional `Session.print_spool`
service after the original PDF rendering. Physical prints from menus, Actions,
nested scripts, Batch and Droplets enter the same asynchronous spool service;
the callback receives immutable PDF bytes/metadata and no Session. Batch scratch
sessions inherit this service. Original Print Script Events run normally after
accepted submission with their existing recursion guard and enabled preference.
Acceptance reports `pending:true`, `sent:false` and `printRequest`; it never
claims system completion. The original PDF/layout algorithms and upstream
checkout are unchanged. Without a callback, desktop behavior is unchanged.

A title-bar close sends `{kind:"closeRequested",id}`. Rust invokes the original
File Exit/unsaved-changes state machine. Output `closeState` becomes
`{id,pending:true}` while that prompt or deferred save is active, and false after
original Cancel or final close authorization. The shell ignores null/old IDs,
so a snapshot taken before the worker receives the request cannot cancel its
poll. Save cancellation/failure keeps the prompt, document and window. File
completion also flushes UI output without requiring a foreground VSync.

## Validation

Host tests cover real document and egui TextEdit Chinese composition, cancel,
UTF-16 surrogate selection/deletion and queued consecutive deletes; text and
RGBA clipboard paths; original target identity; cache cleanup and bounds; title
bar close acknowledgements, original Cancel and Save-before-Exit publication;
PNG destination reuse after successful Save; and registered system CJK fonts
producing nonzero glyph IDs and raster alpha. Native IME/clipboard and Chinese
appearance on the device remain separate verification gates.
