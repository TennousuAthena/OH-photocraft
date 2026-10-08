# eframe application seam for OHOS

The port workspace patches eframe 0.36.2 to this small application interface.
It exports the upstream `App` callback signatures, a safely constructible `Frame`,
and the original egui / egui-wgpu / wgpu types. It contains no winit or egui-winit
dependency. The upstream PhotoCraft checkout remains unchanged and its desktop
and browser builds keep the real eframe crate.

Why a Cargo patch is required: upstream eframe has unconditional native winit
and egui-winit dependencies, and enables egui-wgpu's winit integration whenever
its wgpu backend is selected. Skipping `run_native` does not skip these packages.

The API surface was audited against PhotoCraft commit `4337a62`. The production
UI uses only `App::{logic,ui,raw_input_hook}`, `Frame`, and GPU type reexports.
Its callbacks do not read `Frame`. Tests and desktop app examples that need
eframe's native runners, `CreationContext`, or egui-kittest's eframe integration
must run from the unchanged upstream workspace; they are outside this facade.

Apply only in the independent OHOS workspace:

```toml
[patch.crates-io]
eframe = { path = "vendor/eframe" }
```

`wgpu` keeps the upstream feature name but deliberately does not choose a GPU
backend. The runner enables `wgpu/vulkan` and/or `wgpu/gles` itself. `accesskit`
is accepted as a feature name; egui 0.36 contains accessibility tree support
unconditionally, while the native bridge remains the runner's responsibility.

The callback declarations and defaults are compatible with the upstream eframe
0.36.2 `epi.rs`, licensed MIT OR Apache-2.0 by the egui contributors:
<https://github.com/emilk/egui/blob/0.36.2/crates/eframe/src/epi.rs>.
This adaptation is distributed under the same license; license texts accompany
this crate.
