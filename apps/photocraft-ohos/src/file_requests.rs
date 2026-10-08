//! Asynchronous system pickers and publication of encoded sandbox files.
//!
//! This object and the application live on the render worker. Services share it
//! without blocking ArkUI or granting the engine access to external file URIs.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use photocraft_engine::Session;
use photocraft_ui_egui::{PhotocraftApp, i18n};

#[derive(Clone)]
pub struct FileRequests {
    staging: PathBuf,
    published_file: PathBuf,
    state: Rc<RefCell<State>>,
    slot: crate::source_files::OperationSlot,
    sources: crate::source_files::SourceFiles,
}

#[derive(Default)]
struct State {
    pending: Option<Pending>,
    error: Option<String>,
    history_restore_error: Option<String>,
    encoding: Option<Encoding>,
    published: HashMap<String, u64>,
}

#[derive(Clone)]
struct Encoding {
    path: String,
    document: Arc<photocraft_doc::Document>,
    settings: photocraft_ui_egui::ExportSettings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Open,
    Save,
}

struct Pending {
    _reservation: crate::source_files::Reservation,
    id: u64,
    kind: Kind,
    path: String,
    suggested_name: String,
    previous_path: String,
    choose_destination: bool,
    ready: bool,
    finalized: bool,
    delivered: bool,
    document: Option<SavedDocument>,
    intent: String,
    target: Option<u64>,
    params: serde_json::Value,
    encoding: Option<Encoding>,
    allow_format_choice: bool,
}

#[derive(Clone, Debug)]
struct SavedDocument {
    id: u64,
    revision: u64,
}

impl FileRequests {
    pub fn new(files: &Path) -> Result<Self, String> {
        if !files.is_absolute() {
            return Err("file staging requires an absolute filesDir".into());
        }
        let staging = files.join("PhotoCraft/Documents/staging");
        std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
        let staging = staging.canonicalize().map_err(|e| e.to_string())?;
        let slot = crate::source_files::OperationSlot::default();
        let sources = crate::source_files::SourceFiles::new(files, &staging, slot.clone())?;
        let requests = Self {
            published_file: staging
                .parent()
                .ok_or("file staging directory has no parent")?
                .join("published-saves.json"),
            staging,
            state: Rc::default(),
            slot,
            sources,
        };
        if let Err(error) = requests.restore_published() {
            requests.state.borrow_mut().history_restore_error = Some(error);
        }
        Ok(requests)
    }

    pub fn is_published_save(&self, path: &str) -> bool {
        self.state.borrow().published.get(path).is_some_and(|size| {
            self.stage_file_size(Path::new(path))
                .is_ok_and(|actual| actual == *size)
        })
    }

    // Imported/failed/copied files never grant in-place authority merely because
    // they exist. Only successful external Save completions enter this journal.
    fn remember_published(&self, path: &str) -> Result<(), String> {
        let size = self.stage_file_size(Path::new(path))?;
        self.state.borrow_mut().published.insert(path.into(), size);
        let mut entries = self
            .state
            .borrow()
            .published
            .iter()
            .filter_map(|(path, size)| {
                let relative = Path::new(path).strip_prefix(&self.staging).ok()?;
                Some((relative.to_string_lossy().into_owned(), *size))
            })
            .collect::<Vec<_>>();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        if entries.len() > MAX_PUBLISHED_SAVES {
            entries.drain(..entries.len() - MAX_PUBLISHED_SAVES);
        }
        self.state.borrow_mut().published.retain(|path, _| {
            Path::new(path)
                .strip_prefix(&self.staging)
                .is_ok_and(|relative| {
                    entries
                        .binary_search_by(|(path, _)| {
                            path.as_str().cmp(&relative.to_string_lossy())
                        })
                        .is_ok()
                })
        });
        let entries = entries
            .into_iter()
            .map(|(path, size)| serde_json::json!({"path":path, "size":size}))
            .collect::<Vec<_>>();
        let bytes = serde_json::to_vec(&serde_json::json!({"version":1,"saves":entries}))
            .map_err(|e| e.to_string())?;
        if bytes.len() > MAX_PUBLISHED_JSON {
            return Err("saved destination history exceeds its size limit".into());
        }
        photocraft_format::atomic_write(&self.published_file, &bytes).map_err(|e| e.to_string())
    }

    fn restore_published(&self) -> Result<(), String> {
        let metadata = match std::fs::symlink_metadata(&self.published_file) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.to_string()),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("saved destination history must be a regular sandbox file".into());
        }
        if metadata.len() > MAX_PUBLISHED_JSON as u64 {
            return Err("saved destination history exceeds its size limit".into());
        }
        let mut bytes = Vec::new();
        std::fs::File::open(&self.published_file)
            .map_err(|e| e.to_string())?
            .take(MAX_PUBLISHED_JSON as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > MAX_PUBLISHED_JSON {
            return Err("saved destination history exceeds its size limit".into());
        }
        let journal: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if journal["version"].as_u64() != Some(1) {
            return Err("saved destination history has an unsupported version".into());
        }
        let saves = journal["saves"]
            .as_array()
            .ok_or("saved destination history needs a saves array")?;
        if saves.len() > MAX_PUBLISHED_SAVES {
            return Err("saved destination history contains too many entries".into());
        }
        let mut restored = HashMap::new();
        for entry in saves {
            let relative = entry["path"]
                .as_str()
                .ok_or("saved destination entry needs a path")?;
            if relative.len() > MAX_PUBLISHED_PATH || Path::new(relative).is_absolute() {
                return Err("saved destination entry has an invalid path".into());
            }
            let size = entry["size"]
                .as_u64()
                .filter(|size| *size > 0)
                .ok_or("saved destination entry needs a positive file size")?;
            let path = self.staging.join(relative);
            if self.stage_file_size(&path)? != size {
                return Err("saved destination file changed since publication".into());
            }
            if restored
                .insert(path.to_string_lossy().into_owned(), size)
                .is_some()
            {
                return Err("saved destination history repeats a path".into());
            }
        }
        self.state.borrow_mut().published = restored;
        Ok(())
    }

    fn stage_file_size(&self, path: &Path) -> Result<u64, String> {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .ok_or("saved destination has no document extension")?;
        if !SAVE_EXTENSIONS.contains(&extension.as_str()) {
            return Err("saved destination has an unsupported document extension".into());
        }
        self.regular_stage_file_size(path)
    }

    fn regular_stage_file_size(&self, path: &Path) -> Result<u64, String> {
        let relative = path
            .strip_prefix(&self.staging)
            .map_err(|_| "saved destination must be in Documents/staging")?;
        let parts = relative.components().collect::<Vec<_>>();
        let [
            std::path::Component::Normal(id),
            std::path::Component::Normal(name),
        ] = parts.as_slice()
        else {
            return Err("saved destination requires one request directory and file".into());
        };
        let id = id
            .to_str()
            .ok_or("saved destination request id is not UTF-8")?;
        let number = id
            .parse::<u64>()
            .map_err(|_| "invalid saved destination request id")?;
        if number == 0 || number > 9_007_199_254_740_991 || number.to_string() != id {
            return Err("invalid saved destination request id".into());
        }
        let name = name.to_str().ok_or("saved destination name is not UTF-8")?;
        if name.len() > 240 || basename(name) != name {
            return Err("invalid saved destination name".into());
        }
        let directory = path.parent().ok_or("saved destination has no parent")?;
        let parent = std::fs::symlink_metadata(directory).map_err(|e| e.to_string())?;
        let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if !parent.is_dir()
            || parent.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() == 0
            || path.canonicalize().map_err(|e| e.to_string())?.parent() != Some(directory)
            || directory
                .canonicalize()
                .map_err(|e| e.to_string())?
                .parent()
                != Some(self.staging.as_path())
        {
            return Err("saved destination must be a regular retained staging file".into());
        }
        Ok(metadata.len())
    }

    pub fn is_pending(&self) -> bool {
        self.slot.is_pending()
    }

    pub fn attach_sources(&self, session: &mut Session, ctx: &egui::Context) {
        self.sources.attach(session, ctx);
    }

    pub fn pick_open(&self) {
        self.pick_open_for("file.open", None);
    }

    pub fn pick_open_for(&self, intent: &str, target: Option<u64>) {
        self.pick_open_with(intent, target, &serde_json::Value::Null);
    }

    pub fn pick_open_with(&self, intent: &str, target: Option<u64>, params: &serde_json::Value) {
        let mut state = self.state.borrow_mut();
        let reservation = match self.slot.reserve() {
            Ok(reservation) => reservation,
            Err(error) => {
                state.error = Some(error);
                return;
            }
        };
        state.error = None;
        state.history_restore_error = None;
        state.pending = Some(Pending {
            _reservation: reservation,
            id: next_id(),
            kind: Kind::Open,
            path: String::new(),
            suggested_name: String::new(),
            previous_path: String::new(),
            choose_destination: true,
            ready: true,
            finalized: true,
            delivered: false,
            document: None,
            intent: intent.into(),
            target,
            params: params.clone(),
            encoding: None,
            allow_format_choice: false,
        });
    }

    pub fn open_recent(&self, previous_path: &str) -> Result<(), String> {
        if self.is_pending() {
            return Err("Finish the current file operation first".into());
        }
        if previous_path.is_empty() || previous_path.len() > 8192 || previous_path.contains('\0') {
            return Err("Recent file identity is invalid".into());
        }
        self.pick_open();
        if let Some(pending) = self.state.borrow_mut().pending.as_mut() {
            pending.previous_path = previous_path.into();
            pending.choose_destination = false;
        }
        Ok(())
    }

    pub fn pick_save(&self, suggested: &str) -> Option<String> {
        match self.begin_save(suggested, None) {
            Ok(path) => Some(path),
            Err(error) => {
                self.state.borrow_mut().error = Some(error);
                None
            }
        }
    }

    pub fn stage_export(&self, suggested: &str, intent: &str) -> Result<String, String> {
        let path = self.begin_save(suggested, None)?;
        if let Some(pending) = self.state.borrow_mut().pending.as_mut() {
            pending.intent = intent.into();
        }
        Ok(path)
    }

    fn begin_save(&self, suggested: &str, previous: Option<&str>) -> Result<String, String> {
        let mut state = self.state.borrow_mut();
        let reservation = self.slot.reserve()?;
        state.error = None;
        state.history_restore_error = None;
        let name = basename(suggested);
        let (id, dir) = loop {
            let id = next_id();
            let dir = self.staging.join(id.to_string());
            match std::fs::create_dir(&dir) {
                Ok(()) => break (id, dir),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.to_string()),
            }
        };
        let path = dir.join(&name).to_string_lossy().into_owned();
        state.pending = Some(Pending {
            _reservation: reservation,
            id,
            kind: Kind::Save,
            path: path.clone(),
            suggested_name: name,
            previous_path: previous.unwrap_or_default().to_string(),
            choose_destination: previous.is_none(),
            ready: false,
            finalized: false,
            delivered: false,
            document: None,
            intent: "file.export".into(),
            target: None,
            params: serde_json::Value::Null,
            encoding: None,
            allow_format_choice: false,
        });
        Ok(path)
    }

    pub fn write(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let original = Path::new(path);
        if !original.is_absolute()
            || original
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err("Save requires an absolute sandbox path".into());
        }
        let matching = {
            let state = self.state.borrow();
            match &state.pending {
                Some(p) if p.kind == Kind::Save && p.path == path && !p.delivered => {
                    Some(p.path.clone())
                }
                Some(_) => return Err("Finish the current file operation first".into()),
                None => None,
            }
        };
        // An in-place save writes a fresh stage so a cancelled picker or failed
        // external publication cannot overwrite the last successfully saved copy.
        let target = match matching {
            Some(target) => target,
            None => self.begin_save(path, Some(path))?,
        };
        if let Err(error) = photocraft_format::atomic_write(Path::new(&target), bytes) {
            self.abort_stage();
            return Err(error.to_string());
        }
        let mut state = self.state.borrow_mut();
        let encoding = state
            .encoding
            .take()
            .filter(|encoding| encoding.path == path);
        if let Some(pending) = &mut state.pending {
            pending.ready = true;
            pending.encoding = encoding;
        }
        Ok(())
    }

    pub fn defer_save(
        &self,
        document: u64,
        revision: u64,
        path: &str,
    ) -> Result<Option<String>, String> {
        let mut state = self.state.borrow_mut();
        let pending = state.pending.as_mut().ok_or("no staged save")?;
        if pending.kind != Kind::Save
            || !pending.ready
            || (pending.path != path && pending.previous_path != path)
        {
            return Err("staged save does not match the document writer".into());
        }
        pending.document = Some(SavedDocument {
            id: document,
            revision,
        });
        pending.intent = "file.save".into();
        pending.allow_format_choice = pending.choose_destination;
        Ok(Some(pending.path.clone()))
    }

    pub fn capture_encoding(
        &self,
        path: &str,
        document: Arc<photocraft_doc::Document>,
        settings: photocraft_ui_egui::ExportSettings,
    ) {
        self.state.borrow_mut().encoding = Some(Encoding {
            path: path.into(),
            document,
            settings,
        });
    }

    pub fn save_copy_ready(&self, path: &str) -> Result<(), String> {
        let mut state = self.state.borrow_mut();
        let pending = state.pending.as_mut().ok_or("no staged copy")?;
        if pending.kind != Kind::Save || pending.path != path || pending.document.is_some() {
            return Err("staged copy does not match".into());
        }
        pending.allow_format_choice = true;
        pending.intent = "file.saveACopy".into();
        Ok(())
    }

    /// Re-encode the immutable document captured by Services.export, preserving
    /// its settings and the revision bound by defer_save across later edits.
    pub fn prepare_save(
        &self,
        app: &mut PhotocraftApp,
        id: u64,
        destination_name: &str,
    ) -> Result<String, String> {
        if self.sources.has_request(id) {
            return self.sources.prepare_save(id, destination_name);
        }
        let (old_path, encoding, saved, allow_choice, original_name) = {
            let state = self.state.borrow();
            let pending = state
                .pending
                .as_ref()
                .filter(|pending| {
                    pending.id == id && pending.kind == Kind::Save && pending.delivered
                })
                .ok_or("save is no longer pending")?;
            (
                pending.path.clone(),
                pending.encoding.clone(),
                pending.document.clone(),
                pending.allow_format_choice,
                pending.suggested_name.clone(),
            )
        };
        let name = basename(destination_name);
        let extension = |name: &str| {
            Path::new(name)
                .extension()
                .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default()
        };
        let ext = extension(&name);
        let old_ext = extension(&original_name);
        if ext == old_ext && name == original_name {
            return Ok(old_path);
        }
        if ext != old_ext && (!allow_choice || !SAVE_EXTENSIONS.contains(&ext.as_str())) {
            return Err(format!(
                "save format .{ext} is not allowed for this request"
            ));
        }
        let path = Path::new(&old_path)
            .parent()
            .ok_or("save staging directory missing")?
            .join(&name)
            .to_string_lossy()
            .into_owned();
        if ext == old_ext {
            self.regular_stage_file_size(Path::new(&old_path))?;
            if std::fs::symlink_metadata(&path).is_ok() {
                return Err("chosen staging name already exists".into());
            }
            std::fs::rename(&old_path, &path).map_err(|error| error.to_string())?;
            if let Some(saved) = &saved
                && let Err(error) =
                    app.retarget_deferred_save(saved.id, saved.revision, &old_path, &path, None)
            {
                let _ = std::fs::rename(&path, &old_path);
                return Err(error);
            }
        } else {
            let encoding = encoding.ok_or("save snapshot is unavailable")?;
            let export = app
                .services
                .export
                .as_ref()
                .ok_or("no exporter configured")?;
            let (bytes, warnings) = export(&encoding.document, &path, &encoding.settings)?;
            photocraft_format::atomic_write(Path::new(&path), &bytes).map_err(|e| e.to_string())?;
            if let Some(saved) = &saved
                && let Err(error) = app.retarget_deferred_save(
                    saved.id,
                    saved.revision,
                    &old_path,
                    &path,
                    Some(warnings),
                )
            {
                cleanup(&path);
                return Err(error);
            }
            let _ = std::fs::remove_file(&old_path);
        }
        {
            let mut state = self.state.borrow_mut();
            state.encoding = None;
            let pending = state
                .pending
                .as_mut()
                .filter(|pending| pending.id == id)
                .ok_or("save is no longer pending")?;
            pending.path = path.clone();
            pending.suggested_name = name;
        }
        Ok(path)
    }

    pub fn quick_export(&self, session: &mut Session) -> Result<serde_json::Value, String> {
        let doc = session.active().ok_or("no active document")?;
        let stem = Path::new(&doc.doc.name)
            .file_stem()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into());
        let format = serde_json::to_value(session.prefs().export.quick_export_format)
            .map_err(|e| e.to_string())?;
        let format = format.as_str().unwrap_or("png");
        // A provider URI does not grant ambient access to its parent folder.
        // Always publish through a system-selected destination on OHOS.
        let path = self.begin_save(&format!("{stem}.{format}"), None)?;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            session.execute("file.export.quickExport", serde_json::json!({"path": path}))
        }))
        .map_err(|_| "Quick Export failed while encoding".to_string())
        .and_then(|result| result.map_err(|e| e.to_string()));
        match result {
            Ok(result) => Ok(result),
            Err(error) => {
                self.abort_stage();
                Err(error.to_string())
            }
        }
    }

    /// Detect direct engine writes (Quick Export) before publishing a request.
    /// Document state is held by the UI's deferred-save seam, never rolled back.
    pub fn finish_frame(&self, app: &mut PhotocraftApp) {
        self.sources.finish_frame(app);
        let mut state = self.state.borrow_mut();
        // Startup happens before the app has resolved its interface language.
        if let Some(error) = state.history_restore_error.take()
            && state.error.is_none()
        {
            state.error = Some(i18n::fmt(
                i18n::t("Saved destination history could not be restored: {error}; choose a destination again"),
                &[("error", &error)],
            ));
        }
        let Some(pending) = state.pending.as_mut() else {
            if let Some(error) = state.error.take() {
                app.ui.status = i18n::t(&error).to_string();
                app.ui.status_error = true;
            }
            return;
        };
        if pending.kind == Kind::Save && !pending.finalized {
            pending.ready |= std::fs::metadata(&pending.path)
                .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0);
            if !pending.ready {
                let path = pending.path.clone();
                state.pending = None;
                cleanup(&path);
                return;
            }
            pending.finalized = true;
        }
        app.ui.status = match pending.kind {
            Kind::Open => i18n::t("Choose a file to open").into(),
            Kind::Save => i18n::fmt(
                i18n::t("Waiting to publish {name}"),
                &[("name", &pending.suggested_name)],
            ),
        };
        app.ui.status_error = false;
        if let Some(error) = state.error.take() {
            app.ui.status = i18n::t(&error).to_string();
            app.ui.status_error = true;
        }
    }

    pub fn take_json(&self) -> String {
        let mut state = self.state.borrow_mut();
        let Some(pending) = state.pending.as_mut() else {
            return self.sources.take_json();
        };
        if !pending.ready || !pending.finalized || pending.delivered {
            return String::new();
        }
        pending.delivered = true;
        serde_json::json!({
            "id": pending.id,
            "kind": if pending.kind == Kind::Open { "open" } else { "save" },
            "path": pending.path,
            "suggestedName": pending.suggested_name,
            "chooseDestination": pending.choose_destination,
            "previousPath": pending.previous_path,
            "intent": if pending.intent == "file.open" && !pending.previous_path.is_empty() { "file.openRecent" } else { &pending.intent },
            "allowFormatChoice": pending.allow_format_choice,
            "allowedExtensions": if pending.allow_format_choice { SAVE_EXTENSIONS.iter().map(|ext| ext.to_string()).collect::<Vec<_>>() } else { Path::new(&pending.suggested_name).extension().map(|ext| vec![ext.to_string_lossy().to_ascii_lowercase()]).unwrap_or_default() },
        })
        .to_string()
    }

    #[cfg(test)]
    pub fn complete(
        &self,
        app: &mut PhotocraftApp,
        id: u64,
        result: &str,
        display_name: &str,
        error: &str,
    ) -> Result<(), String> {
        self.complete_with_path(
            app,
            &egui::Context::default(),
            id,
            result,
            "",
            display_name,
            error,
        )
    }

    // The explicit protocol fields mirror the five-argument C ABI and are kept
    // separate from the worker-owned app/context to avoid overloading strings.
    #[allow(clippy::too_many_arguments)]
    pub fn complete_with_path(
        &self,
        app: &mut PhotocraftApp,
        ctx: &egui::Context,
        id: u64,
        result: &str,
        resolved_path: &str,
        display_name: &str,
        error: &str,
    ) -> Result<(), String> {
        if self.sources.has_request(id) {
            return self
                .sources
                .complete(app, ctx, id, result, resolved_path, display_name, error);
        }
        if !matches!(result, "success" | "cancel" | "error") {
            return Err("unknown file operation result".into());
        }
        let pending = {
            let mut state = self.state.borrow_mut();
            match &state.pending {
                Some(p) if p.id == id && p.delivered => {}
                _ => return Err("file operation is no longer pending".into()),
            }
            state.error = None;
            state.history_restore_error = None;
            state.encoding = None;
            state.pending.take().ok_or("missing file operation")?
        };
        let name = if display_name.is_empty() {
            pending.suggested_name.as_str()
        } else {
            display_name
        };
        if let Some(document) = &pending.document
            && let Err(error) = app.complete_deferred_save(
                ctx,
                document.id,
                document.revision,
                &pending.path,
                result == "success",
            )
        {
            app.ui.status = i18n::t(&error).to_string();
            app.ui.status_error = true;
            return Err(error);
        }
        let remember_error = if result == "success" && pending.document.is_some() {
            self.remember_published(&pending.path).err()
        } else {
            None
        };
        match result {
            "success" => {
                if pending.kind == Kind::Open {
                    if let Err(error) = self.import_selection(app, &pending, resolved_path, name) {
                        app.ui.status = i18n::t(&error).to_string();
                        app.ui.status_error = true;
                        return Err(error);
                    }
                    let template = if pending.intent.starts_with("file.place") {
                        i18n::t("Placed {name}")
                    } else {
                        i18n::t("Opened {name}")
                    };
                    app.ui.status = i18n::fmt(template, &[("name", name)]);
                } else if pending.document.is_some() {
                    app.ui.status = i18n::fmt(i18n::t("Saved {name}"), &[("name", name)]);
                } else {
                    app.ui.status = i18n::fmt(i18n::t("Exported {name}"), &[("name", name)]);
                    cleanup(&pending.path);
                }
                app.ui.status_error = false;
            }
            "cancel" => {
                cleanup(&pending.path);
                app.ui.status = i18n::t("File operation cancelled").into();
                app.ui.status_error = false;
            }
            "error" => {
                if !self.retain_failed_stage(&pending, resolved_path) {
                    cleanup(&pending.path);
                }
                app.ui.status = if error.is_empty() {
                    i18n::t("File operation failed").into()
                } else {
                    i18n::t(error).to_string()
                };
                app.ui.status_error = true;
            }
            _ => {}
        }
        if let Some(error) = remember_error {
            // Publication already succeeded. Keep its saved revision and URI
            // binding, but make the inability to restore Ctrl+S next launch visible.
            app.ui.status = i18n::fmt(
                i18n::t("Saved {name}; destination history could not be stored: {error}"),
                &[("name", name), ("error", &error)],
            );
            app.ui.status_error = true;
        }
        Ok(())
    }

    // A failed URI rollback may leave this as the last encoded copy. The
    // completion cannot retain another request's file or any imported document.
    fn retain_failed_stage(&self, pending: &Pending, path: &str) -> bool {
        if pending.kind != Kind::Save || path.is_empty() || path != pending.path {
            return false;
        }
        let path = Path::new(path);
        let directory = self.staging.join(pending.id.to_string());
        if path.parent() != Some(directory.as_path()) {
            return false;
        }
        let Ok(metadata) = std::fs::symlink_metadata(path) else {
            return false;
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return false;
        }
        match (
            path.canonicalize(),
            directory.canonicalize(),
            self.staging.canonicalize(),
        ) {
            (Ok(file), Ok(directory), Ok(staging)) => {
                file.parent() == Some(directory.as_path())
                    && directory.parent() == Some(staging.as_path())
            }
            _ => false,
        }
    }

    fn import_selection(
        &self,
        app: &mut PhotocraftApp,
        pending: &Pending,
        path: &str,
        name: &str,
    ) -> Result<(), String> {
        let path_ref = Path::new(path);
        if !path_ref.is_absolute()
            || path_ref
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err("selected file must be staged in the app's filesDir".into());
        }
        let canonical = path_ref.canonicalize().map_err(|e| e.to_string())?;
        if !self
            .staging
            .ancestors()
            .nth(3)
            .is_some_and(|files| canonical.starts_with(files))
        {
            return Err("selected file must be staged in the app's filesDir".into());
        }
        let bytes = std::fs::read(path_ref).map_err(|e| e.to_string())?;
        match pending.intent.as_str() {
            "file.open" | "file.openAs" => {
                app.open_file(path, &bytes)?;
            }
            "edit.presets.exportImportPresets" => {
                let text =
                    String::from_utf8(bytes).map_err(|_| format!("{name} is not a preset file"))?;
                let mut params = if pending.params.is_object() {
                    pending.params.clone()
                } else {
                    serde_json::json!({})
                };
                params["action"] = serde_json::json!("import");
                params["data"] = serde_json::json!(text);
                app.run(&pending.intent, params)?;
            }
            "file.placeEmbedded"
            | "file.placeLinked"
            | "file.import.notes"
            | "file.scripts.browse" => {
                let active = app.session.active().map(|st| st.doc.id.0);
                if let Some(id) = pending.target {
                    let target = app
                        .session
                        .documents()
                        .iter()
                        .position(|st| st.doc.id.0 == id)
                        .ok_or("the target document was closed")?;
                    app.session.set_active(target);
                } else if pending.intent != "file.scripts.browse" {
                    return Err("the file operation has no target document".into());
                }
                let result = match pending.intent.as_str() {
                    "file.placeEmbedded" | "file.placeLinked" => {
                        let linked =
                            (pending.intent == "file.placeLinked").then(|| path.to_string());
                        photocraft_engine::file_cmds::place_bytes(
                            &mut app.session,
                            name,
                            bytes,
                            linked,
                            &serde_json::json!({}),
                        )
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                    }
                    "file.import.notes" => photocraft_engine::notes_cmds::import_notes_from(
                        &mut app.session,
                        name,
                        &bytes,
                    )
                    .map(|_| {
                        app.ui.analysis.notes = true;
                    })
                    .map_err(|e| e.to_string()),
                    "file.scripts.browse" => app
                        .run(
                            "file.scripts.browse",
                            serde_json::json!({"script": String::from_utf8_lossy(&bytes)}),
                        )
                        .map(|_| ()),
                    _ => Err("unsupported file intent".into()),
                };
                if let Some(index) = active.and_then(|id| {
                    app.session
                        .documents()
                        .iter()
                        .position(|st| st.doc.id.0 == id)
                }) {
                    app.session.set_active(index);
                }
                result?;
                app.sync_views();
            }
            intent => {
                return Err(format!(
                    "Asynchronous file import is not supported for {intent}"
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn abort_stage(&self) {
        if let Some(pending) = self.state.borrow_mut().pending.take() {
            cleanup(&pending.path);
        }
    }
}

fn basename(suggested: &str) -> String {
    let name = suggested.rsplit(['/', '\\']).next().unwrap_or_default();
    let mut name: String = name
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| {
            if matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    if name.len() > 240 {
        let suffix = name
            .rsplit_once('.')
            .filter(|(_, ext)| !ext.is_empty() && ext.len() <= 16)
            .map(|(_, ext)| format!(".{ext}"))
            .unwrap_or_default();
        let mut stem = name.strip_suffix(&suffix).unwrap_or(&name).to_string();
        while stem.len() + suffix.len() > 240 {
            stem.pop();
        }
        name = format!("{stem}{suffix}");
    }
    let name = name.trim().trim_matches('.');
    if name.is_empty() {
        "Untitled.psd".into()
    } else {
        name.into()
    }
}

fn cleanup(path: &str) {
    if path.is_empty() {
        return;
    }
    let path = Path::new(path);
    let _ = std::fs::remove_file(path);
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir(parent);
    }
}

pub(crate) fn next_id() -> u64 {
    static IDS: OnceLock<AtomicU64> = OnceLock::new();
    IDS.get_or_init(|| {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(1);
        AtomicU64::new(seed)
    })
    .fetch_add(1, Ordering::Relaxed)
}

/// Mirrors desktop Services::SAVE_FILTERS at the pinned upstream revision.
const SAVE_EXTENSIONS: &[&str] = &["psd", "psb", "pcraft", "png", "jpg", "tif", "tga", "exr"];
const MAX_PUBLISHED_JSON: usize = 1024 * 1024;
const MAX_PUBLISHED_SAVES: usize = 2048;
const MAX_PUBLISHED_PATH: usize = 280;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new() -> Result<Self, String> {
            let path = std::env::temp_dir().join(format!("photocraft-file-request-{}", next_id()));
            std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            Ok(Self(path))
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn app(root: &Path, requests: &FileRequests) -> Result<PhotocraftApp, String> {
        let mut app = PhotocraftApp::new(
            Session::new(),
            crate::services::services_with_files(root, requests),
        );
        app.run(
            "file.new",
            json!({"width": 2, "height": 2, "name": "original"}),
        )?;
        app.run("layer.new.layer", json!({}))?;
        Ok(app)
    }

    fn queued_save(
        app: &mut PhotocraftApp,
        requests: &FileRequests,
    ) -> Result<serde_json::Value, String> {
        app.save_as(None)?;
        requests.finish_frame(app);
        serde_json::from_str(&requests.take_json()).map_err(|e| e.to_string())
    }

    #[test]
    fn cancelled_or_failed_publication_never_marks_a_document_saved() -> Result<(), String> {
        let root = TempRoot::new()?;
        let requests = FileRequests::new(&root.0.join("cache"))?;
        let mut app = app(&root.0, &requests)?;
        let original = app.session.active().ok_or("no document")?.clone();
        for result in ["cancel", "error"] {
            let request = queued_save(&mut app, &requests)?;
            let state = app.session.active().ok_or("no document")?;
            assert_eq!(state.path, original.path);
            assert_eq!(state.saved_revision, original.saved_revision);
            assert!(state.is_dirty());
            requests.complete(
                &mut app,
                request["id"].as_u64().ok_or("no id")?,
                result,
                "original.psd",
                "publication failed",
            )?;
            let state = app.session.active().ok_or("no document")?;
            assert_eq!(state.path, original.path);
            assert_eq!(state.saved_revision, original.saved_revision);
            assert_eq!(state.doc.name, original.doc.name);
            assert!(state.is_dirty());
            assert_eq!(app.ui.status_error, result == "error");
            assert!(!Path::new(request["path"].as_str().ok_or("no path")?).exists());
        }
        Ok(())
    }

    #[test]
    fn completion_targets_the_original_document_and_encoded_revision() -> Result<(), String> {
        let root = TempRoot::new()?;
        let requests = FileRequests::new(&root.0.join("cache"))?;
        let mut app = app(&root.0, &requests)?;
        let saved_id = app.session.active().ok_or("no document")?.doc.id.0;
        let encoded_revision = app.session.active().ok_or("no document")?.revision;
        let request = queued_save(&mut app, &requests)?;
        app.run("layer.new.layer", json!({}))?;
        app.run(
            "file.new",
            json!({"width": 2, "height": 2, "name": "second"}),
        )?;
        let second_id = app.session.active().ok_or("no document")?.doc.id.0;
        requests.complete(
            &mut app,
            request["id"].as_u64().ok_or("no id")?,
            "success",
            "original.psd",
            "",
        )?;
        assert_eq!(
            app.session.active().ok_or("no document")?.doc.id.0,
            second_id
        );
        let saved = app
            .session
            .documents()
            .iter()
            .find(|st| st.doc.id.0 == saved_id)
            .ok_or("original document missing")?;
        assert_eq!(saved.saved_revision, encoded_revision);
        assert!(saved.is_dirty(), "edits after encoding must stay dirty");
        assert_eq!(saved.path.as_deref(), request["path"].as_str());
        assert!(Path::new(request["path"].as_str().ok_or("no path")?).exists());
        Ok(())
    }

    #[test]
    fn save_copy_and_export_keep_the_working_document_dirty() -> Result<(), String> {
        let root = TempRoot::new()?;
        let requests = FileRequests::new(&root.0.join("cache"))?;
        let mut app = app(&root.0, &requests)?;
        photocraft_ui_egui::menus::invoke(
            &mut app,
            &egui::Context::default(),
            "file.saveACopy",
            json!({}),
        )?;
        requests.finish_frame(&mut app);
        let copy: serde_json::Value =
            serde_json::from_str(&requests.take_json()).map_err(|e| e.to_string())?;
        requests.complete(
            &mut app,
            copy["id"].as_u64().ok_or("no id")?,
            "success",
            "copy.psd",
            "",
        )?;
        assert!(app.session.active().ok_or("no document")?.is_dirty());
        assert_eq!(app.session.active().ok_or("no document")?.path, None);
        assert!(!Path::new(copy["path"].as_str().ok_or("no path")?).exists());

        let fields =
            serde_json::from_value(json!({"format": "png", "scale": 100, "transparency": true}))
                .map_err(|e| e.to_string())?;
        photocraft_ui_egui::export_dialog::confirm(&mut app, &fields)?;
        requests.finish_frame(&mut app);
        let export: serde_json::Value =
            serde_json::from_str(&requests.take_json()).map_err(|e| e.to_string())?;
        requests.complete(
            &mut app,
            export["id"].as_u64().ok_or("no id")?,
            "success",
            "original.png",
            "",
        )?;
        assert!(app.session.active().ok_or("no document")?.is_dirty());
        assert_eq!(app.session.active().ok_or("no document")?.path, None);
        Ok(())
    }

    #[test]
    fn repeated_save_uses_a_fresh_stage_and_the_previous_destination() -> Result<(), String> {
        let root = TempRoot::new()?;
        let requests = FileRequests::new(&root.0.join("cache"))?;
        let mut app = app(&root.0, &requests)?;
        let first = queued_save(&mut app, &requests)?;
        requests.complete(
            &mut app,
            first["id"].as_u64().ok_or("no id")?,
            "success",
            "original.psd",
            "",
        )?;
        let previous = app
            .session
            .active()
            .ok_or("no document")?
            .path
            .clone()
            .ok_or("no saved path")?;
        let previous_bytes = std::fs::read(&previous).map_err(|e| e.to_string())?;
        app.run("layer.new.layer", json!({}))?;
        photocraft_ui_egui::menus::invoke(
            &mut app,
            &egui::Context::default(),
            "file.save",
            json!({}),
        )?;
        requests.finish_frame(&mut app);
        let second: serde_json::Value =
            serde_json::from_str(&requests.take_json()).map_err(|e| e.to_string())?;
        assert_eq!(second["chooseDestination"], false);
        assert_eq!(second["previousPath"], previous);
        assert_ne!(second["path"], previous);
        assert!(app.session.active().ok_or("no document")?.is_dirty());
        requests.complete(
            &mut app,
            second["id"].as_u64().ok_or("no id")?,
            "cancel",
            "",
            "",
        )?;
        assert_eq!(
            app.session.active().ok_or("no document")?.path.as_deref(),
            Some(previous.as_str())
        );
        assert!(Path::new(&previous).exists());
        assert_eq!(
            std::fs::read(&previous).map_err(|e| e.to_string())?,
            previous_bytes
        );
        Ok(())
    }

    #[test]
    fn open_and_open_as_share_one_async_request_and_preserve_import_paths() -> Result<(), String> {
        let root = TempRoot::new()?;
        let requests = FileRequests::new(&root.0.join("cache"))?;
        let mut app = app(&root.0, &requests)?;
        let ctx = egui::Context::default();
        photocraft_ui_egui::menus::invoke(&mut app, &ctx, "file.open", json!({}))?;
        let first: serde_json::Value =
            serde_json::from_str(&requests.take_json()).map_err(|e| e.to_string())?;
        assert_eq!(first["kind"], "open");
        assert_eq!(first["path"], "");
        assert_eq!(app.session.documents().len(), 1);
        photocraft_ui_egui::menus::invoke(&mut app, &ctx, "file.openAs", json!({}))?;
        assert!(
            requests.take_json().is_empty(),
            "a second picker must not replace the first"
        );
        let id = first["id"].as_u64().ok_or("no id")?;
        assert!(
            requests
                .complete(&mut app, id + 1, "success", "", "")
                .is_err()
        );
        requests.complete(&mut app, id, "cancel", "", "")?;
        assert!(app.session.active().ok_or("no document")?.is_dirty());

        photocraft_ui_egui::menus::invoke(&mut app, &ctx, "file.openAs", json!({}))?;
        let second: serde_json::Value =
            serde_json::from_str(&requests.take_json()).map_err(|e| e.to_string())?;
        let png = app.services.encode_png.as_ref().ok_or("no encoder")?(1, 1, &[255, 0, 0, 255])?;
        let path = root.0.join("cache/import.png");
        photocraft_format::atomic_write(&path, &png).map_err(|e| e.to_string())?;
        // The shell stages the authorized URI and completes the matching intent.
        requests.complete_with_path(
            &mut app,
            &ctx,
            second["id"].as_u64().ok_or("no id")?,
            "success",
            path.to_str().ok_or("non-UTF8 path")?,
            "import.png",
            "",
        )?;
        assert_eq!(app.session.documents().len(), 2);
        assert_eq!(
            app.session.active().ok_or("no document")?.path.as_deref(),
            path.to_str()
        );
        assert!(
            app.ui
                .recent_files
                .iter()
                .any(|recent| recent == &path.to_string_lossy())
        );
        Ok(())
    }

    #[test]
    fn quick_export_direct_engine_write_is_published_without_clearing_dirty() -> Result<(), String>
    {
        let root = TempRoot::new()?;
        let requests = FileRequests::new(&root.0.join("cache"))?;
        let mut app = app(&root.0, &requests)?;
        app.run(
            "prefs.set",
            json!({"values": {"export": {"quickExportLocation": "sameFolder"}}}),
        )?;
        photocraft_ui_egui::export_dialog::quick_export_png(&mut app)?;
        requests.finish_frame(&mut app);
        let request: serde_json::Value =
            serde_json::from_str(&requests.take_json()).map_err(|e| e.to_string())?;
        assert_eq!(request["kind"], "save");
        assert_eq!(request["chooseDestination"], true);
        let path = request["path"].as_str().ok_or("no path")?;
        assert!(std::fs::metadata(path).is_ok_and(|metadata| metadata.len() > 0));
        requests.complete(
            &mut app,
            request["id"].as_u64().ok_or("no id")?,
            "success",
            "original.png",
            "",
        )?;
        assert!(app.session.active().ok_or("no document")?.is_dirty());
        assert_eq!(app.session.active().ok_or("no document")?.path, None);
        assert!(!Path::new(path).exists());
        Ok(())
    }

    #[test]
    fn save_before_close_waits_for_publication_and_retries_after_cancel_or_error()
    -> Result<(), String> {
        let root = TempRoot::new()?;
        let requests = FileRequests::new(&root.0)?;
        let mut app = app(&root.0, &requests)?;
        let ctx = egui::Context::default();
        photocraft_ui_egui::menus::invoke(&mut app, &ctx, "file.close", json!({}))?;
        for result in ["cancel", "error", "success"] {
            app.save_unsaved_changes(&ctx);
            assert_eq!(
                app.session.documents().len(),
                1,
                "encoding must not close the document"
            );
            assert!(app.session.active().ok_or("missing document")?.is_dirty());
            requests.finish_frame(&mut app);
            let request: serde_json::Value =
                serde_json::from_str(&requests.take_json()).map_err(|e| e.to_string())?;
            assert!(
                request["path"]
                    .as_str()
                    .ok_or("missing stage")?
                    .contains("PhotoCraft/Documents/staging")
            );
            requests.complete_with_path(
                &mut app,
                &ctx,
                request["id"].as_u64().ok_or("missing id")?,
                result,
                "",
                "original.psd",
                "publication failed",
            )?;
            if result == "success" {
                assert!(
                    app.session.documents().is_empty(),
                    "successful publication resumes Close"
                );
                assert!(Path::new(request["path"].as_str().ok_or("missing stage")?).exists());
            } else {
                assert_eq!(app.session.documents().len(), 1);
                assert!(app.session.active().ok_or("missing document")?.is_dirty());
                assert_eq!(app.session.active().ok_or("missing document")?.path, None);
            }
        }
        Ok(())
    }

    #[test]
    fn close_all_saves_each_document_before_running_the_parked_action() -> Result<(), String> {
        let root = TempRoot::new()?;
        let requests = FileRequests::new(&root.0)?;
        let mut app = app(&root.0, &requests)?;
        app.run(
            "file.new",
            json!({"width": 2, "height": 2, "name": "second"}),
        )?;
        app.run("layer.new.layer", json!({}))?;
        let ctx = egui::Context::default();
        photocraft_ui_egui::menus::invoke(&mut app, &ctx, "file.closeAll", json!({}))?;
        for remaining in [2, 0] {
            app.save_unsaved_changes(&ctx);
            requests.finish_frame(&mut app);
            let request: serde_json::Value =
                serde_json::from_str(&requests.take_json()).map_err(|e| e.to_string())?;
            requests.complete_with_path(
                &mut app,
                &ctx,
                request["id"].as_u64().ok_or("missing id")?,
                "success",
                "",
                "document.psd",
                "",
            )?;
            assert_eq!(app.session.documents().len(), remaining);
        }
        Ok(())
    }

    #[test]
    fn edits_while_a_close_save_is_publishing_keep_the_prompt_and_document() -> Result<(), String> {
        let root = TempRoot::new()?;
        let requests = FileRequests::new(&root.0)?;
        let mut app = app(&root.0, &requests)?;
        let ctx = egui::Context::default();
        photocraft_ui_egui::menus::invoke(&mut app, &ctx, "file.close", json!({}))?;
        app.save_unsaved_changes(&ctx);
        requests.finish_frame(&mut app);
        let request: serde_json::Value =
            serde_json::from_str(&requests.take_json()).map_err(|e| e.to_string())?;
        app.run("layer.new.layer", json!({}))?;
        requests.complete_with_path(
            &mut app,
            &ctx,
            request["id"].as_u64().ok_or("missing id")?,
            "success",
            "",
            "original.psd",
            "",
        )?;
        assert_eq!(app.session.documents().len(), 1);
        assert!(app.session.active().ok_or("missing document")?.is_dirty());
        app.save_unsaved_changes(&ctx);
        requests.finish_frame(&mut app);
        let retry: serde_json::Value =
            serde_json::from_str(&requests.take_json()).map_err(|e| e.to_string())?;
        assert_eq!(retry["chooseDestination"], false);
        requests.complete_with_path(
            &mut app,
            &ctx,
            retry["id"].as_u64().ok_or("missing id")?,
            "success",
            "",
            "original.psd",
            "",
        )?;
        assert!(app.session.documents().is_empty());
        Ok(())
    }

    #[test]
    fn place_completion_keeps_its_original_target_after_switching_documents() -> Result<(), String>
    {
        for intent in ["file.placeEmbedded", "file.placeLinked"] {
            let root = TempRoot::new()?;
            let requests = FileRequests::new(&root.0)?;
            let mut app = app(&root.0, &requests)?;
            let original_id = app.session.active().ok_or("missing document")?.doc.id;
            let original_layers = app
                .session
                .active()
                .ok_or("missing document")?
                .doc
                .layers
                .len();
            let ctx = egui::Context::default();
            photocraft_ui_egui::menus::invoke(&mut app, &ctx, intent, json!({}))?;
            let request: serde_json::Value =
                serde_json::from_str(&requests.take_json()).map_err(|e| e.to_string())?;
            assert_eq!(request["intent"], intent);
            app.run(
                "file.new",
                json!({"width": 2, "height": 2, "name": "second"}),
            )?;
            let second_id = app
                .session
                .active()
                .ok_or("missing second document")?
                .doc
                .id;
            let second_layers = app
                .session
                .active()
                .ok_or("missing second document")?
                .doc
                .layers
                .len();
            let png = app.services.encode_png.as_ref().ok_or("missing encoder")?(
                1,
                1,
                &[255, 0, 0, 255],
            )?;
            let path = root.0.join("place.png");
            photocraft_format::atomic_write(&path, &png).map_err(|e| e.to_string())?;
            requests.complete_with_path(
                &mut app,
                &ctx,
                request["id"].as_u64().ok_or("missing id")?,
                "success",
                path.to_str().ok_or("non-UTF8 path")?,
                "place.png",
                "",
            )?;
            assert_eq!(
                app.session
                    .active()
                    .ok_or("missing active document")?
                    .doc
                    .id,
                second_id
            );
            assert_eq!(
                app.session
                    .active()
                    .ok_or("missing active document")?
                    .doc
                    .layers
                    .len(),
                second_layers
            );
            assert_eq!(
                app.session
                    .documents()
                    .iter()
                    .find(|st| st.doc.id == original_id)
                    .ok_or("missing original document")?
                    .doc
                    .layers
                    .len(),
                original_layers + 1
            );
            assert!(
                Path::new(&path).exists(),
                "linked input must remain available"
            );
        }
        Ok(())
    }

    #[test]
    fn failed_open_is_visible_and_does_not_create_a_document() -> Result<(), String> {
        let root = TempRoot::new()?;
        let requests = FileRequests::new(&root.0)?;
        let mut app = app(&root.0, &requests)?;
        let ctx = egui::Context::default();
        app.open_dialog_file();
        let request: serde_json::Value =
            serde_json::from_str(&requests.take_json()).map_err(|e| e.to_string())?;
        assert!(
            requests
                .complete_with_path(
                    &mut app,
                    &ctx,
                    request["id"].as_u64().ok_or("missing id")?,
                    "success",
                    "",
                    "missing.png",
                    ""
                )
                .is_err()
        );
        assert!(app.ui.status_error);
        assert_eq!(app.session.documents().len(), 1);
        assert!(requests.take_json().is_empty());
        Ok(())
    }

    #[test]
    fn save_as_selects_a_new_destination_and_cancel_preserves_the_previous_file()
    -> Result<(), String> {
        let root = TempRoot::new()?;
        let requests = FileRequests::new(&root.0)?;
        let mut app = app(&root.0, &requests)?;
        let first = queued_save(&mut app, &requests)?;
        requests.complete(
            &mut app,
            first["id"].as_u64().ok_or("missing id")?,
            "success",
            "original.psd",
            "",
        )?;
        let previous = app
            .session
            .active()
            .ok_or("missing document")?
            .path
            .clone()
            .ok_or("missing saved path")?;
        let previous_bytes = std::fs::read(&previous).map_err(|e| e.to_string())?;
        app.run("layer.new.layer", json!({}))?;
        let ctx = egui::Context::default();
        for result in ["cancel", "error", "success"] {
            photocraft_ui_egui::menus::invoke(&mut app, &ctx, "file.saveAs", json!({}))?;
            requests.finish_frame(&mut app);
            let request: serde_json::Value =
                serde_json::from_str(&requests.take_json()).map_err(|e| e.to_string())?;
            assert_eq!(request["chooseDestination"], true);
            assert_eq!(request["previousPath"], "");
            assert_eq!(
                app.session
                    .active()
                    .ok_or("missing document")?
                    .path
                    .as_deref(),
                Some(previous.as_str())
            );
            requests.complete_with_path(
                &mut app,
                &ctx,
                request["id"].as_u64().ok_or("missing id")?,
                result,
                "",
                "new.psd",
                "publication failed",
            )?;
            assert_eq!(
                std::fs::read(&previous).map_err(|e| e.to_string())?,
                previous_bytes
            );
            if result == "success" {
                assert_eq!(
                    app.session
                        .active()
                        .ok_or("missing document")?
                        .path
                        .as_deref(),
                    request["path"].as_str()
                );
                assert!(!app.session.active().ok_or("missing document")?.is_dirty());
            } else {
                assert_eq!(
                    app.session
                        .active()
                        .ok_or("missing document")?
                        .path
                        .as_deref(),
                    Some(previous.as_str())
                );
                assert!(app.session.active().ok_or("missing document")?.is_dirty());
            }
        }
        Ok(())
    }

    #[test]
    fn preset_picker_preserves_the_selected_import_kinds() -> Result<(), String> {
        let root = TempRoot::new()?;
        let requests = FileRequests::new(&root.0)?;
        let mut app = app(&root.0, &requests)?;
        let mut source = Session::new();
        source
            .execute(
                "brush.presets.save",
                json!({"name": "Async imported brush"}),
            )
            .map_err(|e| e.to_string())?;
        let out = source
            .execute(
                "edit.presets.exportImportPresets",
                json!({"action": "export"}),
            )
            .map_err(|e| e.to_string())?;
        let path = root.0.join("presets.pcpresets");
        photocraft_format::atomic_write(&path, out["data"].to_string().as_bytes())
            .map_err(|e| e.to_string())?;
        let ctx = egui::Context::default();
        for brushes in [false, true] {
            let fields = serde_json::from_value(json!({"__prefsui": "presetsIO", "action": "import", "brushes": brushes, "customShapes": !brushes})).map_err(|e| e.to_string())?;
            photocraft_ui_egui::prefs_ui::confirm(&mut app, &fields)?;
            let request: serde_json::Value =
                serde_json::from_str(&requests.take_json()).map_err(|e| e.to_string())?;
            requests.complete_with_path(
                &mut app,
                &ctx,
                request["id"].as_u64().ok_or("missing id")?,
                "success",
                path.to_str().ok_or("non-UTF8 path")?,
                "presets.pcpresets",
                "",
            )?;
            assert_eq!(
                app.session
                    .tools
                    .presets
                    .iter()
                    .any(|brush| brush.name == "Async imported brush"),
                brushes
            );
            assert_eq!(app.session.documents().len(), 1);
            assert!(app.session.active().ok_or("missing document")?.is_dirty());
        }
        Ok(())
    }

    #[test]
    fn script_picker_can_create_the_first_document() -> Result<(), String> {
        let root = TempRoot::new()?;
        let requests = FileRequests::new(&root.0)?;
        let mut app = PhotocraftApp::new(
            Session::new(),
            crate::services::services_with_files(&root.0, &requests),
        );
        let ctx = egui::Context::default();
        photocraft_ui_egui::menus::invoke(&mut app, &ctx, "file.scripts.browse", json!({}))?;
        let request: serde_json::Value =
            serde_json::from_str(&requests.take_json()).map_err(|e| e.to_string())?;
        let path = root.0.join("new-document.txt");
        photocraft_format::atomic_write(
            &path,
            b"file.new {\"width\":2,\"height\":2,\"name\":\"Script document\"}\n",
        )
        .map_err(|e| e.to_string())?;
        requests.complete_with_path(
            &mut app,
            &ctx,
            request["id"].as_u64().ok_or("missing id")?,
            "success",
            path.to_str().ok_or("non-UTF8 path")?,
            "new-document.txt",
            "",
        )?;
        assert_eq!(
            app.session.active().ok_or("missing document")?.doc.name,
            "Script document"
        );
        Ok(())
    }

    #[test]
    fn png_save_as_psd_or_pcraft_keeps_layers_and_the_captured_revision() -> Result<(), String> {
        for extension in ["psd", "pcraft"] {
            let root = TempRoot::new()?;
            let requests = FileRequests::new(&root.0)?;
            let mut app = PhotocraftApp::new(
                Session::new(),
                crate::services::services_with_files(&root.0, &requests),
            );
            let png = app.services.encode_png.as_ref().ok_or("missing encoder")?(
                2,
                2,
                &[255, 0, 0, 255].repeat(4),
            )?;
            let original_path = root.0.join("original.png");
            photocraft_format::atomic_write(&original_path, &png).map_err(|e| e.to_string())?;
            app.open_path(original_path.to_str().ok_or("non-UTF8 path")?)?;
            app.run("layer.new.layer", json!({"name": "Preserved layer"}))?;
            let document = app.session.active().ok_or("missing document")?;
            let (document_id, revision, layers) = (
                document.doc.id.0,
                document.revision,
                document.doc.layers.len(),
            );
            let request = queued_save(&mut app, &requests)?;
            assert_eq!(request["allowFormatChoice"], true);
            assert!(
                request["allowedExtensions"]
                    .as_array()
                    .ok_or("missing formats")?
                    .contains(&json!(extension))
            );
            app.run("layer.new.layer", json!({"name": "Later edit"}))?;
            app.run(
                "file.new",
                json!({"width": 2, "height": 2, "name": "Different tab"}),
            )?;
            let active_id = app
                .session
                .active()
                .ok_or("missing active document")?
                .doc
                .id
                .0;
            let path = requests.prepare_save(
                &mut app,
                request["id"].as_u64().ok_or("missing id")?,
                &format!("chosen.{extension}"),
            )?;
            let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
            let imported = photocraft_io::import(&path, &bytes)
                .map_err(|e| e.to_string())?
                .document;
            assert_eq!(
                imported.layers.len(),
                layers,
                "format preparation must retain the original layered snapshot"
            );
            assert!(
                imported
                    .layers
                    .iter()
                    .any(|layer| layer.name == "Preserved layer")
            );
            assert!(
                !imported
                    .layers
                    .iter()
                    .any(|layer| layer.name == "Later edit")
            );
            requests.complete(
                &mut app,
                request["id"].as_u64().ok_or("missing id")?,
                "success",
                &format!("chosen.{extension}"),
                "",
            )?;
            assert_eq!(
                app.session
                    .active()
                    .ok_or("missing active document")?
                    .doc
                    .id
                    .0,
                active_id
            );
            let saved = app
                .session
                .documents()
                .iter()
                .find(|st| st.doc.id.0 == document_id)
                .ok_or("missing original document")?;
            assert_eq!(saved.saved_revision, revision);
            assert!(saved.is_dirty());
            assert_eq!(saved.path.as_deref(), Some(path.as_str()));
            assert_eq!(
                std::fs::read(&original_path).map_err(|e| e.to_string())?,
                png
            );
        }
        Ok(())
    }

    #[test]
    fn changed_format_cancel_or_encoding_failure_preserves_saved_file_and_dirty_state()
    -> Result<(), String> {
        for failure in ["cancel", "error", "encoding"] {
            let root = TempRoot::new()?;
            let requests = FileRequests::new(&root.0)?;
            let mut app = app(&root.0, &requests)?;
            let saved = queued_save(&mut app, &requests)?;
            requests.complete(
                &mut app,
                saved["id"].as_u64().ok_or("missing id")?,
                "success",
                "original.psd",
                "",
            )?;
            let previous = app
                .session
                .active()
                .ok_or("missing document")?
                .path
                .clone()
                .ok_or("missing saved path")?;
            let previous_bytes = std::fs::read(&previous).map_err(|e| e.to_string())?;
            app.run("layer.new.layer", json!({}))?;
            let request = queued_save(&mut app, &requests)?;
            let id = request["id"].as_u64().ok_or("missing id")?;
            let path = if failure == "encoding" {
                app.services.export = Some(Box::new(|_, _, _| Err("codec failed".into())));
                assert!(
                    requests
                        .prepare_save(&mut app, id, "chosen.pcraft")
                        .is_err()
                );
                request["path"].as_str().ok_or("missing stage")?.to_string()
            } else {
                requests.prepare_save(&mut app, id, "chosen.pcraft")?
            };
            requests.complete(
                &mut app,
                id,
                if failure == "cancel" {
                    "cancel"
                } else {
                    "error"
                },
                "",
                "publication failed",
            )?;
            let document = app.session.active().ok_or("missing document")?;
            assert!(document.is_dirty());
            assert_eq!(document.path.as_deref(), Some(previous.as_str()));
            assert_eq!(
                std::fs::read(&previous).map_err(|e| e.to_string())?,
                previous_bytes
            );
            assert!(!Path::new(&path).exists());
        }
        Ok(())
    }

    #[test]
    fn save_copy_can_change_format_but_export_and_in_place_save_cannot() -> Result<(), String> {
        let root = TempRoot::new()?;
        let requests = FileRequests::new(&root.0)?;
        let mut app = app(&root.0, &requests)?;
        let ctx = egui::Context::default();
        photocraft_ui_egui::menus::invoke(&mut app, &ctx, "file.saveACopy", json!({}))?;
        requests.finish_frame(&mut app);
        let request: Value =
            serde_json::from_str(&requests.take_json()).map_err(|e| e.to_string())?;
        assert_eq!(request["allowFormatChoice"], true);
        let path = requests.prepare_save(
            &mut app,
            request["id"].as_u64().ok_or("missing id")?,
            "copy.pcraft",
        )?;
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        assert_eq!(
            photocraft_io::import(&path, &bytes)
                .map_err(|e| e.to_string())?
                .document
                .layers
                .len(),
            app.session
                .active()
                .ok_or("missing document")?
                .doc
                .layers
                .len()
        );
        requests.complete(
            &mut app,
            request["id"].as_u64().ok_or("missing id")?,
            "success",
            "copy.pcraft",
            "",
        )?;
        assert!(app.session.active().ok_or("missing document")?.is_dirty());
        assert_eq!(app.session.active().ok_or("missing document")?.path, None);
        assert!(!Path::new(&path).exists());
        photocraft_ui_egui::export_dialog::quick_export_png(&mut app)?;
        requests.finish_frame(&mut app);
        let export: Value =
            serde_json::from_str(&requests.take_json()).map_err(|e| e.to_string())?;
        assert_eq!(export["allowFormatChoice"], false);
        assert!(
            requests
                .prepare_save(
                    &mut app,
                    export["id"].as_u64().ok_or("missing id")?,
                    "export.psd"
                )
                .is_err()
        );
        requests.complete(
            &mut app,
            export["id"].as_u64().ok_or("missing id")?,
            "cancel",
            "",
            "",
        )?;
        let saved = queued_save(&mut app, &requests)?;
        requests.complete(
            &mut app,
            saved["id"].as_u64().ok_or("missing id")?,
            "success",
            "original.psd",
            "",
        )?;
        app.run("layer.new.layer", json!({}))?;
        photocraft_ui_egui::menus::invoke(&mut app, &ctx, "file.save", json!({}))?;
        requests.finish_frame(&mut app);
        let in_place: Value =
            serde_json::from_str(&requests.take_json()).map_err(|e| e.to_string())?;
        assert_eq!(in_place["allowFormatChoice"], false);
        assert!(
            requests
                .prepare_save(
                    &mut app,
                    in_place["id"].as_u64().ok_or("missing id")?,
                    "replacement.pcraft"
                )
                .is_err()
        );
        requests.complete(
            &mut app,
            in_place["id"].as_u64().ok_or("missing id")?,
            "cancel",
            "",
            "",
        )?;
        Ok(())
    }
    #[test]
    fn published_png_reuses_destination_but_imported_and_failed_flat_files_do_not()
    -> Result<(), String> {
        let root = TempRoot::new()?;
        let requests = FileRequests::new(&root.0)?;
        let mut app = PhotocraftApp::new(
            Session::new(),
            crate::services::services_with_files(&root.0, &requests),
        );
        let bytes = app
            .services
            .encode_png
            .as_ref()
            .ok_or("missing PNG encoder")?(1, 1, &[1, 2, 3, 255])?;
        let imported = root.0.join("input.png");
        photocraft_format::atomic_write(&imported, &bytes).map_err(|error| error.to_string())?;
        app.open_path(imported.to_str().ok_or("non UTF8 path")?)?;
        app.session
            .execute("layer.new.layer", json!({"name":"edit"}))
            .map_err(|error| error.to_string())?;
        let ctx = egui::Context::default();
        photocraft_ui_egui::menus::invoke(&mut app, &ctx, "file.save", json!({}))?;
        requests.finish_frame(&mut app);
        let first: Value =
            serde_json::from_str(&requests.take_json()).map_err(|error| error.to_string())?;
        assert_eq!(first["chooseDestination"], true);
        requests.complete(
            &mut app,
            first["id"].as_u64().ok_or("missing id")?,
            "cancel",
            "",
            "",
        )?;
        assert!(!requests.is_published_save(first["path"].as_str().ok_or("missing path")?));
        let saved = queued_save(&mut app, &requests)?;
        let mut previous = saved["path"].as_str().ok_or("missing path")?.to_string();
        requests.complete(
            &mut app,
            saved["id"].as_u64().ok_or("missing id")?,
            "success",
            "saved.png",
            "",
        )?;
        for name in ["first later edit", "second later edit"] {
            app.session
                .execute("layer.new.layer", json!({"name":name}))
                .map_err(|error| error.to_string())?;
            photocraft_ui_egui::menus::invoke(&mut app, &ctx, "file.save", json!({}))?;
            requests.finish_frame(&mut app);
            let request: Value =
                serde_json::from_str(&requests.take_json()).map_err(|error| error.to_string())?;
            assert_eq!(request["chooseDestination"], false);
            assert_eq!(request["previousPath"], previous);
            assert_eq!(request["allowFormatChoice"], false);
            requests.complete(
                &mut app,
                request["id"].as_u64().ok_or("missing id")?,
                "success",
                "saved.png",
                "",
            )?;
            previous = request["path"].as_str().ok_or("missing path")?.into();
            assert!(!app.session.active().ok_or("missing doc")?.is_dirty());
        }
        assert_eq!(
            std::fs::read(imported).map_err(|error| error.to_string())?,
            bytes
        );
        Ok(())
    }

    #[test]
    fn same_format_destination_renames_the_stage_and_recent_without_reencoding()
    -> Result<(), String> {
        let root = TempRoot::new()?;
        let requests = FileRequests::new(&root.0)?;
        let mut document = app(&root.0, &requests)?;
        let request = queued_save(&mut document, &requests)?;
        let id = request["id"].as_u64().ok_or("missing save id")?;
        let old = request["path"].as_str().ok_or("missing stage")?;
        let original = std::fs::read(old).map_err(|e| e.to_string())?;
        let actual =
            requests.prepare_save(&mut document, id, "PhotoCraft-NewDocument-20261008.psd")?;
        assert!(!Path::new(old).exists());
        assert_eq!(std::fs::read(&actual).map_err(|e| e.to_string())?, original);
        assert_eq!(
            Path::new(&actual)
                .file_name()
                .and_then(|name| name.to_str()),
            Some("PhotoCraft-NewDocument-20261008.psd")
        );
        requests.complete(
            &mut document,
            id,
            "success",
            "PhotoCraft-NewDocument-20261008.psd",
            "",
        )?;
        assert_eq!(
            document
                .session
                .active()
                .ok_or("missing document")?
                .doc
                .name,
            "PhotoCraft-NewDocument-20261008.psd"
        );
        assert_eq!(
            document
                .session
                .active()
                .ok_or("missing document")?
                .path
                .as_deref(),
            Some(actual.as_str())
        );
        assert_eq!(
            document.ui.recent_files.first().map(String::as_str),
            Some(actual.as_str())
        );
        let mut reopened = PhotocraftApp::new(
            Session::new(),
            crate::services::services_with_files(&root.0, &requests),
        );
        reopened.open_path(&actual)?;
        assert_eq!(
            reopened
                .session
                .active()
                .ok_or("missing reopened document")?
                .doc
                .name,
            "PhotoCraft-NewDocument-20261008.psd"
        );
        assert_eq!(
            reopened
                .session
                .active()
                .ok_or("missing reopened document")?
                .doc
                .layers
                .len(),
            document
                .session
                .active()
                .ok_or("missing original document")?
                .doc
                .layers
                .len()
        );
        let path = document
            .session
            .active()
            .ok_or("missing document")?
            .path
            .clone();
        let revision = document
            .session
            .active()
            .ok_or("missing document")?
            .saved_revision;
        let name = document
            .session
            .active()
            .ok_or("missing document")?
            .doc
            .name
            .clone();
        document.run("layer.new.layer", json!({}))?;
        let ctx = egui::Context::default();
        photocraft_ui_egui::menus::invoke(&mut document, &ctx, "file.saveACopy", json!({}))?;
        requests.finish_frame(&mut document);
        let copy: Value = serde_json::from_str(&requests.take_json()).map_err(|e| e.to_string())?;
        let copy_id = copy["id"].as_u64().ok_or("missing copy id")?;
        let copy_path = requests.prepare_save(&mut document, copy_id, "actual-copy.psd")?;
        requests.complete(&mut document, copy_id, "success", "actual-copy.psd", "")?;
        assert!(!Path::new(&copy_path).exists());
        assert_eq!(
            document
                .session
                .active()
                .ok_or("missing document")?
                .doc
                .name,
            name
        );
        assert_eq!(
            document.session.active().ok_or("missing document")?.path,
            path
        );
        assert_eq!(
            document
                .session
                .active()
                .ok_or("missing document")?
                .saved_revision,
            revision
        );
        assert!(
            document
                .session
                .active()
                .ok_or("missing document")?
                .is_dirty()
        );
        photocraft_ui_egui::export_dialog::quick_export_png(&mut document)?;
        requests.finish_frame(&mut document);
        let export: Value =
            serde_json::from_str(&requests.take_json()).map_err(|e| e.to_string())?;
        let export_id = export["id"].as_u64().ok_or("missing export id")?;
        let export_path = requests.prepare_save(&mut document, export_id, "actual-export.png")?;
        requests.complete(&mut document, export_id, "success", "actual-export.png", "")?;
        assert!(!Path::new(&export_path).exists());
        assert_eq!(
            document
                .session
                .active()
                .ok_or("missing document")?
                .doc
                .name,
            name
        );
        assert_eq!(
            document.session.active().ok_or("missing document")?.path,
            path
        );
        assert_eq!(
            document
                .session
                .active()
                .ok_or("missing document")?
                .saved_revision,
            revision
        );
        assert_eq!(
            document.ui.recent_files.first().map(String::as_str),
            Some(actual.as_str())
        );
        Ok(())
    }

    #[test]
    fn successfully_published_png_reopens_with_its_destination_after_worker_restart()
    -> Result<(), String> {
        let root = TempRoot::new()?;
        let requests = FileRequests::new(&root.0)?;
        let mut document = app(&root.0, &requests)?;
        let request = queued_save(&mut document, &requests)?;
        let id = request["id"].as_u64().ok_or("missing save id")?;
        let saved = requests.prepare_save(&mut document, id, "saved.png")?;
        requests.complete(&mut document, id, "success", "saved.png", "")?;
        assert!(requests.is_published_save(&saved));
        drop(document);
        drop(requests);
        let restored = FileRequests::new(&root.0)?;
        let mut reopened = PhotocraftApp::new(
            Session::new(),
            crate::services::services_with_files(&root.0, &restored),
        );
        reopened.open_path(&saved)?;
        reopened.run("layer.new.layer", json!({"name":"after restart"}))?;
        let ctx = egui::Context::default();
        photocraft_ui_egui::menus::invoke(&mut reopened, &ctx, "file.save", json!({}))?;
        restored.finish_frame(&mut reopened);
        let next: Value = serde_json::from_str(&restored.take_json()).map_err(|e| e.to_string())?;
        assert_eq!(next["chooseDestination"], false);
        assert_eq!(next["previousPath"], saved);
        assert_eq!(next["allowFormatChoice"], false);
        assert!(
            reopened
                .session
                .active()
                .ok_or("missing reopened document")?
                .is_dirty()
        );
        restored.complete(
            &mut reopened,
            next["id"].as_u64().ok_or("missing next id")?,
            "cancel",
            "",
            "",
        )?;
        Ok(())
    }

    #[test]
    fn only_successful_document_saves_create_persistent_in_place_authority() -> Result<(), String> {
        let root = TempRoot::new()?;
        let requests = FileRequests::new(&root.0)?;
        let mut document = app(&root.0, &requests)?;
        for result in ["cancel", "error"] {
            let request = queued_save(&mut document, &requests)?;
            let id = request["id"].as_u64().ok_or("missing id")?;
            let path = request["path"].as_str().ok_or("missing path")?;
            requests.complete(&mut document, id, result, "", "failure")?;
            assert!(!FileRequests::new(&root.0)?.is_published_save(path));
        }
        let ctx = egui::Context::default();
        photocraft_ui_egui::menus::invoke(&mut document, &ctx, "file.saveACopy", json!({}))?;
        requests.finish_frame(&mut document);
        let copy: Value = serde_json::from_str(&requests.take_json()).map_err(|e| e.to_string())?;
        requests.complete(
            &mut document,
            copy["id"].as_u64().ok_or("missing copy id")?,
            "success",
            "copy.psd",
            "",
        )?;
        photocraft_ui_egui::export_dialog::quick_export_png(&mut document)?;
        requests.finish_frame(&mut document);
        let export: Value =
            serde_json::from_str(&requests.take_json()).map_err(|e| e.to_string())?;
        requests.complete(
            &mut document,
            export["id"].as_u64().ok_or("missing export id")?,
            "success",
            "export.png",
            "",
        )?;
        assert!(!requests.published_file.exists());
        assert!(
            FileRequests::new(&root.0)?
                .state
                .borrow()
                .published
                .is_empty()
        );
        assert!(
            document
                .session
                .active()
                .ok_or("missing document")?
                .is_dirty()
        );
        Ok(())
    }

    #[test]
    fn corrupted_oversized_and_forged_destination_history_never_grants_import_authority()
    -> Result<(), String> {
        let root = TempRoot::new()?;
        let requests = FileRequests::new(&root.0)?;
        let imported = root.0.join("PhotoCraft/Documents/imports/input.png");
        std::fs::create_dir_all(imported.parent().ok_or("missing import parent")?)
            .map_err(|e| e.to_string())?;
        std::fs::write(&imported, b"original import").map_err(|e| e.to_string())?;
        let journal = requests.published_file.clone();
        let input_path = imported.to_str().ok_or("non UTF8 import")?;
        for malformed in [
            "{broken".to_string(),
            json!({"version":1,"saves":[{"path":input_path,"size":15}]}).to_string(),
            json!({"version":1,"saves":[{"path":"../imports/input.png","size":15}]}).to_string(),
            json!({"version":1,"saves":[{"path":"123/input.png","size":0}]}).to_string(),
            json!({"version":2,"saves":[]}).to_string(),
            " ".repeat(MAX_PUBLISHED_JSON + 1),
        ] {
            std::fs::write(&journal, malformed).map_err(|e| e.to_string())?;
            let restored = FileRequests::new(&root.0)?;
            assert!(!restored.is_published_save(input_path));
            assert!(restored.state.borrow().published.is_empty());
            let mut document = app(&root.0, &restored)?;
            restored.finish_frame(&mut document);
            assert!(document.ui.status_error);
        }
        let file = requests.staging.join("123/input.png");
        std::fs::create_dir_all(file.parent().ok_or("missing stage parent")?)
            .map_err(|e| e.to_string())?;
        std::fs::write(&file, b"altered").map_err(|e| e.to_string())?;
        std::fs::write(
            &journal,
            json!({"version":1,"saves":[{"path":"123/input.png","size":3}]}).to_string(),
        )
        .map_err(|e| e.to_string())?;
        assert!(
            !FileRequests::new(&root.0)?.is_published_save(file.to_str().ok_or("non UTF8 stage")?)
        );
        #[cfg(unix)]
        {
            std::fs::remove_file(&file).map_err(|e| e.to_string())?;
            std::os::unix::fs::symlink(&imported, &file).map_err(|e| e.to_string())?;
            std::fs::write(
                &journal,
                json!({"version":1,"saves":[{"path":"123/input.png","size":15}]}).to_string(),
            )
            .map_err(|e| e.to_string())?;
            assert!(
                !FileRequests::new(&root.0)?
                    .is_published_save(file.to_str().ok_or("non UTF8 symlink")?)
            );
        }
        assert_eq!(
            std::fs::read(&imported).map_err(|e| e.to_string())?,
            b"original import"
        );
        Ok(())
    }

    #[test]
    fn destination_history_write_failure_is_visible_after_real_publication() -> Result<(), String> {
        let root = TempRoot::new()?;
        let requests = FileRequests::new(&root.0)?;
        std::fs::create_dir(&requests.published_file).map_err(|e| e.to_string())?;
        let mut document = app(&root.0, &requests)?;
        let request = queued_save(&mut document, &requests)?;
        let id = request["id"].as_u64().ok_or("missing id")?;
        let saved = requests.prepare_save(&mut document, id, "saved.png")?;
        requests.complete(&mut document, id, "success", "saved.png", "")?;
        assert!(
            !document
                .session
                .active()
                .ok_or("missing document")?
                .is_dirty()
        );
        assert!(document.ui.status_error);
        assert!(document.ui.status.contains("history could not be stored"));
        assert!(requests.is_published_save(&saved));
        assert!(!FileRequests::new(&root.0)?.is_published_save(&saved));
        Ok(())
    }
    #[test]
    fn rollback_failure_can_retain_only_its_actual_encoded_stage_and_never_marks_saved()
    -> Result<(), String> {
        for retained in [true, false] {
            let root = TempRoot::new()?;
            let requests = FileRequests::new(&root.0)?;
            let mut app = app(&root.0, &requests)?;
            let original = app.session.active().ok_or("doc missing")?.clone();
            let request = queued_save(&mut app, &requests)?;
            let path = request["path"].as_str().ok_or("path missing")?;
            let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
            let other = root.0.join("retain-spoof.psd");
            std::fs::write(&other, b"not the stage").map_err(|error| error.to_string())?;
            requests.complete_with_path(
                &mut app,
                &egui::Context::default(),
                request["id"].as_u64().ok_or("id missing")?,
                "error",
                if retained {
                    path
                } else {
                    other.to_str().ok_or("invalid path")?
                },
                "",
                "provider rollback failed",
            )?;
            let state = app.session.active().ok_or("doc missing")?;
            assert_eq!(state.path, original.path);
            assert_eq!(state.saved_revision, original.saved_revision);
            assert!(state.is_dirty());
            assert!(app.ui.status_error);
            assert_eq!(Path::new(path).exists(), retained);
            assert_eq!(
                std::fs::read(other).map_err(|error| error.to_string())?,
                b"not the stage"
            );
            if retained {
                assert_eq!(
                    std::fs::read(path).map_err(|error| error.to_string())?,
                    bytes
                );
            }
        }
        Ok(())
    }
}
