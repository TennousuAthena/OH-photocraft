//! PhotoCraft services backed by the UIAbility's absolute `filesDir`.
//!
//! Preferences, recovery bundles and brush presets are owned by this app's
//! sandbox. Native pickers and clipboard operations are injected by the shell.

use photocraft_codecs::{ChannelLayout, EncodeOptions, Image};
use photocraft_format::Autosaver;
use photocraft_ui_egui::Services;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::file_requests::FileRequests;

/// Attach asynchronous system pickers to the original PhotoCraft menus.
pub fn services_with_files(files_dir: &Path, requests: &FileRequests) -> Services {
    let mut hooks = services(files_dir);
    if let Some(export) = hooks.export.take() {
        let encoding = requests.clone();
        hooks.export = Some(Box::new(move |doc, path, settings| {
            let result = export(doc, path, settings)?;
            encoding.capture_encoding(path, std::sync::Arc::new(doc.clone()), settings.clone());
            Ok(result)
        }));
    }
    let open = requests.clone();
    hooks.pick_open = Some(Box::new(move || {
        open.pick_open();
        None
    }));
    let open = requests.clone();
    hooks.pick_open_intent = Some(Box::new(move |intent, target, params| {
        open.pick_open_with(intent, target, params);
        None
    }));
    let save = requests.clone();
    hooks.pick_save = Some(Box::new(move |suggested| save.pick_save(suggested)));
    let write = requests.clone();
    hooks.write = Some(Box::new(move |path, bytes| write.write(path, bytes)));
    let deferred = requests.clone();
    hooks.defer_save = Some(Box::new(move |document, revision, path| {
        deferred.defer_save(document, revision, path)
    }));
    let copy = requests.clone();
    let published = requests.clone();
    hooks.save_in_place = Some(Box::new(move |path| published.is_published_save(path)));
    hooks.save_copy_ready = Some(Box::new(move |path| copy.save_copy_ready(path)));
    let quick = requests.clone();
    hooks.quick_export = Some(Box::new(move |session| quick.quick_export(session)));
    hooks
}

/// Build platform hooks without depending on the desktop app's private modules.
pub fn services(files_dir: &Path) -> Services {
    let root = data_root(files_dir);
    let load_root = root.clone();
    let save_root = root.clone();
    let autosave_root = root.clone();
    let recover_root = root.clone();
    let savers: Rc<RefCell<HashMap<u64, Autosaver>>> = Rc::default();
    let discard_savers = savers.clone();
    let preset_store = root
        .as_ref()
        .ok()
        .map(|root| photocraft_engine::preset_store::open_dir_async(root.join("Presets")));

    Services {
        import: Some(Box::new(|name, bytes| {
            guard("Open", || {
                photocraft_io::import(name, bytes)
                    .map(|result| (result.document, result.warnings))
                    .map_err(|error| error.to_string())
            })
        })),
        export: Some(Box::new(|doc, path, settings| {
            guard("Export", || {
                let mut options = photocraft_io::ExportOptions::default();
                if let Some(quality) = settings.jpeg_quality {
                    options.encode.jpeg_quality = quality;
                }
                photocraft_io::export(doc, path, &options)
                    .map(|result| (result.bytes, result.warnings))
                    .map_err(|error| error.to_string())
            })
        })),
        encode_png: Some(Box::new(|width, height, rgba| {
            guard("PNG encoding", || {
                let image = Image::from_u8(width, height, ChannelLayout::Rgba, rgba.to_vec())
                    .map_err(|error| error.to_string())?;
                photocraft_codecs::encode(
                    &image,
                    photocraft_codecs::Format::Png,
                    &EncodeOptions::default(),
                )
                .map_err(|error| error.to_string())
            })
        })),
        write: Some(Box::new(|path, bytes| {
            let path = absolute_path(path)?;
            photocraft_format::atomic_write(path, bytes).map_err(|error| error.to_string())
        })),
        load_prefs: Some(Box::new(move || {
            let root = load_root.as_ref().ok()?;
            std::fs::read_to_string(root.join("preferences.json")).ok()
        })),
        save_prefs: Some(Box::new(move |text| {
            let root = save_root.as_ref().map_err(Clone::clone)?;
            photocraft_format::atomic_write(&root.join("preferences.json"), text.as_bytes())
                .map_err(|error| error.to_string())
        })),
        autosave: Some(Box::new(move |doc, revision, path| {
            guard("Autosave", || {
                let root = autosave_root.as_ref().map_err(Clone::clone)?;
                let mut savers = savers
                    .try_borrow_mut()
                    .map_err(|_| "Recovery service is busy".to_string())?;
                let saver = savers.entry(doc.id.0).or_insert_with(|| {
                    Autosaver::new(root.join("Recovery"), &format!("doc-{}", doc.id.0))
                });
                let previous_result = saver.last_result();
                saver.request(
                    doc.clone(),
                    revision,
                    path.map(str::to_string),
                    Default::default(),
                );
                previous_result.transpose().map(|_| ())
            })
        })),
        discard_autosave: Some(Box::new(move |id| {
            let saver = discard_savers
                .try_borrow_mut()
                .ok()
                .and_then(|mut savers| savers.remove(&id));
            if let Some(saver) = saver {
                let _ = saver.discard();
            }
        })),
        recover: Some(Box::new(move || {
            let Ok(root) = &recover_root else {
                return Vec::new();
            };
            let dir = root.join("Recovery");
            let mut recovered = Vec::new();
            for entry in photocraft_format::list_recovery(&dir) {
                if let Ok(doc) = guard("Recovery", || {
                    photocraft_format::recover(&entry).map_err(|error| error.to_string())
                }) {
                    recovered.push((entry.info.original_path.clone(), doc));
                    // The baseline UI gives recovered documents new ids and autosaves them
                    // again. Keep unreadable entries so a failed recovery cannot erase data.
                    let _ = photocraft_format::discard_recovery(&dir, &entry);
                }
            }
            recovered
        })),
        append_text: Some(Box::new(|path, text| {
            use std::io::Write;
            let path = absolute_path(path)?;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|error| error.to_string())?;
            file.write_all(text.as_bytes())
                .map_err(|error| error.to_string())
        })),
        preset_store,
        ..Default::default()
    }
}

fn data_root(files_dir: &Path) -> Result<PathBuf, String> {
    if !files_dir.is_absolute() {
        return Err("PhotoCraft requires the UIAbility's absolute filesDir".into());
    }
    let root = files_dir.join("PhotoCraft");
    std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    Ok(root)
}

/// File handles supplied by the native picker are translated by the shell. A
/// relative name must never depend on the native process's working directory.
fn absolute_path(path: &str) -> Result<&Path, String> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return Err("Save requires an absolute path authorized by the shell".into());
    }
    Ok(path)
}

fn guard<T>(operation: &str, work: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).unwrap_or_else(|_| {
        Err(format!(
            "{operation} failed with an internal error; your open documents are unchanged"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(label: &str) -> Result<Self, String> {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "photocraft-ohos-services-{label}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).map_err(|error| error.to_string())?;
            Ok(Self(path))
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn png_import_export_preserves_pixels() -> Result<(), String> {
        let root = TempRoot::new("png")?;
        let mut hooks = services(&root.0);
        let rgba = [255, 0, 0, 255, 0, 128, 255, 255];
        let png = hooks.encode_png.as_ref().ok_or("missing PNG encoder")?(2, 1, &rgba)?;
        let (doc, _) = hooks.import.as_ref().ok_or("missing importer")?("sample.png", &png)?;
        let (exported, _) = hooks.export.as_ref().ok_or("missing exporter")?(
            &doc,
            "sample.png",
            &Default::default(),
        )?;
        let image = photocraft_codecs::decode(&exported).map_err(|error| error.to_string())?;
        assert_eq!((image.width(), image.height()), (2, 1));
        assert_eq!(image.to_rgba8(), rgba);
        let saved = root.0.join("sample.png");
        hooks.write.as_mut().ok_or("missing writer")?(
            saved.to_str().ok_or("non-UTF8 test path")?,
            &exported,
        )?;
        assert_eq!(
            std::fs::read(saved).map_err(|error| error.to_string())?,
            exported
        );
        Ok(())
    }

    #[test]
    fn preferences_are_isolated_in_the_ability_sandbox() -> Result<(), String> {
        let a = TempRoot::new("prefs-a")?;
        let b = TempRoot::new("prefs-b")?;
        let mut first = services(&a.0);
        let mut second = services(&b.0);
        assert!(first.load_prefs.as_mut().ok_or("missing loader")?().is_none());
        let text = "{\"interface\":{\"language\":\"zh-CN\"}}";
        first.save_prefs.as_mut().ok_or("missing saver")?(text)?;
        assert_eq!(
            first.load_prefs.as_mut().ok_or("missing loader")?(),
            Some(text.into())
        );
        assert!(second.load_prefs.as_mut().ok_or("missing loader")?().is_none());
        assert_eq!(
            std::fs::read_to_string(a.0.join("PhotoCraft/preferences.json"))
                .map_err(|error| error.to_string())?,
            text
        );
        Ok(())
    }

    #[test]
    fn relative_sandbox_or_save_paths_report_errors() -> Result<(), String> {
        let mut hooks = services(Path::new("relative"));
        assert!(hooks.save_prefs.as_mut().ok_or("missing saver")?("{}").is_err());
        assert!(hooks.write.as_mut().ok_or("missing writer")?("relative.png", b"image").is_err());
        Ok(())
    }

    #[test]
    fn escaped_import_export_panics_become_errors() {
        let result: Result<(), String> = guard("Open", || {
            std::panic::resume_unwind(Box::new("synthetic codec failure"));
        });
        assert!(result.is_err());
    }
}
