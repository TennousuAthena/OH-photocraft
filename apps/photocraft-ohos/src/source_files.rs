//! Fresh authorized file reads and Smart Object publication transactions.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use photocraft_doc::{LayerContent, LayerId, SmartSource};
use photocraft_engine::{
    Session,
    source_cmds::{SourceCommand, SourceResult},
};
use photocraft_ui_egui::{PhotocraftApp, i18n};
use serde_json::{Value, json};

const MAX_SOURCE_BYTES: u64 = 1_073_741_824;
const MAX_SOURCE_FILES: usize = 500;

#[derive(Clone, Default)]
pub(crate) struct OperationSlot(Arc<AtomicBool>);
pub(crate) struct Reservation(Arc<AtomicBool>);
impl OperationSlot {
    pub fn reserve(&self) -> Result<Reservation, String> {
        self.0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Reservation(self.0.clone()))
            .map_err(|_| "Finish the current file operation first".into())
    }
    pub fn is_pending(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[derive(Clone)]
pub(crate) struct SourceFiles {
    files: PathBuf,
    canonical_files: PathBuf,
    staging: PathBuf,
    slot: OperationSlot,
    state: Arc<Mutex<State>>,
}
#[derive(Default)]
struct State {
    pending: Option<Pending>,
    context: Option<egui::Context>,
}
struct Pending {
    request: SourceCommand,
    sources: Vec<ReadSource>,
    next: usize,
    refreshed: Vec<(LayerId, String)>,
    owned_reads: Vec<String>,
    failed: Vec<LayerId>,
    failures: Vec<String>,
    bytes_read: u64,
    phase: Phase,
    _reservation: Reservation,
}
struct ReadSource {
    path: String,
    layers: Vec<LayerId>,
}
enum Phase {
    Read(Wire),
    Save(Wire),
    Local(SourceResult),
}
struct Wire {
    id: u64,
    path: String,
    name: String,
    previous: String,
    choose: bool,
    delivered: bool,
    retain_source: bool,
}

impl SourceFiles {
    pub fn new(files: &Path, staging: &Path, slot: OperationSlot) -> Result<Self, String> {
        let canonical_files = files.canonicalize().map_err(|e| e.to_string())?;
        no_links(&canonical_files, staging)?;
        Ok(Self {
            files: files.to_path_buf(),
            canonical_files,
            staging: staging.into(),
            slot,
            state: Arc::default(),
        })
    }

    pub fn attach(&self, session: &mut Session, ctx: &egui::Context) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).context = Some(ctx.clone());
        let service = self.clone();
        session.source_file = Some(Arc::new(move |request| {
            let reservation = service
                .slot
                .reserve()
                .map_err(photocraft_engine::EngineError::Other)?;
            let pending = service
                .begin(request, reservation)
                .map_err(photocraft_engine::EngineError::Other)?;
            let mut state = service.state.lock().unwrap_or_else(|e| e.into_inner());
            state.pending = Some(pending);
            if let Some(ctx) = &state.context {
                ctx.request_repaint();
            }
            Ok(json!({"pending":true}))
        }));
    }

    fn begin(&self, request: SourceCommand, reservation: Reservation) -> Result<Pending, String> {
        let mut pending = Pending {
            request,
            sources: Vec::new(),
            next: 0,
            refreshed: Vec::new(),
            owned_reads: Vec::new(),
            failed: Vec::new(),
            failures: Vec::new(),
            bytes_read: 0,
            phase: Phase::Local(SourceResult::Refresh {
                paths: Vec::new(),
                failed: Vec::new(),
            }),
            _reservation: reservation,
        };
        match pending.request.command.as_str() {
            "file.revert" => {
                let path = pending
                    .request
                    .path
                    .clone()
                    .ok_or("The document has never been saved")?;
                validate_read_path(&path)?;
                pending.phase = Phase::Read(read_wire(&path, false, false));
            }
            "layer.smartObjects.replaceContents" | "layer.smartObjects.relinkToFile" => {
                let path = pending
                    .request
                    .params
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                validate_read_path(path)?;
                pending.phase = Phase::Read(read_wire(
                    path,
                    path.is_empty(),
                    pending.request.command.ends_with("relinkToFile"),
                ));
            }
            "layer.smartObjects.updateAllModifiedContent" => {
                for (_, _, layer) in pending.request.document.walk() {
                    if let LayerContent::Smart(smart) = &layer.content
                        && let SmartSource::Linked { path } = &smart.source
                        && photocraft_io::linked::find_linked_file(
                            &pending.request.document.metadata,
                            path,
                        )
                        .is_none()
                    {
                        add_read(&mut pending.sources, path, layer.id)?;
                    }
                    if pending.sources.len() > MAX_SOURCE_FILES {
                        return Err("Too many linked sources in one update".into());
                    }
                }
                pending.phase = pending
                    .sources
                    .first()
                    .map(|source| Phase::Read(read_wire(&source.path, false, true)))
                    .unwrap_or(Phase::Local(SourceResult::Refresh {
                        paths: Vec::new(),
                        failed: Vec::new(),
                    }));
            }
            _ => {
                let layer = pending
                    .request
                    .layer
                    .ok_or("No Smart Object layer was captured")?;
                let LayerContent::Smart(smart) = &pending
                    .request
                    .document
                    .layer(layer)
                    .ok_or("The Smart Object layer is missing")?
                    .content
                else {
                    return Err("The layer is not a Smart Object".into());
                };
                if let SmartSource::Linked { path } = &smart.source
                    && photocraft_io::linked::find_linked_file(
                        &pending.request.document.metadata,
                        path,
                    )
                    .is_none()
                {
                    add_read(&mut pending.sources, path, layer)?;
                    let retain = pending.request.command.ends_with("updateModifiedContent");
                    pending.phase = Phase::Read(read_wire(path, false, retain));
                } else if pending.request.command.ends_with("updateModifiedContent") {
                    pending.phase = Phase::Local(SourceResult::Refresh {
                        paths: Vec::new(),
                        failed: Vec::new(),
                    });
                } else {
                    let (name, bytes) = photocraft_engine::smart_cmds::source_bytes(
                        &pending.request.document.metadata,
                        &smart.source,
                    )
                    .ok_or("The Smart Object contents are unavailable")?;
                    if bytes.is_empty() || bytes.len() as u64 > MAX_SOURCE_BYTES {
                        return Err("Smart Object contents exceed supported limits".into());
                    }
                    pending.phase = if pending.request.command.ends_with("editContents")
                        || pending.request.command.ends_with("convertToEmbedded")
                    {
                        Phase::Local(SourceResult::Read {
                            path: String::new(),
                            name,
                            bytes: bytes.as_ref().clone(),
                        })
                    } else {
                        Phase::Save(self.save_wire(
                            &name,
                            &bytes,
                            pending.request.command.ends_with("convertToLinked"),
                        )?)
                    };
                }
            }
        }
        Ok(pending)
    }

    fn save_wire(&self, name: &str, bytes: &[u8], retain_source: bool) -> Result<Wire, String> {
        if bytes.is_empty() || bytes.len() as u64 > MAX_SOURCE_BYTES {
            return Err("Smart Object contents exceed supported limits".into());
        }
        let name = safe_name(name)?;
        let (id, directory) = loop {
            let id = crate::file_requests::next_id();
            let directory = self.staging.join(id.to_string());
            match std::fs::create_dir(&directory) {
                Ok(()) => break (id, directory),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.to_string()),
            }
        };
        let path = directory.join(&name);
        if let Err(error) = photocraft_format::atomic_write(&path, bytes) {
            let _ = std::fs::remove_dir(&directory);
            return Err(error.to_string());
        }
        Ok(Wire {
            id,
            path: path.to_string_lossy().into_owned(),
            name,
            previous: String::new(),
            choose: true,
            delivered: false,
            retain_source,
        })
    }

    pub fn has_request(&self, id: u64) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).pending.as_ref()
            .is_some_and(|pending| matches!(&pending.phase, Phase::Read(wire) | Phase::Save(wire) if wire.id == id))
    }

    pub fn take_json(&self) -> String {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(pending) = &mut state.pending else {
            return String::new();
        };
        let save = matches!(pending.phase, Phase::Save(_));
        let wire = match &mut pending.phase {
            Phase::Read(wire) | Phase::Save(wire) => wire,
            Phase::Local(_) => return String::new(),
        };
        if wire.delivered {
            return String::new();
        }
        wire.delivered = true;
        json!({"id":wire.id,"kind":if save {"save"} else {"open"},"path":wire.path,"suggestedName":wire.name,
            "previousPath":wire.previous,"chooseDestination":wire.choose,"intent":pending.request.command,
            "allowFormatChoice":false,"allowedExtensions":Path::new(&wire.name).extension().map(|ext| vec![ext.to_string_lossy().to_ascii_lowercase()]).unwrap_or_default(),
            "retainSource":wire.retain_source}).to_string()
    }

    pub fn prepare_save(&self, id: u64, destination: &str) -> Result<String, String> {
        let name = safe_name(destination)?;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(Pending {
            phase: Phase::Save(wire),
            ..
        }) = &mut state.pending
        else {
            return Err("Smart publication is no longer pending".into());
        };
        if wire.id != id || !wire.delivered {
            return Err("Smart publication identity does not match".into());
        }
        let ext = |name: &str| {
            Path::new(name)
                .extension()
                .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
        };
        if ext(&name) != ext(&wire.name) {
            return Err("The source file format must be preserved".into());
        }
        if name != wire.name {
            let old = Path::new(&wire.path);
            let next = old.parent().ok_or("Missing publication owner")?.join(&name);
            std::fs::rename(old, &next)
                .map_err(|e| format!("Cannot name Smart Object export: {e}"))?;
            wire.path = next.to_string_lossy().into_owned();
            wire.name = name;
        }
        Ok(wire.path.clone())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn complete(
        &self,
        app: &mut PhotocraftApp,
        ctx: &egui::Context,
        id: u64,
        result: &str,
        path: &str,
        name: &str,
        error: &str,
    ) -> Result<(), String> {
        if !matches!(result, "success" | "cancel" | "error") {
            return Err("Unknown source operation result".into());
        }
        let mut pending = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let valid = state.pending.as_ref().is_some_and(|pending| matches!(&pending.phase, Phase::Read(wire) | Phase::Save(wire) if wire.id == id && wire.delivered));
            if !valid {
                return Err("Source operation is no longer pending".into());
            }
            state.pending.take().ok_or("Missing source operation")?
        };
        let outcome = if result != "success" {
            if let Phase::Save(wire) = &pending.phase
                && !(result == "error" && path == wire.path && self.valid_stage(wire).is_ok())
            {
                self.remove_stage(wire);
            }
            Err(if result == "cancel" {
                "File operation cancelled".into()
            } else if error.is_empty() {
                "The file operation failed".into()
            } else {
                error.chars().take(4096).collect()
            })
        } else {
            self.accept(app, &mut pending, path, name)
        };
        let outcome = if result != "cancel"
            && pending
                .request
                .command
                .ends_with("updateAllModifiedContent")
            && matches!(pending.phase, Phase::Read(_))
            && pending.next < pending.sources.len()
        {
            match outcome {
                Err(error) => {
                    if let Err(target_error) = target_valid(app, &pending.request) {
                        Err(target_error)
                    } else {
                        if let Some(source) = pending.sources.get(pending.next) {
                            pending.failed.extend(source.layers.iter().copied());
                        }
                        pending.failures.push(error);
                        self.advance_refresh(app, &mut pending)
                    }
                }
                success => success,
            }
        } else {
            outcome
        };
        match outcome {
            Ok(Some(status)) => {
                let partial = (!pending.failures.is_empty()).then(|| pending.failures.join("; "));
                app.session
                    .source_outcome(&pending.request, partial.as_deref(), false);
                self.clean_reads(&pending);
                app.sync_views();
                app.ui.status = status;
                app.ui.status_error = !pending.failures.is_empty();
            }
            Ok(None) => {
                self.state.lock().unwrap_or_else(|e| e.into_inner()).pending = Some(pending);
                ctx.request_repaint();
                return Ok(());
            }
            Err(message) => {
                app.session
                    .source_outcome(&pending.request, Some(&message), result == "cancel");
                self.clean_reads(&pending);
                app.ui.status = i18n::t(&message).to_string();
                app.ui.status_error = result != "cancel";
                ctx.request_repaint();
                if result == "success" {
                    return Err(message);
                }
            }
        }
        ctx.request_repaint();
        Ok(())
    }

    fn accept(
        &self,
        app: &mut PhotocraftApp,
        pending: &mut Pending,
        path: &str,
        name: &str,
    ) -> Result<Option<String>, String> {
        match &pending.phase {
            Phase::Read(_) => {
                let name = safe_name(name)?;
                target_valid(app, &pending.request)?;
                let bytes = self.read_owned(path)?;
                pending.bytes_read = pending
                    .bytes_read
                    .checked_add(bytes.len() as u64)
                    .ok_or("Source byte count overflow")?;
                if pending.bytes_read > MAX_SOURCE_BYTES {
                    return Err("Linked sources exceed supported limits".into());
                }
                pending.owned_reads.push(path.into());
                let command = pending.request.command.as_str();
                if command.ends_with("updateModifiedContent")
                    || command.ends_with("updateAllModifiedContent")
                {
                    // Invalid input must not replace a valid linked source path.
                    photocraft_engine::smart_cmds::decode_source(&name, &bytes)
                        .map_err(|e| e.to_string())?;
                    let source = pending
                        .sources
                        .get(pending.next)
                        .ok_or("Missing captured linked source")?;
                    for layer in &source.layers {
                        pending.refreshed.push((*layer, path.into()));
                    }
                    return self.advance_refresh(app, pending);
                } else if command.ends_with("exportContents")
                    || command.ends_with("convertToLinked")
                {
                    pending.phase = Phase::Save(self.save_wire(
                        &name,
                        &bytes,
                        command.ends_with("convertToLinked"),
                    )?);
                    return Ok(None);
                } else {
                    self.apply(
                        app,
                        &pending.request,
                        SourceResult::Read {
                            path: path.into(),
                            name,
                            bytes,
                        },
                    )?;
                }
                Ok(Some(i18n::fmt(
                    completion_template(command),
                    &[("name", &pending.request.document.name)],
                )))
            }
            Phase::Save(wire) => {
                self.valid_stage(wire)?;
                if pending.request.command.ends_with("convertToLinked") {
                    self.apply(app, &pending.request, SourceResult::Published(wire.path.clone()))
                        .map_err(|error| format!("The contents were published, but conversion was not applied: {error}. The original source was kept"))?;
                    Ok(Some(i18n::fmt(i18n::t("Linked {name}"), &[("name", &wire.name)])))
                } else {
                    // Publication exported the captured bytes; later edits/closure do
                    // not invalidate an export or mutate the newer document.
                    if target_valid(app, &pending.request).is_ok() {
                        self.apply(
                            app,
                            &pending.request,
                            SourceResult::Published(wire.path.clone()),
                        )?;
                    }
                    self.remove_stage(wire);
                    Ok(Some(i18n::fmt(i18n::t("Exported {name}"), &[("name", &wire.name)])))
                }
            }
            Phase::Local(_) => Err("Source completion does not match a platform request".into()),
        }
    }

    fn advance_refresh(
        &self,
        app: &mut PhotocraftApp,
        pending: &mut Pending,
    ) -> Result<Option<String>, String> {
        pending.next += 1;
        if let Some(next) = pending.sources.get(pending.next) {
            pending.phase = Phase::Read(read_wire(&next.path, false, true));
            return Ok(None);
        }
        let response = SourceResult::Refresh {
            paths: std::mem::take(&mut pending.refreshed),
            failed: pending.failed.clone(),
        };
        self.apply(app, &pending.request, response)?;
        let status = if pending.failures.is_empty() {
            i18n::fmt(i18n::t("Updated {name}"), &[("name", &pending.request.document.name)])
        } else {
            i18n::fmt(
                i18n::t("{count} linked source(s) could not be updated: {errors}"),
                &[("count", &pending.failures.len().to_string()), ("errors", &pending.failures.join("; "))],
            )
        };
        Ok(Some(status))
    }

    fn apply(
        &self,
        app: &mut PhotocraftApp,
        request: &SourceCommand,
        response: SourceResult,
    ) -> Result<(), String> {
        app.session
            .complete_source(request, response)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub fn finish_frame(&self, app: &mut PhotocraftApp) {
        let local = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state
                .pending
                .as_ref()
                .is_some_and(|pending| matches!(pending.phase, Phase::Local(_)))
            {
                state.pending.take()
            } else {
                None
            }
        };
        if let Some(mut pending) = local {
            let Phase::Local(response) = std::mem::replace(
                &mut pending.phase,
                Phase::Local(SourceResult::Refresh {
                    paths: Vec::new(),
                    failed: Vec::new(),
                }),
            ) else {
                return;
            };
            let result = self.apply(app, &pending.request, response);
            app.session.source_outcome(
                &pending.request,
                result.as_ref().err().map(String::as_str),
                false,
            );
            self.clean_reads(&pending);
            app.sync_views();
            let status = result.map(|_| {
                i18n::fmt(
                    completion_template(&pending.request.command),
                    &[("name", &pending.request.document.name)],
                )
            });
            app.ui.status_error = status.is_err();
            app.ui.status = status.unwrap_or_else(|error| i18n::t(&error).to_string());
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ctx) = &state.context {
                ctx.request_repaint();
            }
        }
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(pending) = &state.pending {
            app.ui.status = if matches!(pending.phase, Phase::Save(_)) {
                i18n::t("Waiting to publish Smart Object contents")
            } else {
                i18n::t("Reading the selected source")
            }
            .into();
            app.ui.status_error = false;
        }
    }

    fn read_owned(&self, text: &str) -> Result<Vec<u8>, String> {
        crate::folder_requests::absolute_path(text)?;
        if self
            .files
            .canonicalize()
            .map_err(|_| "Application source storage is unavailable")?
            != self.canonical_files
        {
            return Err("Application source storage changed".into());
        }
        let path = Path::new(text);
        let root = self.files.join("PhotoCraft/Documents/imports");
        if path.parent().and_then(Path::parent) != Some(root.as_path()) {
            return Err("Source must belong to an owned fresh import".into());
        }
        no_links(&self.files, path)?;
        let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_SOURCE_BYTES {
            return Err("Source file exceeds supported limits or is empty".into());
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|e| e.to_string())?
            .take(MAX_SOURCE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_SOURCE_BYTES || bytes.len() as u64 != metadata.len() {
            return Err("Source changed while it was being read".into());
        }
        Ok(bytes)
    }

    fn valid_stage(&self, wire: &Wire) -> Result<(), String> {
        let path = Path::new(&wire.path);
        if path.parent() != Some(self.staging.join(wire.id.to_string()).as_path()) {
            return Err("Publication does not belong to this source request".into());
        }
        no_links(&self.canonical_files, path)?;
        let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_SOURCE_BYTES {
            return Err("Publication is not a supported regular file".into());
        }
        Ok(())
    }

    fn remove_stage(&self, wire: &Wire) {
        if self.valid_stage(wire).is_ok() {
            let _ = std::fs::remove_file(&wire.path);
            if let Some(parent) = Path::new(&wire.path).parent() {
                let _ = std::fs::remove_dir(parent);
            }
        }
    }

    fn clean_reads(&self, pending: &Pending) {
        let retain = pending.request.command.ends_with("relinkToFile")
            || pending.request.command.ends_with("updateModifiedContent")
            || pending
                .request
                .command
                .ends_with("updateAllModifiedContent");
        if retain {
            return;
        }
        for text in &pending.owned_reads {
            let path = Path::new(text);
            if path.parent().and_then(Path::parent)
                == Some(self.files.join("PhotoCraft/Documents/imports").as_path())
                && no_links(&self.files, path).is_ok()
            {
                let _ = std::fs::remove_file(path);
                if let Some(parent) = path.parent() {
                    let _ = std::fs::remove_dir(parent);
                }
            }
        }
    }
}

fn target_valid(app: &PhotocraftApp, request: &SourceCommand) -> Result<(), String> {
    app.session
        .validate_source_owner(request)
        .map_err(|error| error.to_string())
}
fn read_wire(previous: &str, choose: bool, retain_source: bool) -> Wire {
    Wire {
        id: crate::file_requests::next_id(),
        path: String::new(),
        name: String::new(),
        previous: previous.into(),
        choose,
        delivered: false,
        retain_source,
    }
}
fn validate_read_path(path: &str) -> Result<(), String> {
    if path.len() > 8192 || path.contains('\0') {
        return Err("Linked source identity is invalid".into());
    }
    Ok(())
}
fn add_read(reads: &mut Vec<ReadSource>, path: &str, layer: LayerId) -> Result<(), String> {
    validate_read_path(path)?;
    if let Some(source) = reads.iter_mut().find(|source| source.path == path) {
        source.layers.push(layer);
    } else {
        reads.push(ReadSource {
            path: path.into(),
            layers: vec![layer],
        });
    }
    Ok(())
}
fn safe_name(name: &str) -> Result<String, String> {
    if name.is_empty()
        || name.len() > 240
        || name.contains(['/', '\\', '\0'])
        || matches!(name, "." | "..")
    {
        return Err("The source file name is invalid".into());
    }
    Ok(name.into())
}
fn no_links(root: &Path, path: &Path) -> Result<(), String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "Source is outside the app's owned storage")?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err("Source storage path is invalid".into());
        }
        current.push(component);
        if std::fs::symlink_metadata(&current)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("Source storage contains a symbolic link".into());
        }
    }
    Ok(())
}
fn completion_template(command: &str) -> &'static str {
    i18n::t(match command {
        "file.revert" => "Reverted {name}",
        "layer.smartObjects.editContents" => "Opened contents of {name}",
        "layer.smartObjects.replaceContents" => "Replaced contents of {name}",
        "layer.smartObjects.relinkToFile" => "Relinked {name}",
        "layer.smartObjects.convertToEmbedded" => "Embedded {name}",
        _ => "Updated {name}",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_requests::FileRequests;
    use photocraft_doc::DocId;

    struct Fixture {
        root: PathBuf,
        requests: FileRequests,
        ctx: egui::Context,
        app: PhotocraftApp,
    }
    impl Fixture {
        fn new() -> Result<Self, String> {
            let root = std::env::temp_dir().join(format!(
                "photocraft-source-{}",
                crate::file_requests::next_id()
            ));
            std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
            let requests = FileRequests::new(&root)?;
            let ctx = egui::Context::default();
            PhotocraftApp::setup_context(&ctx, photocraft_ui_egui::theme::ThemeKind::Pro);
            ctx.enable_accesskit();
            let mut app = PhotocraftApp::new(
                Session::new(),
                crate::services::services_with_files(&root, &requests),
            );
            requests.attach_sources(&mut app.session, &ctx);
            app.run("file.new", json!({"width":2,"height":2,"name":"working"}))?;
            Ok(Self {
                root,
                requests,
                ctx,
                app,
            })
        }
        fn png(&self, rgba: [u8; 4]) -> Result<Vec<u8>, String> {
            let hooks = crate::services::services(&self.root);
            (hooks.encode_png.ok_or("missing PNG encoder")?)(2, 2, &rgba.repeat(4))
        }
        fn imported(&self, bytes: &[u8], name: &str) -> Result<String, String> {
            let dir = self
                .root
                .join("PhotoCraft/Documents/imports")
                .join(format!("{}-source", crate::file_requests::next_id()));
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let path = dir.join(name);
            std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
            Ok(path.to_string_lossy().into_owned())
        }
        fn menu(&mut self, id: &str, params: Value) -> Result<(), String> {
            photocraft_ui_egui::menus::invoke(&mut self.app, &self.ctx, id, params)?;
            if id == "file.revert" && !self.requests.is_pending() {
                self.confirm_revert()?;
            }
            self.requests.finish_frame(&mut self.app);
            Ok(())
        }
        fn confirm_revert(&mut self) -> Result<(), String> {
            fn center(shape: &egui::Shape) -> Option<egui::Pos2> {
                match shape {
                    egui::Shape::Text(text) if text.galley.job.text == "Revert" => {
                        Some(text.pos + text.galley.size() / 2.0)
                    }
                    egui::Shape::Vec(shapes) => shapes.iter().find_map(center),
                    _ => None,
                }
            }
            let input = |events| egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                events,
                ..Default::default()
            };
            let output = self.ctx.run_ui(input(Vec::new()), |ui| {
                photocraft_ui_egui::discard_ui::show(&mut self.app, ui.ctx())
            });
            output.drop_without_applying_deltas();
            let output = self.ctx.run_ui(input(Vec::new()), |ui| {
                photocraft_ui_egui::discard_ui::show(&mut self.app, ui.ctx())
            });
            let painted = output.shapes.iter().find_map(|shape| center(&shape.shape));
            let button = output
                .platform_output
                .accesskit_update
                .as_ref()
                .and_then(|tree| {
                    tree.nodes.iter().find_map(|(id, node)| {
                        (node.role() == egui::accesskit::Role::Button
                            && node.label() == Some("Revert"))
                        .then_some(*id)
                    })
                });
            output.drop_without_applying_deltas();
            painted.ok_or("Original Revert confirmation button was not painted")?;
            let button = button.ok_or("Original Revert accessible button was not available")?;
            let output = self.ctx.run_ui(
                input(vec![egui::Event::AccessKitActionRequest(
                    egui::accesskit::ActionRequest {
                        action: egui::accesskit::Action::Click,
                        target_tree: egui::accesskit::TreeId::ROOT,
                        target_node: button,
                        data: None,
                    },
                )]),
                |ui| photocraft_ui_egui::discard_ui::show(&mut self.app, ui.ctx()),
            );
            output.drop_without_applying_deltas();
            if !self.requests.is_pending() {
                return Err(format!(
                    "Revert confirmation did not queue a source operation: {}",
                    self.app.ui.status
                ));
            }
            Ok(())
        }
        fn request(&self) -> Result<Value, String> {
            serde_json::from_str(&self.requests.take_json()).map_err(|e| e.to_string())
        }
        fn complete(
            &mut self,
            request: &Value,
            result: &str,
            path: &str,
            name: &str,
        ) -> Result<(), String> {
            self.requests.complete_with_path(
                &mut self.app,
                &self.ctx,
                request["id"].as_u64().ok_or("no id")?,
                result,
                path,
                name,
                "source permission failed",
            )
        }
        fn place(
            &mut self,
            bytes: &[u8],
            name: &str,
            linked: Option<String>,
        ) -> Result<LayerId, String> {
            let response = photocraft_engine::file_cmds::place_bytes(
                &mut self.app.session,
                name,
                bytes.to_vec(),
                linked,
                &json!({"fit":false,"center":[1,1]}),
            )
            .map_err(|e| e.to_string())?;
            self.app.sync_views();
            Ok(LayerId(response["layer"].as_u64().ok_or("missing layer")?))
        }
        fn pixel(&mut self, document: DocId) -> Result<Vec<f64>, String> {
            let previous = self.app.session.active_index();
            let index = self
                .app
                .session
                .documents()
                .iter()
                .position(|state| state.doc.id == document)
                .ok_or("document closed")?;
            self.app.session.set_active(index);
            let result = self
                .app
                .session
                .execute("document.pixel", json!({"x":0,"y":0}))
                .map_err(|e| e.to_string());
            if let Some(index) = previous {
                self.app.session.set_active(index);
            }
            serde_json::from_value(result?).map_err(|e| e.to_string())
        }
        fn smart(&self, layer: LayerId) -> Result<&photocraft_doc::SmartObject, String> {
            let LayerContent::Smart(smart) = &self
                .app
                .session
                .active()
                .ok_or("no document")?
                .doc
                .layer(layer)
                .ok_or("no layer")?
                .content
            else {
                return Err("not smart".into());
            };
            Ok(smart)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn original_script_dispatch_resumes_through_real_file_mailbox_without_reentrant_admission()
    -> Result<(), String> {
        let mut f = Fixture::new()?;
        f.app.session.automation.enabled = true;
        let old = f.png([0, 0, 0, 255])?;
        let old_path = f.imported(&old, "script-owner.png")?;
        let owner = f.app.session.add_document(
            photocraft_io::import("script-owner.png", &old)
                .map_err(|error| error.to_string())?
                .document,
            Some(old_path),
        );
        let id = f.app.session.active().ok_or("no owner")?.doc.id;
        let response=f.app.run("file.scripts.browse",json!({"steps":[["file.scripts.browse",{"steps":[["file.revert",{}],["image.adjustments.invert",{}]]}],["file.revert",{}]]}))?;
        let job = photocraft_engine::jobs::JobId(response["job"].as_u64().ok_or("no script job")?);
        f.app.session.poll_jobs();
        let first = f.request()?;
        assert_eq!(first["intent"], "file.revert");
        assert!(f.requests.is_pending());
        f.app.run("file.new", json!({"width":1,"height":1}))?;
        let other = f.app.session.active().ok_or("no other")?.doc.id;
        let white = f.png([255, 255, 255, 255])?;
        let path = f.imported(&white, "fresh-script.png")?;
        f.complete(&first, "success", &path, "fresh-script.png")?;
        assert!(
            !f.requests.is_pending(),
            "first transaction must release its reservation before next source admission"
        );
        f.app.session.poll_jobs();
        let second = f.request()?;
        assert_ne!(first["id"], second["id"]);
        f.complete(&second, "cancel", "", "")?;
        f.app.session.poll_jobs();
        assert_eq!(
            f.app
                .session
                .jobs_with_recent()
                .into_iter()
                .find(|info| info.id == job)
                .ok_or("no final job")?
                .state,
            "cancelled"
        );
        assert_eq!(f.app.session.active().ok_or("no active")?.doc.id, other);
        assert_eq!(
            f.app
                .session
                .documents()
                .get(owner)
                .ok_or("lost owner")?
                .doc
                .id,
            id
        );
        assert!(
            f.pixel(id)?.first().is_some_and(|red| *red < 0.01),
            "fresh read then original Invert ran on owner; cancelled second Revert did not mutate it"
        );
        Ok(())
    }

    #[test]
    fn revert_uses_fresh_bytes_and_original_document_even_after_a_tab_switch() -> Result<(), String>
    {
        let mut f = Fixture::new()?;
        let old = f.png([255, 0, 0, 255])?;
        let old_path = f.imported(&old, "original.png")?;
        let doc = photocraft_io::import("original.png", &old)
            .map_err(|e| e.to_string())?
            .document;
        let original = f.app.session.add_document(doc, Some(old_path.clone()));
        f.app.sync_views();
        let id = f.app.session.active().ok_or("no document")?.doc.id;
        f.app.run("image.adjustments.invert", json!({}))?;
        let before = f.pixel(id)?;
        f.menu("file.revert", json!({}))?;
        let request = f.request()?;
        assert_eq!(request["intent"], "file.revert");
        assert_eq!(request["previousPath"], old_path);
        assert_eq!(request["chooseDestination"], false);
        assert_eq!(request["retainSource"], false);
        let latest = f.png([0, 255, 0, 255])?;
        let latest_path = f.imported(&latest, "renamed.png")?;
        f.app
            .run("file.new", json!({"width":3,"height":3,"name":"another"}))?;
        let other = f.app.session.active().ok_or("no other")?.doc.id;
        f.complete(&request, "success", &latest_path, "renamed.png")?;
        assert_eq!(f.app.session.active().ok_or("no active")?.doc.id, other);
        let state = f
            .app
            .session
            .documents()
            .get(original)
            .ok_or("no original")?;
        assert_eq!(state.doc.id, id);
        assert_eq!(state.path.as_deref(), Some(old_path.as_str()));
        assert_eq!(state.doc.name, "original.png");
        assert!(!state.is_dirty());
        let pixel = f.pixel(id)?;
        assert!(pixel[1] > 0.99 && pixel[0] < 0.01);
        assert!(!Path::new(&latest_path).exists());
        assert_eq!(std::fs::read(&old_path).map_err(|e| e.to_string())?, old);
        f.app.session.set_active(original);
        f.app.run("edit.undo", json!({}))?;
        assert_eq!(f.pixel(id)?, before);
        assert!(f.app.session.active().ok_or("no active")?.is_dirty());
        Ok(())
    }

    #[test]
    fn fresh_revert_does_not_use_the_old_copy_as_an_existence_or_content_oracle()
    -> Result<(), String> {
        let mut f = Fixture::new()?;
        let old = f.png([255, 0, 0, 255])?;
        let latest = f.png([0, 255, 0, 255])?;
        let old_path = f.imported(&old, "source.png")?;
        f.app.session.add_document(
            photocraft_io::import("source.png", &old)
                .map_err(|e| e.to_string())?
                .document,
            Some(old_path.clone()),
        );
        f.app.sync_views();
        let id = f.app.session.active().ok_or("no doc")?.doc.id;
        std::fs::remove_file(&old_path).map_err(|e| e.to_string())?;
        f.menu("file.revert", json!({}))?;
        let request = f.request()?;
        assert_eq!(request["previousPath"], old_path);
        let fresh = f.imported(&latest, "source.png")?;
        f.complete(&request, "success", &fresh, "source.png")?;
        assert!(f.pixel(id)?[1] > 0.99);
        assert!(!f.app.session.active().ok_or("no doc")?.is_dirty());
        assert!(!Path::new(&old_path).exists());
        Ok(())
    }

    #[test]
    fn revert_cancel_failure_and_late_result_preserve_document_and_history() -> Result<(), String> {
        for result in ["cancel", "error", "changed", "closed"] {
            let mut f = Fixture::new()?;
            let bytes = f.png([40, 70, 90, 255])?;
            let old = f.imported(&bytes, "old.png")?;
            f.app.session.add_document(
                photocraft_io::import("old.png", &bytes)
                    .map_err(|e| e.to_string())?
                    .document,
                Some(old),
            );
            f.app.sync_views();
            f.app.run("image.adjustments.invert", json!({}))?;
            f.menu("file.revert", json!({}))?;
            let request = f.request()?;
            if result == "changed" {
                f.app.run("image.adjustments.invert", json!({}))?;
            }
            if result == "closed" {
                let index = f.app.session.active_index().ok_or("missing active")?;
                f.app.session.close(index);
                f.app.sync_views();
            }
            let docs = f
                .app
                .session
                .documents()
                .iter()
                .map(|s| {
                    (
                        s.doc.id,
                        s.doc.clone(),
                        s.revision,
                        s.saved_revision,
                        s.path.clone(),
                    )
                })
                .collect::<Vec<_>>();
            let journal = f.app.session.journal.len();
            if matches!(result, "changed" | "closed") {
                let path = f.imported(&bytes, "fresh.png")?;
                assert!(f.complete(&request, "success", &path, "fresh.png").is_err());
                assert!(f.app.ui.status_error);
            } else {
                f.complete(&request, result, "", "")?;
            }
            assert_eq!(f.app.session.journal.len(), journal);
            for (state, old) in f.app.session.documents().iter().zip(docs) {
                assert_eq!(
                    (
                        state.doc.id,
                        state.revision,
                        state.saved_revision,
                        state.path.clone()
                    ),
                    (old.0, old.2, old.3, old.4)
                );
                assert!(Arc::ptr_eq(&state.doc, &old.1));
            }
            assert!(!f.requests.is_pending());
        }
        Ok(())
    }

    #[test]
    fn linked_update_refreshes_pixels_in_one_undo_step_and_keeps_layer_identity()
    -> Result<(), String> {
        let mut f = Fixture::new()?;
        let red = f.png([255, 0, 0, 255])?;
        let old = f.imported(&red, "linked.png")?;
        let layer = f.place(&red, "linked.png", Some(old.clone()))?;
        let document = f.app.session.active().ok_or("no document")?.doc.id;
        let transform = f.smart(layer)?.transform;
        let revision = f.app.session.active().ok_or("no document")?.revision;
        f.menu(
            "layer.smartObjects.updateModifiedContent",
            json!({"layer":layer.0}),
        )?;
        let request = f.request()?;
        assert_eq!(request["previousPath"], old);
        assert_eq!(request["retainSource"], true);
        let green = f.png([0, 255, 0, 255])?;
        let fresh = f.imported(&green, "linked.png")?;
        f.complete(&request, "success", &fresh, "linked.png")?;
        assert_eq!(
            f.app.session.active().ok_or("no document")?.revision,
            revision + 1
        );
        assert_eq!(f.smart(layer)?.transform, transform);
        assert!(matches!(&f.smart(layer)?.source,SmartSource::Linked {path} if path==&fresh));
        let pixel = f.pixel(document)?;
        assert!(pixel[1] > 0.99 && pixel[0] < 0.01);
        f.app.run("edit.undo", json!({}))?;
        assert!(matches!(&f.smart(layer)?.source,SmartSource::Linked {path} if path==&old));
        let pixel = f.pixel(document)?;
        assert!(pixel[0] > 0.99 && pixel[1] < 0.01);
        Ok(())
    }

    #[test]
    fn update_all_reports_a_failed_source_and_does_not_refresh_it_from_cache() -> Result<(), String>
    {
        let mut f = Fixture::new()?;
        let red = f.png([255, 0, 0, 255])?;
        let green = f.png([0, 255, 0, 255])?;
        let old_a = f.imported(&red, "a.png")?;
        let old_b = f.imported(&red, "b.png")?;
        let a = f.place(&red, "a.png", Some(old_a.clone()))?;
        let b = f.place(&red, "b.png", Some(old_b.clone()))?;
        let revision = f.app.session.active().ok_or("no document")?.revision;
        f.menu("layer.smartObjects.updateAllModifiedContent", json!({}))?;
        let first = f.request()?;
        let success_path = if first["previousPath"] == old_a {
            old_b.clone()
        } else {
            old_a.clone()
        };
        f.complete(&first, "error", "", "")?;
        assert!(f.requests.is_pending());
        let second = f.request()?;
        assert_eq!(second["previousPath"], success_path);
        let fresh = f.imported(&green, "updated.png")?;
        f.complete(&second, "success", &fresh, "updated.png")?;
        assert!(f.app.ui.status_error);
        assert!(f.app.ui.status.contains("1 linked source"));
        assert_eq!(
            f.app.session.active().ok_or("no doc")?.revision,
            revision + 1
        );
        let (good, bad, old_bad) = if success_path == old_a {
            (a, b, &old_b)
        } else {
            (b, a, &old_a)
        };
        assert!(matches!(&f.smart(good)?.source,SmartSource::Linked {path} if path==&fresh));
        assert!(matches!(&f.smart(bad)?.source,SmartSource::Linked {path} if path==old_bad));
        assert!(!f.requests.is_pending());
        f.app.run("edit.undo", json!({}))?;
        assert!(matches!(&f.smart(a)?.source,SmartSource::Linked {path} if path==&old_a));
        assert!(matches!(&f.smart(b)?.source,SmartSource::Linked {path} if path==&old_b));
        Ok(())
    }

    #[test]
    fn all_failed_or_invalid_linked_sources_do_not_add_an_edit_or_claim_success()
    -> Result<(), String> {
        let mut f = Fixture::new()?;
        let red = f.png([255, 0, 0, 255])?;
        let old = f.imported(&red, "valid.png")?;
        let layer = f.place(&red, "valid.png", Some(old.clone()))?;
        let revision = f.app.session.active().ok_or("no doc")?.revision;
        f.menu("layer.smartObjects.updateAllModifiedContent", json!({}))?;
        let request = f.request()?;
        let journal = f.app.session.journal.len();
        let invalid = f.imported(b"not an image", "corrupt.png")?;
        f.complete(&request, "success", &invalid, "corrupt.png")?;
        assert!(f.app.ui.status_error);
        assert!(f.app.ui.status.contains("could not be updated"));
        assert_eq!(f.app.session.journal.len(), journal);
        assert_eq!(f.app.session.active().ok_or("no doc")?.revision, revision);
        assert!(matches!(&f.smart(layer)?.source,SmartSource::Linked{path} if path==&old));
        assert!(!f.requests.is_pending());
        Ok(())
    }

    #[test]
    fn smart_replace_relink_and_convert_to_embedded_use_original_layer_and_fresh_contents()
    -> Result<(), String> {
        let mut f = Fixture::new()?;
        let red = f.png([255, 0, 0, 255])?;
        let green = f.png([0, 255, 0, 255])?;
        let layer = f.place(&red, "a.png", None)?;
        let id = f.app.session.active().ok_or("no doc")?.doc.id;
        for command in [
            "layer.smartObjects.replaceContents",
            "layer.smartObjects.relinkToFile",
        ] {
            f.menu(command, json!({"layer":layer.0}))?;
            let request = f.request()?;
            assert_eq!(request["chooseDestination"], true);
            let fresh = f.imported(&green, "replacement.png")?;
            f.complete(&request, "success", &fresh, "replacement.png")?;
            assert_eq!(f.app.session.active().ok_or("no doc")?.doc.id, id);
            assert!(f.pixel(id)?[1] > 0.99);
        }
        f.menu(
            "layer.smartObjects.convertToEmbedded",
            json!({"layer":layer.0}),
        )?;
        let request = f.request()?;
        assert_eq!(request["chooseDestination"], false);
        let fresh = f.imported(&red, "fresh-linked.png")?;
        f.complete(&request, "success", &fresh, "fresh-linked.png")?;
        assert!(
            matches!(&f.smart(layer)?.source,SmartSource::Embedded {bytes,..} if bytes.as_slice()==red)
        );
        assert!(f.pixel(id)?[0] > 0.99);
        Ok(())
    }

    #[test]
    fn relink_completion_stays_with_original_layer_when_another_document_is_active()
    -> Result<(), String> {
        let mut f = Fixture::new()?;
        let red = f.png([255, 0, 0, 255])?;
        let green = f.png([0, 255, 0, 255])?;
        let layer = f.place(&red, "original.png", None)?;
        let owner = f.app.session.active().ok_or("no doc")?.doc.id;
        f.menu("layer.smartObjects.relinkToFile", json!({"layer":layer.0}))?;
        let request = f.request()?;
        f.app
            .run("file.new", json!({"width":2,"height":2,"name":"other"}))?;
        let other_layer = f.place(&red, "other.png", None)?;
        let other = f.app.session.active().ok_or("no other")?.doc.id;
        let before = f.app.session.active().ok_or("no other")?.doc.clone();
        let fresh = f.imported(&green, "selected.png")?;
        f.complete(&request, "success", &fresh, "selected.png")?;
        assert_eq!(f.app.session.active().ok_or("no active")?.doc.id, other);
        assert!(Arc::ptr_eq(
            &f.app.session.active().ok_or("no active")?.doc,
            &before
        ));
        assert!(matches!(
            f.smart(other_layer)?.source,
            SmartSource::Embedded { .. }
        ));
        let index = f
            .app
            .session
            .documents()
            .iter()
            .position(|state| state.doc.id == owner)
            .ok_or("owner closed")?;
        f.app.session.set_active(index);
        assert!(matches!(&f.smart(layer)?.source,SmartSource::Linked{path} if path==&fresh));
        assert!(f.pixel(owner)?[1] > 0.99);
        Ok(())
    }

    #[test]
    fn smart_export_and_conversion_publish_original_bytes_and_only_ack_can_link()
    -> Result<(), String> {
        let mut f = Fixture::new()?;
        let bytes = f.png([10, 20, 30, 255])?;
        let layer = f.place(&bytes, "original.png", None)?;
        f.app.save_as(None)?;
        f.requests.finish_frame(&mut f.app);
        let parent_save = f.request()?;
        f.requests.prepare_save(
            &mut f.app,
            parent_save["id"].as_u64().ok_or("no id")?,
            "parent.psd",
        )?;
        f.complete(&parent_save, "success", "", "parent.psd")?;
        assert!(!f.app.session.active().ok_or("no doc")?.is_dirty());
        let revision = f.app.session.active().ok_or("no doc")?.revision;
        for result in ["cancel", "error"] {
            f.menu(
                "layer.smartObjects.convertToLinked",
                json!({"layer":layer.0}),
            )?;
            let request = f.request()?;
            assert_eq!(
                std::fs::read(request["path"].as_str().ok_or("no path")?)
                    .map_err(|e| e.to_string())?,
                bytes
            );
            assert!(
                f.requests
                    .prepare_save(
                        &mut f.app,
                        request["id"].as_u64().ok_or("no id")?,
                        "wrong.psd"
                    )
                    .is_err()
            );
            f.complete(&request, result, "", "")?;
            assert_eq!(f.app.session.active().ok_or("no doc")?.revision, revision);
            assert!(matches!(
                f.smart(layer)?.source,
                SmartSource::Embedded { .. }
            ));
        }
        f.menu(
            "layer.smartObjects.exportContents",
            json!({"layer":layer.0}),
        )?;
        let export = f.request()?;
        let stage = export["path"].as_str().ok_or("no path")?.to_string();
        assert_eq!(std::fs::read(&stage).map_err(|e| e.to_string())?, bytes);
        f.complete(&export, "success", "", "export.png")?;
        assert_eq!(f.app.session.active().ok_or("no doc")?.revision, revision);
        assert!(!Path::new(&stage).exists());
        f.menu(
            "layer.smartObjects.convertToLinked",
            json!({"layer":layer.0}),
        )?;
        let save = f.request()?;
        let stage = f.requests.prepare_save(
            &mut f.app,
            save["id"].as_u64().ok_or("no id")?,
            "actual-name.png",
        )?;
        f.complete(&save, "success", "", "actual-name.png")?;
        assert_eq!(
            f.app.session.active().ok_or("no doc")?.revision,
            revision + 1
        );
        assert!(matches!(&f.smart(layer)?.source,SmartSource::Linked{path} if path==&stage));
        assert!(Path::new(&stage).exists());
        assert_eq!(std::fs::read(&stage).map_err(|e| e.to_string())?, bytes);
        assert!(f.app.session.active().ok_or("no doc")?.is_dirty());
        assert!(
            !f.requests.is_published_save(&stage),
            "a linked resource is not authority to save the parent document in place"
        );
        Ok(())
    }

    #[test]
    fn edit_linked_contents_reads_fresh_source_then_keeps_original_parent_layer()
    -> Result<(), String> {
        let mut f = Fixture::new()?;
        let red = f.png([255, 0, 0, 255])?;
        let green = f.png([0, 255, 0, 255])?;
        let old = f.imported(&red, "old.png")?;
        let layer = f.place(&red, "old.png", Some(old))?;
        let parent = f.app.session.active().ok_or("no doc")?.doc.id;
        f.menu("layer.smartObjects.editContents", json!({"layer":layer.0}))?;
        let request = f.request()?;
        let fresh = f.imported(&green, "fresh.png")?;
        f.complete(&request, "success", &fresh, "fresh.png")?;
        let child = f.app.session.active().ok_or("no child")?.doc.id;
        assert_ne!(child, parent);
        assert!(f.pixel(child)?[1] > 0.99);
        f.app.run("image.adjustments.invert", json!({}))?;
        f.menu("layer.smartObjects.saveContents", json!({}))?;
        let index = f
            .app
            .session
            .documents()
            .iter()
            .position(|st| st.doc.id == parent)
            .ok_or("parent closed")?;
        f.app.session.set_active(index);
        assert!(matches!(
            f.smart(layer)?.source,
            SmartSource::Embedded { .. }
        ));
        let pixel = f.pixel(parent)?;
        assert!(pixel[0] > 0.99 && pixel[1] < 0.01 && pixel[2] > 0.99);
        Ok(())
    }

    #[test]
    fn invalid_source_path_and_publication_after_new_edit_cannot_replace_model()
    -> Result<(), String> {
        let mut f = Fixture::new()?;
        let bytes = f.png([100, 80, 50, 255])?;
        let layer = f.place(&bytes, "old.png", None)?;
        f.menu(
            "layer.smartObjects.replaceContents",
            json!({"layer":layer.0}),
        )?;
        let request = f.request()?;
        let outside = f.root.join("outside.png");
        std::fs::write(&outside, &bytes).map_err(|e| e.to_string())?;
        let revision = f.app.session.active().ok_or("no doc")?.revision;
        assert!(
            f.complete(
                &request,
                "success",
                outside.to_str().ok_or("non utf8")?,
                "outside.png"
            )
            .is_err()
        );
        assert_eq!(f.app.session.active().ok_or("no doc")?.revision, revision);
        f.menu(
            "layer.smartObjects.convertToLinked",
            json!({"layer":layer.0}),
        )?;
        let request = f.request()?;
        f.app.run("layer.new.layer", json!({}))?;
        let stage = request["path"].as_str().ok_or("no stage")?;
        assert!(f.complete(&request, "success", "", "source.png").is_err());
        assert!(
            f.app
                .ui
                .status
                .contains("published, but conversion was not applied")
        );
        assert!(Path::new(stage).exists());
        assert!(matches!(
            f.smart(layer)?.source,
            SmartSource::Embedded { .. }
        ));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn linked_import_ancestor_is_rejected_without_touching_either_document() -> Result<(), String> {
        let mut f = Fixture::new()?;
        let bytes = f.png([100, 80, 50, 255])?;
        let layer = f.place(&bytes, "old.png", None)?;
        let imports = f.root.join("PhotoCraft/Documents/imports");
        std::fs::create_dir(&imports).map_err(|e| e.to_string())?;
        let outside = f.root.join("outside");
        std::fs::create_dir(&outside).map_err(|e| e.to_string())?;
        std::fs::write(outside.join("source.png"), &bytes).map_err(|e| e.to_string())?;
        std::os::unix::fs::symlink(&outside, imports.join("linked")).map_err(|e| e.to_string())?;
        f.menu(
            "layer.smartObjects.replaceContents",
            json!({"layer":layer.0}),
        )?;
        let request = f.request()?;
        let revision = f.app.session.active().ok_or("no doc")?.revision;
        let path = imports.join("linked/source.png");
        assert!(
            f.complete(
                &request,
                "success",
                path.to_str().ok_or("non UTF8")?,
                "source.png"
            )
            .is_err()
        );
        assert_eq!(f.app.session.active().ok_or("no doc")?.revision, revision);
        assert_eq!(
            std::fs::read(outside.join("source.png")).map_err(|e| e.to_string())?,
            bytes
        );
        assert!(matches!(
            f.smart(layer)?.source,
            SmartSource::Embedded { .. }
        ));
        Ok(())
    }

    #[test]
    fn synchronous_scripts_and_batch_report_source_errors_before_later_writes() -> Result<(), String>
    {
        let mut f = Fixture::new()?;
        let bytes = f.png([20, 50, 80, 255])?;
        let layer = f.place(&bytes, "source.png", None)?;
        let result=f.app.session.execute("file.scripts.browse",json!({"steps":[["layer.smartObjects.exportContents",{"layer":layer.0,"path":"must-not-write.png"}],["layer.new.layer",{}]]})).map_err(|e|e.to_string())?;
        assert_eq!(result["ok"], false);
        assert!(
            result["results"][0]["error"]
                .as_str()
                .ok_or("no error")?
                .contains("asynchronous")
        );
        assert_eq!(result["results"].as_array().ok_or("not array")?.len(), 1);
        assert!(!f.requests.is_pending());
        let input = f.imported(&bytes, "input.png")?;
        let output = f.root.join("batch");
        std::fs::create_dir(&output).map_err(|e| e.to_string())?;
        let batch=f.app.session.execute("file.automate.batch",json!({"input":[input],"output":output,"format":"png","steps":[["file.revert",{}]]})).map_err(|e|e.to_string())?;
        assert_eq!(batch["files"], json!([]));
        assert!(
            batch["errors"][0]["error"]
                .as_str()
                .ok_or("no batch error")?
                .contains("asynchronous")
        );
        assert_eq!(
            std::fs::read_dir(&output)
                .map_err(|e| e.to_string())?
                .count(),
            0
        );
        Ok(())
    }
}
