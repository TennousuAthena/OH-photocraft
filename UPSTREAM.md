# PhotoCraft upstream

- Repository: https://github.com/storytold/photocraft
- Pinned commit: `4337a6227a823a28728e68aed844feab62b3314d`
- License: MIT OR Apache-2.0, see `upstream/LICENSE-MIT` and `upstream/LICENSE-APACHE`.

`upstream/` is a Git submodule pinned to the commit above. The OHOS Cargo workspace lives outside
it and consumes the engine and UI crates through path dependencies. Clone this repository with
`git clone --recurse-submodules https://github.com/TennousuAthena/OH-photocraft.git`, or initialize
an existing checkout with `git submodule update --init --recursive`. Keep the submodule at this
pin until the compatibility checks have been repeated.

The local `vendor/eframe` package is an API compatibility adapter for the OHOS runner. It does
not contain a desktop window backend; the original upstream desktop build keeps using the real
eframe package. ArkTS hosts the native XComponent and HarmonyOS platform services.

`vendor/photocraft-ui-egui` and `vendor/photocraft-engine` are copies of the corresponding pinned
upstream crates with HarmonyOS changes. Their README files describe the changes and the
replay/check procedure. The original upstream checkout is preserved without modifications.

HarmonyOS port author: [TennousuAthena](https://github.com/TennousuAthena).
Original PhotoCraft copyright and asset notices remain in the upstream license files,
`upstream/NOTICE` and `upstream/ATTRIBUTION.md`; the port attribution is in [ATTRIBUTION.md](ATTRIBUTION.md).
