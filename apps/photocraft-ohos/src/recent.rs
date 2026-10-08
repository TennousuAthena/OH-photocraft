//! Recent files describe external sources. Cached imports are never treated as fresh sources.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use photocraft_ui_egui::{PhotocraftApp, RecentFileInfo, Services};
use serde::Deserialize;

use crate::file_requests::FileRequests;

#[derive(Clone)]
pub struct RecentSources {
    documents: PathBuf,
    sources: Rc<RefCell<HashMap<String, String>>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    path: String,
    uri: String,
    #[serde(default)]
    folder: Option<FolderBinding>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FolderBinding {
    folder_uri: String,
    relative_path: String,
}

impl RecentSources {
    pub fn new(files: &Path) -> Self {
        Self {
            documents: files.join("PhotoCraft/Documents"),
            sources: Rc::default(),
        }
    }

    pub fn attach(&self, services: &mut Services, files: &FileRequests) {
        let catalog = self.clone();
        services.recent_file = Some(Box::new(move |path| catalog.info(path)));
        let requests = files.clone();
        services.open_recent = Some(Box::new(move |path| requests.open_recent(path)));
    }

    pub fn refresh(&self, app: &mut PhotocraftApp) -> Result<(), String> {
        self.reload()?;
        app.sync_recent();
        Ok(())
    }

    fn reload(&self) -> Result<(), String> {
        let path = self.documents.join("source-uris.json");
        if !path.exists() {
            self.sources.borrow_mut().clear();
            return Ok(());
        }
        self.inspect(&path, false)?;
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|_| "Recent source records cannot be read")?;
        if !metadata.is_file() || metadata.len() > 1_048_576 {
            return Err("Recent source records are invalid or too large".into());
        }
        let mut bytes = Vec::new();
        std::fs::File::open(&path)
            .map_err(|_| "Recent source records cannot be read")?
            .take(1_048_577)
            .read_to_end(&mut bytes)
            .map_err(|_| "Recent source records cannot be read")?;
        if bytes.len() > 1_048_576 {
            return Err("Recent source records are too large".into());
        }
        let bindings: Vec<Binding> = serde_json::from_slice(&bytes)
            .map_err(|_| "Recent source records are damaged; open the file again")?;
        if bindings.len() > 4096 {
            return Err("Recent source records contain too many files".into());
        }
        let mut sources = HashMap::new();
        for binding in bindings {
            let identity = if let Some(folder) = binding.folder {
                if !binding.uri.is_empty()
                    || folder.folder_uri.is_empty()
                    || folder.folder_uri.len() > 8192
                    || folder.folder_uri.contains('\0')
                    || folder.relative_path.len() > 8192
                    || folder.relative_path.contains(['\0', '\\'])
                    || folder.relative_path.split('/').count() > 32
                    || folder
                        .relative_path
                        .split('/')
                        .any(|part| part.is_empty() || part == "." || part == "..")
                {
                    return Err("Recent folder source identity is invalid".into());
                }
                serde_json::json!([folder.folder_uri, folder.relative_path]).to_string()
            } else {
                if binding.uri.is_empty() || binding.uri.len() > 8192 || binding.uri.contains('\0')
                {
                    return Err("Recent source identity is invalid".into());
                }
                binding.uri
            };
            self.owned_file(&binding.path)?;
            if sources.insert(binding.path, identity).is_some() {
                return Err("Recent source records contain duplicate paths".into());
            }
        }
        *self.sources.borrow_mut() = sources;
        Ok(())
    }

    fn info(&self, path: &str) -> RecentFileInfo {
        let name = Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Recent document")
            .to_string();
        let identity = self
            .sources
            .borrow()
            .get(path)
            .map_or_else(|| format!("unbound:{path}"), |uri| format!("source:{uri}"));
        RecentFileInfo {
            identity,
            tooltip: name.clone(),
            name,
            location: "System file".into(),
        }
    }

    fn owned_file(&self, text: &str) -> Result<(), String> {
        let path = Path::new(text);
        let relative = path
            .strip_prefix(&self.documents)
            .map_err(|_| "Recent source is outside document storage")?;
        let parts: Vec<_> = relative.components().collect();
        if parts.len() != 3
            || parts
                .iter()
                .any(|part| !matches!(part, Component::Normal(_)))
            || !matches!(
                parts.first().and_then(|part| part.as_os_str().to_str()),
                Some("staging" | "imports")
            )
            || text.len() > 8192
            || text.contains('\0')
        {
            return Err("Recent source has invalid ownership".into());
        }
        self.inspect(path, true)
    }

    fn inspect(&self, path: &Path, allow_missing: bool) -> Result<(), String> {
        let relative = path
            .strip_prefix(&self.documents)
            .map_err(|_| "Recent source has invalid ownership")?;
        if let Some(parent) = self.documents.parent() {
            let metadata = std::fs::symlink_metadata(parent)
                .map_err(|_| "Recent source storage cannot be inspected")?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err("Recent source storage contains a link or invalid directory".into());
            }
        }
        let mut current = self.documents.clone();
        for part in std::iter::once(None).chain(relative.components().map(Some)) {
            if let Some(part) = part {
                current.push(part.as_os_str());
            }
            match std::fs::symlink_metadata(&current) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err("Recent source contains a symbolic link".into());
                }
                Ok(meta) if current != path && !meta.is_dir() => {
                    return Err("Recent source has invalid storage".into());
                }
                Ok(_) => {}
                Err(error) if allow_missing && error.kind() == std::io::ErrorKind::NotFound => {
                    break;
                }
                Err(_) => return Err("Recent source storage cannot be inspected".into()),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_engine::Session;
    use serde_json::{Value, json};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture {
        root: PathBuf,
        catalog: RecentSources,
        files: FileRequests,
        app: PhotocraftApp,
    }
    impl Fixture {
        fn new() -> Result<Self, String> {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "photocraft-recent-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
            let files = FileRequests::new(&root)?;
            let catalog = RecentSources::new(&root);
            let mut services = crate::services::services_with_files(&root, &files);
            catalog.attach(&mut services, &files);
            let app = PhotocraftApp::new(Session::new(), services);
            Ok(Self {
                root,
                catalog,
                files,
                app,
            })
        }
        fn path(&self, area: &str, id: &str, name: &str) -> Result<String, String> {
            let path = self.catalog.documents.join(area).join(id).join(name);
            std::fs::create_dir_all(path.parent().ok_or("path missing")?)
                .map_err(|e| e.to_string())?;
            Ok(path.to_string_lossy().into_owned())
        }
        fn records(&self, records: Value) -> Result<(), String> {
            std::fs::write(
                self.catalog.documents.join("source-uris.json"),
                records.to_string(),
            )
            .map_err(|e| e.to_string())
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn recent_logical_identity_deduplicates_saved_imports_and_original_menu_requests_fresh_source()
    -> Result<(), String> {
        let mut f = Fixture::new()?;
        let old = f.path("staging", "1", "Actual.psd")?;
        let fresh = f.path("imports", "2-3-0", "Actual.psd")?;
        // Cached files may already be gone; a valid private source remains reopenable.
        f.records(json!([{ "path":old,"uri":"content://source/1" },{ "path":fresh,"uri":"content://source/1" }]))?;
        f.app
            .session
            .prefs
            .edit(|prefs| prefs.file_handling.recent_files = vec![fresh.clone(), old.clone()]);
        f.catalog.refresh(&mut f.app)?;
        assert_eq!(f.app.ui.recent_files, vec![fresh.clone()]);
        let items = photocraft_ui_egui::menus::menu_items(&f.app);
        assert_eq!(
            items
                .iter()
                .find(|item| item.id == "file.openRecent.0")
                .ok_or("recent missing")?
                .label,
            "Actual.psd"
        );
        fn text(shape: &egui::epaint::Shape, output: &mut String) {
            match shape {
                egui::epaint::Shape::Text(value) => {
                    output.push_str(value.galley.text());
                    output.push('\n');
                }
                egui::epaint::Shape::Vec(values) => {
                    for value in values {
                        text(value, output);
                    }
                }
                _ => {}
            }
        }
        let ctx = egui::Context::default();
        f.app.ui.theme = photocraft_ui_egui::theme::ThemeKind::Pro;
        PhotocraftApp::setup_context(&ctx, f.app.ui.theme);
        let mut frame = eframe::Frame::_new_kittest();
        let mut visible = String::new();
        for _ in 0..4 {
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 800.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    eframe::App::logic(&mut f.app, ui.ctx(), &mut frame);
                    eframe::App::ui(&mut f.app, ui, &mut frame);
                },
            );
            visible.clear();
            for shape in &output.shapes {
                text(&shape.shape, &mut visible);
            }
            output.drop_without_applying_deltas();
        }
        assert!(visible.contains("Actual.psd"));
        assert!(visible.contains("System file"));
        assert!(!visible.contains(&f.root.to_string_lossy().into_owned()));
        assert!(!visible.contains("content://source"));
        photocraft_ui_egui::menus::invoke(&mut f.app, &ctx, "file.openRecent.0", json!({}))?;
        assert!(
            f.app.session.documents().is_empty(),
            "Recent must not open the retained cache"
        );
        let request: Value =
            serde_json::from_str(&f.files.take_json()).map_err(|e| e.to_string())?;
        assert_eq!(request["intent"], "file.openRecent");
        assert_eq!(request["chooseDestination"], false);
        assert_eq!(request["previousPath"], fresh);
        let info = f.app.recent_file_info(&old);
        assert!(!info.tooltip.contains('/'));
        assert!(!info.location.contains('/'));
        Ok(())
    }

    #[test]
    fn recent_cancel_failure_and_fresh_import_preserve_the_original_dirty_document()
    -> Result<(), String> {
        let mut f = Fixture::new()?;
        f.app.run(
            "file.new",
            json!({"width":3,"height":2,"name":"Working.psd"}),
        )?;
        let original = f.app.session.active().ok_or("working missing")?.clone();
        let previous = f.path("imports", "1-2-0", "Fresh.png")?;
        let ctx = egui::Context::default();
        for status in ["cancel", "error"] {
            f.app.open_recent(&previous)?;
            let request: Value =
                serde_json::from_str(&f.files.take_json()).map_err(|e| e.to_string())?;
            f.files.complete_with_path(
                &mut f.app,
                &ctx,
                request["id"].as_u64().ok_or("id missing")?,
                status,
                "",
                "",
                "Permission expired",
            )?;
            let doc = f.app.session.active().ok_or("working lost")?;
            assert_eq!(doc.doc.id, original.doc.id);
            assert_eq!(doc.is_dirty(), original.is_dirty());
            assert_eq!(doc.path, original.path);
        }
        let fresh = f.path("imports", "3-4-0", "Fresh.png")?;
        let mut source = Session::new();
        source
            .execute(
                "file.new",
                json!({"width":5,"height":4,"background":"#00ff00"}),
            )
            .map_err(|e| e.to_string())?;
        let bytes = photocraft_io::export(
            &source.active().ok_or("source missing")?.doc,
            "Fresh.png",
            &Default::default(),
        )
        .map_err(|e| e.to_string())?
        .bytes;
        std::fs::write(&fresh, bytes).map_err(|e| e.to_string())?;
        f.app.open_recent(&previous)?;
        let request: Value =
            serde_json::from_str(&f.files.take_json()).map_err(|e| e.to_string())?;
        f.files.complete_with_path(
            &mut f.app,
            &ctx,
            request["id"].as_u64().ok_or("id missing")?,
            "success",
            &fresh,
            "Fresh.png",
            "",
        )?;
        assert_eq!(f.app.session.documents().len(), 2);
        assert_eq!(
            f.app
                .session
                .active()
                .ok_or("fresh missing")?
                .doc
                .size
                .width,
            5
        );
        assert_eq!(
            f.app
                .session
                .documents()
                .first()
                .ok_or("working lost")?
                .doc
                .id,
            original.doc.id
        );
        Ok(())
    }

    #[test]
    fn source_catalog_rejects_traversal_links_and_oversized_records() -> Result<(), String> {
        let f = Fixture::new()?;
        f.records(json!([{ "path":format!("{}/imports/../staging/Bad.png",f.catalog.documents.display()),"uri":"content://source/1" }]))?;
        assert!(f.catalog.reload().is_err());
        let imports = f.catalog.documents.join("imports");
        std::os::unix::fs::symlink(&f.root, &imports).map_err(|e| e.to_string())?;
        f.records(json!([{ "path":imports.join("1-2-0/Bad.png"),"uri":"content://source/1" }]))?;
        assert!(f.catalog.reload().is_err());
        std::fs::write(
            f.catalog.documents.join("source-uris.json"),
            vec![b' '; 1_048_577],
        )
        .map_err(|e| e.to_string())?;
        assert!(f.catalog.reload().is_err());
        Ok(())
    }
    #[test]
    fn folder_provenance_coexists_with_single_file_recent_and_rejects_ambiguous_or_unsafe_identity()
    -> Result<(), String> {
        let mut f = Fixture::new()?;
        let first = f.path("imports", "1", "one.png")?;
        let second = f.path("imports", "2", "one.png")?;
        let single = f.path("staging", "3", "saved.png")?;
        let records = json!([
            {"path":first,"uri":"","folder":{"folderUri":"content://private-folder","relativePath":"sub/one.png"}},
            {"path":second,"uri":"","folder":{"folderUri":"content://private-folder","relativePath":"sub/one.png"}},
            {"path":single,"uri":"content://private-file"}
        ]);
        f.records(records.clone())?;
        f.catalog.refresh(&mut f.app)?;
        assert_eq!(
            f.catalog.info(&first).identity,
            f.catalog.info(&second).identity
        );
        assert_ne!(
            f.catalog.info(&first).identity,
            f.catalog.info(&single).identity
        );
        assert_eq!(f.catalog.info(&first).location, "System file");
        for invalid in [
            json!([{ "path":first,"uri":"content://file","folder":{"folderUri":"content://folder","relativePath":"one.png"}}]),
            json!([{ "path":first,"uri":"","folder":{"folderUri":"content://folder","relativePath":"../one.png"}}]),
            json!([records[0], records[0]]),
        ] {
            f.records(invalid)?;
            assert!(f.catalog.refresh(&mut f.app).is_err());
            assert_eq!(
                f.catalog.info(&first).identity,
                f.catalog.info(&second).identity,
                "bad records cannot replace last validated catalog"
            );
        }
        Ok(())
    }
}
