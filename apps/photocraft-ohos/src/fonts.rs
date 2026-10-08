//! Lazy HarmonyOS system-font sources for the unchanged PhotoCraft CJK plugin.
//!
//! Install this before `PhotocraftApp::setup_context`: egui ignores a second
//! plugin of the same type. No font bytes are read until the plugin finds a
//! missing CJK glyph in a rendered frame.

use photocraft_text::cjk::{CjkScript, FontFile};
use photocraft_ui_egui::cjk_fonts::{self, Sources};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const SYSTEM_FONT_DIR: &str = "/system/fonts";
const MAX_CANDIDATES: usize = 256;

/// Install OHOS CJK font discovery before the upstream theme registers its plugin.
pub fn install(ctx: &egui::Context) {
    cjk_fonts::install_with(
        ctx,
        Sources {
            locale: || Some(photocraft_ui_egui::i18n::current().code().to_string()),
            files: files_for_script,
            last_resort: last_resort_files,
            embedded: cjk_fonts::craft_embedded,
        },
    );
}

fn system_paths() -> &'static [PathBuf] {
    static PATHS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    PATHS.get_or_init(|| discover_paths(Path::new(SYSTEM_FONT_DIR)))
}

fn discover_paths(directory: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    // HarmonyOS installs fonts directly in /system/fonts. Limit both the scan
    // and file sizes; no recursive traversal or font-data reads happen here.
    let Ok(entries) = std::fs::read_dir(directory) else {
        return paths;
    };
    for entry in entries.take(MAX_CANDIDATES).flatten() {
        let path = entry.path();
        if !path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "ttf" | "otf" | "ttc" | "otc"
                )
            })
        {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_file() && metadata.len() != 0 && metadata.len() <= cjk_fonts::MAX_FONT_BYTES
        {
            paths.push(path);
        }
    }
    paths.sort();
    paths
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn script_hint(name: &str) -> Option<CjkScript> {
    // Cover HarmonyOS_Sans_SC / TC / JP / KR and NotoSansCJKsc / NotoSansSC.
    let compact: String = name
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect();
    for (suffix, script) in [
        ("sc", CjkScript::SimplifiedChinese),
        ("tc", CjkScript::TraditionalChinese),
        ("jp", CjkScript::Japanese),
        ("kr", CjkScript::Korean),
    ] {
        if compact.contains(&format!("sans{suffix}"))
            || compact.contains(&format!("serif{suffix}"))
            || compact.contains(&format!("cjk{suffix}"))
        {
            return Some(script);
        }
    }
    None
}

fn generic_cjk(name: &str) -> bool {
    name.contains("cjk") || name.contains("droidsansfallback")
}

fn candidates(paths: &[PathBuf], script: CjkScript) -> Vec<FontFile> {
    let mut paths: Vec<_> = paths
        .iter()
        .filter(|path| {
            let name = file_name(path);
            script_hint(&name).map_or_else(|| generic_cjk(&name), |hint| hint == script)
        })
        .cloned()
        .collect();
    paths.sort_by_key(|path| {
        let name = file_name(path);
        let script_score = u8::from(script_hint(&name) != Some(script));
        let style_score =
            u8::from(name.contains("bold") || name.contains("italic") || name.contains("black"));
        (script_score, style_score, name)
    });
    paths
        .into_iter()
        .map(|path| font_file(path, script))
        .collect()
}

fn font_file(path: PathBuf, script: CjkScript) -> FontFile {
    let name = file_name(&path);
    let is_collection = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "ttc" | "otc"));
    // Noto's collections have a face for each locale. Tell the upstream loader
    // which face to choose; ordinary fonts use their first face.
    let family = if is_collection && name.contains("notosanscjk") {
        match script {
            CjkScript::SimplifiedChinese => "Noto Sans CJK SC",
            CjkScript::TraditionalChinese => "Noto Sans CJK TC",
            CjkScript::Japanese => "Noto Sans CJK JP",
            CjkScript::Korean => "Noto Sans CJK KR",
        }
    } else if is_collection && name.contains("notoserifcjk") {
        match script {
            CjkScript::SimplifiedChinese => "Noto Serif CJK SC",
            CjkScript::TraditionalChinese => "Noto Serif CJK TC",
            CjkScript::Japanese => "Noto Serif CJK JP",
            CjkScript::Korean => "Noto Serif CJK KR",
        }
    } else {
        ""
    };
    FontFile { path, family }
}

fn files_for_script(script: CjkScript) -> Vec<FontFile> {
    candidates(system_paths(), script)
}

fn last_resort_files() -> Vec<FontFile> {
    system_paths()
        .iter()
        .filter(|path| {
            let name = file_name(path);
            generic_cjk(&name) || script_hint(&name).is_some()
        })
        .cloned()
        .map(|path| font_file(path, CjkScript::SimplifiedChinese))
        .collect()
}

/// Register a small set of installed OHOS faces with the document text engine.
/// Fontique's upstream Linux scan does not include /system/fonts. UI font
/// registration is independent of this engine and cannot fix Type-tool glyphs.
pub fn install_document_fonts() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let paths = system_paths();
        if paths.is_empty() {
            return;
        }
        let exact = |name: &str| {
            paths
                .iter()
                .find(|path| path.file_name().and_then(|name| name.to_str()) == Some(name))
        };
        let register = |path: &Path| {
            if let Ok(bytes) = std::fs::read(path) {
                let families = photocraft_text::shared()
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .fonts
                    .register_font_data(bytes);
                if !families.is_empty() {
                    crate::logger::info(&format!(
                        "Document system font {}: {}",
                        path.display(),
                        families.join(", ")
                    ));
                }
            }
        };
        for name in ["HarmonyOS_Sans.ttf", "HarmonyOS_Sans_SC.ttf"] {
            if let Some(path) = exact(name) {
                register(path);
            }
        }
        // Registered families with platform-specific names are selectable, but
        // the upstream fallback stack names Noto. Load one installed collection
        // only if its default Type family still lacks Chinese glyphs.
        let probe = photocraft_doc::TextLayer {
            text: "中文".into(),
            font_family: "Inter".into(),
            ..Default::default()
        };
        let missing = {
            let mut engine = photocraft_text::shared()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let layout = engine.layout(&probe, 72.0);
            layout.glyphs.is_empty() || layout.glyphs.iter().any(|glyph| glyph.id == 0)
        };
        if missing && let Some(path) = exact("NotoSansCJK-Regular.ttc") {
            register(path);
        }
        let layout = photocraft_text::shared()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .layout(&probe, 72.0);
        crate::logger::info(&format!(
            "Document CJK glyph probe: {} glyphs; {} missing",
            layout.glyphs.len(),
            layout.glyphs.iter().filter(|glyph| glyph.id == 0).count()
        ));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn discovery_is_lazy_and_tolerates_missing_directories() -> Result<(), String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "photocraft-ohos-fonts-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        assert!(discover_paths(&directory).is_empty());
        std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        let font = directory.join("HarmonyOS_Sans_SC.ttf");
        // Discovery only stores paths. These bytes are deliberately not a font.
        std::fs::write(&font, b"not-read-during-discovery").map_err(|error| error.to_string())?;
        std::fs::write(directory.join("empty.ttf"), b"").map_err(|error| error.to_string())?;
        std::fs::write(directory.join("font_config.json"), b"{}")
            .map_err(|error| error.to_string())?;
        let paths = discover_paths(&directory);
        assert_eq!(paths, vec![font]);
        std::fs::remove_dir_all(directory).map_err(|error| error.to_string())?;
        Ok(())
    }

    #[test]
    fn cjk_candidates_prefer_the_requested_locale_and_regular_weight() {
        let paths: Vec<_> = [
            "HarmonyOS_Sans_SC_Bold.ttf",
            "HarmonyOS_Sans_SC.ttf",
            "HarmonyOS_Sans_JP.ttf",
            "NotoSansCJK-Regular.ttc",
            "Roboto-Regular.ttf",
        ]
        .iter()
        .map(|name| Path::new(SYSTEM_FONT_DIR).join(name))
        .collect();
        let files = candidates(&paths, CjkScript::SimplifiedChinese);
        let names: Vec<_> = files.iter().map(|file| file_name(&file.path)).collect();
        assert_eq!(
            names,
            [
                "harmonyos_sans_sc.ttf",
                "harmonyos_sans_sc_bold.ttf",
                "notosanscjk-regular.ttc"
            ]
        );
        assert_eq!(
            files.last().map(|file| file.family),
            Some("Noto Sans CJK SC")
        );
    }
    #[test]
    fn registered_system_cjk_font_produces_real_document_glyphs_and_pixels() -> Result<(), String> {
        let path = photocraft_text::cjk::font_files(CjkScript::SimplifiedChinese)
            .into_iter()
            .map(|font| font.path)
            .find(|path| path.is_file())
            .ok_or("host has no CJK font for document render regression")?;
        let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
        let mut engine = photocraft_text::TextEngine::new();
        assert!(!engine.fonts.register_font_data(bytes).is_empty());
        let text = photocraft_doc::TextLayer {
            text: "中文".into(),
            font_family: "Inter".into(),
            size_pt: 32.0,
            ..Default::default()
        };
        let mut session = photocraft_engine::Session::new();
        session
            .execute("file.new", serde_json::json!({"width":1,"height":1}))
            .map_err(|error| error.to_string())?;
        let format = session
            .active()
            .ok_or("document missing")?
            .doc
            .pixel_format();
        let (layout, rendered) = engine.render(&text, 72.0, format);
        assert_eq!(layout.glyphs.len(), 2);
        assert!(layout.glyphs.iter().all(|glyph| glyph.id != 0));
        assert!(
            rendered
                .surface
                .read_region(rendered.rect)
                .chunks_exact(rendered.surface.channels())
                .any(|pixel| pixel.last().is_some_and(|alpha| *alpha > 0.0))
        );
        Ok(())
    }
}
