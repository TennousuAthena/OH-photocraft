# Attribution

PhotoCraft is developed by the ArtCraft Team and the PhotoCraft contributors. This repository
contains a HarmonyOS port by [TennousuAthena](https://github.com/TennousuAthena), based on
[storytold/photocraft](https://github.com/storytold/photocraft) at
`4337a6227a823a28728e68aed844feab62b3314d`.

The original copyright notices are retained in [LICENSE-MIT](LICENSE-MIT),
[LICENSE-APACHE](LICENSE-APACHE) and [NOTICE](NOTICE). Source code is licensed under
MIT OR Apache-2.0. Third-party assets retain their respective licenses.

| Material | Attribution and license |
| --- | --- |
| Original PhotoCraft code, artwork, fonts, icons and dictionary | [Upstream attribution](upstream/ATTRIBUTION.md) and [notice](upstream/NOTICE) |
| HarmonyOS runner, ArkTS shell and native bridge | HarmonyOS port by TennousuAthena; MIT OR Apache-2.0 |
| Modified PhotoCraft UI crate | [UI overlay notes](vendor/photocraft-ui-egui/README.md); original PhotoCraft notices retained |
| Modified PhotoCraft engine crate | [Engine overlay notes](vendor/photocraft-engine/README.md); original PhotoCraft notices retained |
| Local eframe compatibility adapter | [Adapter notes](vendor/eframe/README.md), [MIT](vendor/eframe/LICENSE-MIT) and [Apache-2.0](vendor/eframe/LICENSE-APACHE) notices retained |

License texts for the embedded Inter and JetBrains Mono fonts, Lucide icons and SCOWL
dictionary are included in [the HarmonyOS package resources](harmonyos/entry/src/main/resources/rawfile/licenses/).
The shell icon is an original SVG included in the port source under MIT OR Apache-2.0.

The ArtCraft name and logos in `upstream/docs/brand/` have separate
[trademark terms](upstream/docs/brand/LICENSE-brand.txt). Those terms remain applicable to the
upstream material. The HarmonyOS shell uses its own icon.
