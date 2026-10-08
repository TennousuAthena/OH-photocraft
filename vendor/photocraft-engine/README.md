# PhotoCraft engine OHOS overlay

This package copies `upstream/crates/engine/src` and `tests` at
`4337a6227a823a28728e68aed844feab62b3314d`. Its minimal changes are recorded in
[`ohos-print-spool.patch`](ohos-print-spool.patch); the original checkout remains
clean. The explicit Cargo manifest retains the original versions/features and
lower-layer dependencies. MIT, Apache-2.0, NOTICE and ATTRIBUTION files remain
beside the manifest. Source/test fixture paths point back to the original
upstream fixtures and corpora; no assets or PDF algorithms are copied separately.
The original rustfmt/clippy configuration is retained. One existing test has a
precise `manual_range_contains` allowance, preserving its rejection of NaN under
the current strict compiler instead of changing that test's numeric semantics.

The directory-input seam also makes the existing `file_cmds::list_images`
function public (both native and wasm definitions). Its implementation and
supported extensions remain unchanged. The wrapper uses this function after
validating a copied folder, preserving original top-level-only enumeration and
full-path Rust string sorting rather than copying that logic or trusting the
manifest's JavaScript ordering.

`Session.directory_output` is an optional, lower-layer-only output resolver for
the original Image Processor, Batch and Lens Correction. It receives the command
and its logical output handle, with no Session reference. The OHOS wrapper grants one validated empty
output tree only after explicit folder authorization and stage allocation.
The grant matches command and handle before single consumption. The original
`process_files`, Action steps, scratch sessions, lens flatten/filter, fit, color
conversion, encoding and case-insensitive `OutputClaims` run unchanged. The recorded parameters retain the logical handle,
while an expired or unprepared handle returns an explicit authorization error.
Unset callbacks preserve the original desktop and wasm behavior. Other directory
commands are not connected through this resolver.

The optional `Session.print_spool` callback receives the original rendered PDF
bytes, immutable document identity/name, requested copy count and printer name.
It receives no Session, so the adapter cannot re-enter command dispatch. For a
physical print (`send:true`, excluding `dryRun`) it replaces only the final file
write and desktop `lp` submission. An accepted callback returns a sandbox PDF,
request ID and pending status; `sent` remains false. Unset callbacks retain the
original desktop behavior. Explicit PDF output, `send:false`, dry runs, layout,
color management and PDF encoding retain the original implementation.

The original command registry still drives menus, Actions, nested scripts and
Print Script Events. Events fire after accepted submission, with the original
event recursion guard and preferences intact. Batch and Droplet scratch sessions
inherit only this platform service, preserving their original document and
script semantics. A print completion never modifies any document's saved state.

The wrapper atomically writes each PDF to its own durable sandbox directory and
retains immutable job ownership. It limits outstanding jobs to eight. Submission,
blocked status and monitoring errors do not mean printing completed; only a
system terminal update consumes that job and removes its own PDF. The system
dialog determines the printer and final copy count, because the platform's file
printing API does not preselect these fields.

Run the exact non-mutating reconstruction and host checks from `OH-PhotoCraft`:

```sh
python3 vendor/photocraft-engine/verify-overlay.py
python3 vendor/photocraft-ui-egui/verify-overlay.py
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/host cargo test --offline --locked -p photocraft-engine -p photocraft-ohos
```

Wrapper tests dispatch real engine scripts and Batch actions, verify independent
PDF bytes and document identities, and exercise terminal/nonterminal updates.
The engine's original tests run against this overlay. These checks do not
replace physical printer or HarmonyOS task-callback verification.

`Session.source_file` is an optional command boundary for Revert and Smart Object
source operations. It captures an immutable document, DocId, LayerId, path and
revision before platform IO; `complete_source` validates the same owner/version
and invokes the original decode, refresh, source replacement and history logic.
The wrapper fetches fresh authorized bytes and commits a Linked conversion only
after external publication. No URI, egui type or PDF/image algorithm enters the
engine. Unset callbacks keep desktop/wasm command behavior.

The OHOS wrapper explicitly enables `Session.automation`. Scripts Browse, nested
Scripts, Actions and Script Events then use bounded per-step continuations, with
logical document ownership distinct from the tab a user visits during IO. A
source response must match run, scratch-session generation, step, document Arc,
revision, path and selection. A cancelled/failed source stops remaining writes.
Original caller parameters are journaled only after completion; the stable layer
used for that completion never leaks into a cross-document recorded Action.

Batch retains each original scratch Session until its steps finish. The original
`process_files` pipeline is factored at its command callback boundary; import,
format/quality, SavePath, encode, errors and case-insensitive OutputClaims remain
shared with the synchronous implementation. The wrapper holds its virtual engine
job after encoding until the validated external folder publication receipt. An
error or cancellation preserves any already encoded bytes for recovery. Print
submission remains explicitly pending and does not complete a system print job.

Unset automation retains synchronous desktop/wasm behavior. Native inline
fallbacks still reject a source step when a configured platform callback cannot
be resumed. Scripts that directly start a separate Batch/Processor require their
own folder authorization/allocation continuation; an expired or unprepared
logical directory handle errors without writing. This is an explicit remaining
platform boundary, not a claim of complete automation parity.
