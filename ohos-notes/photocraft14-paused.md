# PhotoCraft #14 paused checkpoint

Paused on 2026-10-08 at the user's request to prioritize packaging the other six applications. The installed #13 package and its immutable archive remain unchanged. No PhotoCraft device interaction, Cargo, rustc, host tests, SDK build, archive copy or HAP build was started in this batch.

Checkpoint: `logs/work-in-progress/photocraft14-paused-20261008/`. `owned-source.sha256` and `owned-unvalidated-changes.patch` identify this batch's seven changed/new product files. The complete observed live-versus-#13 inventory also detects 20 other changed files that this batch did not author; these are preserved separately in the machine-readable checkpoint without attribution and were reported to the parent. Live edits are preserved; nothing was reverted or deleted.

The unvalidated edits add an optional immutable `tree_output` engine request and hooks in the original Save for Web, Image Assets, after-save assets and Package handlers. Original UI sliced web confirmation can bypass the single-file picker when this hook is enabled. Package snapshot encoding has an opt-in relative `Links/<basename>` rewrite. Command admission is intended to defer successful export journaling until publication.

These edits are incomplete and have not been compiled, formatted, tested or replayed. The attempted follow-up Processor edit was interrupted before execution: `processor.rs` and `folder_recovery.rs` still match #13. Existing overlay patches still reproduce #13 and have not been regenerated for the new source. The live modified vendor tree therefore must not be described as build-ready or used to replace the released #13 archive.

Remaining work before resuming validation:

- Wire bounded tree requests through the existing folder destination, allocation, original snapshot encoder, full-tree validation, publication receipt and durable recovery. HTML and transparent spacers must be included in the actual published count.
- Preserve automatic Image Assets as an independent export after successful external project save; errors must be visible without undoing the project save marker. Check safe-close ordering and target cancellation.
- Complete Package parent-folder authorization and imported sibling Links ownership/fresh reads. Relative exported links and cached previews alone do not prove independent linked-source reopening. No URI concatenation or guessed provider hierarchy is permitted.
- Add actual API 20/21 URI-stat rejection before picker/provider write, retaining encoded files and journal; verify API 26 normal operation. ArkTS helper intent allowlists have not yet been extended.
- Keep directory Scripts/Actions/Batch continuation as a later explicit execution-frame task; the new engine hook currently rejects script-scope tree requests instead of pretending the step completed.
- Add the original-algorithm/publication/receipt/cancel/reopen tests, then fmt, strict clippy, overlay regeneration/replay, wasm and isolated API26 compile once the parent authorizes validation. No checks are recorded as passed for #14.

Owned process status at pause: no long-running command or compiler was started. All read/write tool invocations completed; there is no owned session to wait for or terminate. Upstream checkout remains clean.
