//! Original directory commands between authorized folder selection and publication.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use photocraft_engine::Session;
use photocraft_ui_egui::{PhotocraftApp, Services, folder_ui, i18n};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::file_requests::FileRequests;
use crate::folder_requests::{
    absolute_path, inspect, owned_job, plain_directory, release_form, remove_owner, target_valid,
};
use crate::platform::Platform;

#[path = "folder_recovery.rs"]
mod recovery;

#[cfg(test)]
const COMMAND: &str = "file.scripts.imageProcessor";
const MAX_BYTES: u64 = 1_073_741_824;

#[derive(Clone)]
pub struct Processor {
    root: PathBuf,
    inputs: PathBuf,
    platform: Platform,
    files: FileRequests,
    state: Rc<RefCell<State>>,
    prepared: Arc<Mutex<Option<PreparedOutput>>>,
}

struct PreparedOutput {
    command: String,
    handle: String,
    tree: String,
}

#[derive(Default)]
struct State {
    destination: Option<Destination>,
    selections: HashMap<String, Selection>,
    releases: VecDeque<Value>,
    job: Option<Job>,
    recoveries: HashMap<u64, recovery::Record>,
    // Format the launch notice after the UI has resolved the system/preferences language.
    recovery_notice: Option<usize>,
}

struct Destination {
    id: u64,
    target: String,
    form: Option<folder_ui::Request>,
    recovery: Option<u64>,
    cancelled: bool,
    cancel_sent: bool,
}

#[derive(Clone)]
struct Selection {
    id: u64,
    target: String,
    form: folder_ui::Request,
    used: bool,
}

struct Job {
    command: String,
    allocation: u64,
    attempt: Option<u64>,
    target: String,
    handle: String,
    params: Value,
    stage: Option<Stage>,
    result: Option<Value>,
    publishing: bool,
    cancelled: bool,
    cancel_sent: bool,
    progress: String,
    recovery: Option<recovery::Record>,
    engine_job: Option<photocraft_engine::jobs::JobId>,
}

#[derive(Clone)]
struct Stage {
    owner: PathBuf,
    tree: PathBuf,
    journal: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Owner {
    version: u32,
    creation_id: u64,
    target: String,
    owner_root: String,
}

impl Processor {
    pub fn new(files: &Path, platform: &Platform, requests: &FileRequests) -> Result<Self, String> {
        let documents = files.join("PhotoCraft/Documents");
        plain_directory(&documents)?;
        let root = documents.join("FolderPublication");
        if !root.exists() {
            std::fs::create_dir(&root).map_err(|_| "Cannot prepare folder export storage")?;
        }
        plain_directory(&root)?;
        let root = root
            .canonicalize()
            .map_err(|_| "Cannot prepare folder export storage")?;
        let (retained, invalid) = recovery::discover(&root);
        let notice = if invalid > 0 {
            Some(invalid)
        } else if !retained.is_empty() {
            Some(0)
        } else {
            None
        };
        Ok(Self {
            root,
            inputs: documents
                .join("folder-imports")
                .canonicalize()
                .map_err(|_| "Cannot prepare folder input storage")?,
            platform: platform.clone(),
            files: requests.clone(),
            state: Rc::new(RefCell::new(State {
                recoveries: retained
                    .into_iter()
                    .map(|record| (record.allocation, record))
                    .collect(),
                recovery_notice: notice,
                ..Default::default()
            })),
            prepared: Arc::default(),
        })
    }

    pub fn attach(&self, hooks: &mut Services, session: &mut Session) {
        session.automation.defer_batch_publication = true;
        let processor = self.clone();
        hooks.browse_folder_destination = Some(Box::new(move |form| processor.select(form)));
        let processor = self.clone();
        hooks.process_folder = Some(Box::new(move |session, command, params| {
            processor.begin(session, command, params)
        }));
        let processor = self.clone();
        hooks.folder_progress = Some(Box::new(move |cancel| {
            let mut state = processor.state.borrow_mut();
            if let Some(destination) = state
                .destination
                .as_mut()
                .filter(|pending| pending.recovery.is_some())
            {
                destination.cancelled |= cancel;
                return Some(i18n::t("Choosing a folder for retained output…").into());
            }
            let job = state.job.as_mut()?;
            job.cancelled |= cancel;
            Some(i18n::t(&job.progress).to_string())
        }));
        let processor = self.clone();
        hooks.folder_recoveries = Some(Box::new(move || {
            processor
                .state
                .borrow()
                .recoveries
                .values()
                .map(recovery::Record::summary)
                .collect()
        }));
        let processor = self.clone();
        hooks.recover_folder = Some(Box::new(move |id, choose| processor.retry(id, choose)));
        let prepared = self.prepared.clone();
        session.directory_output = Some(Arc::new(move |command, output| {
            if !folder_ui::output_command(command) {
                return Err("This command has no folder output authority".into());
            }
            let mut state = prepared.lock().unwrap_or_else(|error| error.into_inner());
            if state
                .as_ref()
                .is_none_or(|grant| grant.handle != output || grant.command != command)
            {
                return Err("Choose the output folder again before processing".into());
            }
            state
                .take()
                .map(|grant| grant.tree)
                .ok_or_else(|| "Folder output is not prepared".into())
        }));
    }

    fn select(&self, form: folder_ui::Request) -> Result<(), String> {
        let mut state = self.state.borrow_mut();
        if !folder_ui::output_command(&form.command) || form.field != "output" {
            return Err("This form does not select an output folder".into());
        }
        if state.destination.is_some() || state.job.is_some() || self.files.is_pending() {
            return Err("Finish the current file or folder operation first".into());
        }
        if state.selections.len() >= 128 {
            return Err("Close unused output forms before selecting another folder".into());
        }
        let id = crate::platform::next_id();
        let target = format!(
            "form:{}:{}:{}:output:{:?}",
            form.dialog, form.generation, form.command, form.document
        );
        self.platform.acquire_folder(id)?;
        if let Err(error) = self.platform.send_event(
            json!({"kind":"folderDestination","id":id,"target":target,"intent":form.command}),
        ) {
            self.platform.release_folder(id);
            return Err(error);
        }
        state.destination = Some(Destination {
            id,
            target,
            form: Some(form),
            recovery: None,
            cancelled: false,
            cancel_sent: false,
        });
        Ok(())
    }

    fn begin(&self, session: &mut Session, command: &str, params: &Value) -> Result<Value, String> {
        if !folder_ui::output_command(command) || !params.is_object() {
            return Err("Invalid folder processing settings".into());
        }
        let inputs = params["input"]
            .as_array()
            .filter(|files| !files.is_empty() && files.len() <= 500)
            .ok_or("Choose an input folder before processing")?;
        let mut input_bytes = 0_u64;
        for input in inputs {
            let bytes = self.input(
                input
                    .as_str()
                    .ok_or("Folder inputs must be selected files")?,
            )?;
            input_bytes = input_bytes
                .checked_add(bytes)
                .filter(|total| *total <= MAX_BYTES)
                .ok_or("Selected input files exceed the folder limit")?;
        }
        let handle = params["output"]
            .as_str()
            .ok_or("Choose an output folder before processing")?;
        let mut state = self.state.borrow_mut();
        if state.destination.is_some() || state.job.is_some() || self.files.is_pending() {
            return Err("Finish the current file or folder operation first".into());
        }
        let selection = state
            .selections
            .get(handle)
            .ok_or("Choose the output folder again before processing")?;
        if session.active().map(|doc| doc.doc.id.0) != selection.form.document
            || selection.form.command != command
        {
            return Err("The output folder belongs to another input form; choose it again".into());
        }
        let target = selection.target.clone();
        let id = crate::platform::next_id();
        self.platform.acquire_folder(id)?;
        if let Err(error) = self.platform.send_event(json!({"kind":"folderStageAllocate","id":id,"target":target,"intent":command,"destinationHandle":handle})) {
            self.platform.release_folder(id); return Err(error);
        }
        if let Some(selection) = state.selections.get_mut(handle) {
            selection.used = true;
        }
        let progress = i18n::fmt(
            i18n::t("Preparing {command} output…"),
            &[("command", command_label(command))],
        );
        state.job = Some(Job {
            command: command.into(),
            allocation: id,
            attempt: None,
            target,
            handle: handle.into(),
            params: params.clone(),
            stage: None,
            result: None,
            publishing: false,
            cancelled: false,
            cancel_sent: false,
            progress: progress.clone(),
            recovery: None,
            engine_job: None,
        });
        Ok(json!({"pending":true,"status":progress}))
    }

    fn retry(&self, id: u64, choose: bool) -> Result<(), String> {
        let mut state = self.state.borrow_mut();
        if state.job.is_some() || state.destination.is_some() || self.files.is_pending() {
            return Err("Finish the current file or folder operation first".into());
        }
        let record = state
            .recoveries
            .get_mut(&id)
            .ok_or("Retained folder output is unavailable")?;
        record.validate(&self.root)?;
        record.store()?;
        if choose {
            let request = crate::platform::next_id();
            let target = record.target.clone();
            let intent = record.command.clone();
            self.platform.acquire_folder(request)?;
            if let Err(error) = self.platform.send_event(
                json!({"kind":"folderDestination","id":request,"target":target,"intent":intent}),
            ) {
                self.platform.release_folder(request);
                return Err(error);
            }
            state.destination = Some(Destination {
                id: request,
                target,
                form: None,
                recovery: Some(id),
                cancelled: false,
                cancel_sent: false,
            });
        } else {
            if record.handle.is_empty() {
                return Err("Choose another folder to authorize this retained output".into());
            }
            let mut job = record.job();
            self.platform.acquire_folder(job.allocation)?;
            if let Err(error) = self.publish(&mut job) {
                self.platform.release_folder(job.allocation);
                return Err(error);
            }
            state.job = Some(job);
        }
        Ok(())
    }

    fn input(&self, text: &str) -> Result<u64, String> {
        absolute_path(text)?;
        let path = Path::new(text);
        if !path.starts_with(&self.inputs) || path == self.inputs {
            return Err("Choose an input folder before processing".into());
        }
        let mut parent = path.parent();
        while let Some(directory) = parent {
            plain_directory(directory)?;
            if directory == self.inputs {
                break;
            }
            parent = directory.parent();
        }
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|_| "The selected input is no longer readable")?;
        if !metadata.is_file()
            || metadata.len() > MAX_BYTES
            || path
                .canonicalize()
                .map_err(|_| "Cannot verify selected input")?
                != path
        {
            return Err("The selected input is invalid or no longer readable".into());
        }
        Ok(metadata.len())
    }

    pub fn finish_frame(&self, app: &mut PhotocraftApp) {
        let mut state = self.state.borrow_mut();
        for encoded in app.session.take_automation_outputs() {
            if let Some(job) = state
                .job
                .as_mut()
                .filter(|job| job.engine_job == Some(encoded.job))
            {
                let validated = job
                    .stage
                    .as_ref()
                    .ok_or_else(|| "The output stage is unavailable".to_string())
                    .and_then(|stage| outputs(&stage.tree, &encoded.result));
                match validated {
                    Ok(()) => {
                        job.result = Some(encoded.result);
                        job.progress = i18n::fmt(
                            i18n::t("Publishing {files} processed files…"),
                            &[(
                                "files",
                                &job.result.as_ref().map_or(0, |value| count(value, "files")).to_string(),
                            )],
                        );
                    }
                    Err(error) => {
                        job.cancelled = true;
                        job.progress = error.clone();
                        app.ui.status = error;
                        app.ui.status_error = true;
                    }
                }
            }
        }
        if let Some(job) = state
            .job
            .as_mut()
            .filter(|job| job.engine_job.is_some() && job.result.is_none())
            && let Some(info) = app
                .session
                .jobs_with_recent()
                .into_iter()
                .find(|info| Some(info.id) == job.engine_job && info.state != "running")
        {
            job.result=Some(info.result.unwrap_or_else(||json!({"files":[],"errors":[{"error":info.error.unwrap_or_else(||"Batch cancelled".into())}]})));
            job.cancelled = true;
        }
        if let Some(invalid) = state.recovery_notice.take() {
            app.ui.status = if invalid > 0 {
                i18n::fmt(
                    i18n::t("{invalid} retained folder output records could not be verified; their files were preserved"),
                    &[("invalid", &invalid.to_string())],
                )
            } else {
                i18n::t("Retained folder outputs can be recovered from the status bar").into()
            };
            app.ui.status_error = true;
        }
        if let Some(destination) = &mut state.destination {
            let cancel = destination.form.as_ref().is_some_and(|form| {
                app.ui
                    .dialogs
                    .iter()
                    .find(|dialog| dialog.id == form.dialog)
                    .is_some_and(|dialog| {
                        dialog.fields.get(folder_ui::cancel_key("output")) == Some(&json!(true))
                    })
            });
            destination.cancelled |= cancel
                || destination
                    .form
                    .as_ref()
                    .is_some_and(|form| !target_valid(app, form))
                || app.allow_native_close();
            if destination.cancelled && !destination.cancel_sent {
                if self.platform.send_event(json!({"kind":"folderDestinationCancel","id":destination.id,"target":destination.target})).is_ok() {
                    destination.cancel_sent = true;
                    if let Some(form) = &destination.form { release_form(app, form); }
                } else { app.ui.status = i18n::t("Waiting to cancel output selection…").into(); }
            }
        }
        let unused: Vec<_> = state
            .selections
            .iter()
            .filter(|(handle, selection)| {
                let replacing = state.destination.as_ref().is_some_and(|pending| {
                    pending.form.as_ref().is_some_and(|form| {
                        form.dialog == selection.form.dialog
                            && form.original.as_str() == Some(handle.as_str())
                    })
                });
                !selection.used && !replacing && !target_valid(app, &selection.form)
            })
            .map(|(handle, _)| handle.clone())
            .collect();
        for handle in unused {
            release_selection(&mut state, &handle);
        }
        while let Some(event) = state.releases.front() {
            if self.platform.send_event(event.clone()).is_err() {
                break;
            }
            state.releases.pop_front();
        }
        if state.job.as_ref().is_some_and(|job| {
            job.stage.is_some()
                && job.result.is_some()
                && !job.publishing
                && (job.cancelled || app.allow_native_close())
        }) && let Some(mut job) = state.job.take()
        {
            let mut recovery_error = None;
            if job.recovery.is_none()
                && job
                    .result
                    .as_ref()
                    .is_some_and(|value| count(value, "files") > 0)
            {
                match recovery::Record::capture(&job).and_then(|record| {
                    record.store()?;
                    Ok(record)
                }) {
                    Ok(record) => job.recovery = Some(record),
                    Err(error) => {
                        recovery_error = Some(i18n::fmt(
                            i18n::t("Batch cancelled; output retained but its recovery record could not be stored: {error}"),
                            &[("error", &error)],
                        ));
                    }
                }
            }
            if let Some(id) = job.engine_job {
                app.session.complete_automation_publication(
                    id,
                    Err(photocraft_engine::EngineError::Cancelled),
                );
            }
            self.platform.release_folder(job.allocation);
            if let Some(record) = job.recovery {
                state.recoveries.insert(record.allocation, record);
            }
            app.ui.status_error = recovery_error.is_some();
            app.ui.status = recovery_error
                .unwrap_or_else(|| i18n::t("Folder export cancelled; processed files retained").into());
        }
        if let Some(job) = &mut state.job {
            job.cancelled |= app.allow_native_close();
            if job.cancelled
                && let Some(id) = job.engine_job
            {
                app.session.cancel_job(id);
            }
            if job.cancelled && !job.cancel_sent {
                let kind = if job.publishing {
                    "folderPublishCancel"
                } else {
                    "folderStageCancel"
                };
                let id = job.attempt.unwrap_or(job.allocation);
                if self
                    .platform
                    .send_event(json!({"kind":kind,"id":id,"target":job.target}))
                    .is_ok()
                {
                    job.cancel_sent = true;
                    job.progress = i18n::t("Cancelling folder export…").into();
                }
            }
            if job.stage.is_some() && job.result.is_some() && !job.publishing && !job.cancelled {
                let _ = self.publish(job);
            }
        }
    }

    pub fn apply(&self, packet: Value, app: &mut PhotocraftApp) -> Result<(), String> {
        match packet["kind"].as_str() {
            Some("folderDestinationComplete") => self.destination_complete(packet, app),
            Some("folderStageComplete") => self.stage_complete(packet, app),
            Some("folderPublishProgress") => self.progress(packet),
            Some("folderPublishComplete") => self.published(packet, app),
            _ => Err("Invalid folder processing response".into()),
        }
    }

    fn destination_complete(&self, packet: Value, app: &mut PhotocraftApp) -> Result<(), String> {
        let (id, target) = identity(&packet)?;
        let mut state = self.state.borrow_mut();
        if state
            .destination
            .as_ref()
            .is_none_or(|pending| pending.id != id || pending.target != target)
        {
            return Ok(());
        }
        let result = completion(&packet)?;
        let pending = state
            .destination
            .take()
            .ok_or("Output selection is no longer pending")?;
        self.platform.release_folder(id);
        let applicable = !pending.cancelled
            && pending
                .form
                .as_ref()
                .is_none_or(|form| target_valid(app, form));
        if let Some(form) = &pending.form {
            release_form(app, form);
        }
        let handle = packet["destinationHandle"].as_str().unwrap_or_default();
        if let Some(recovery) = pending.recovery {
            if result == "success" {
                validate_handle(handle)?;
                if !applicable {
                    state.releases.push_back(json!({"kind":"folderDestinationRelease","id":id,"target":target,"destinationHandle":handle}));
                } else {
                    let record = state
                        .recoveries
                        .get_mut(&recovery)
                        .ok_or("Retained folder output is unavailable")?;
                    let previous = record.handle.clone();
                    record.handle = handle.into();
                    if let Err(error) = record.store() {
                        record.handle = previous;
                        state.releases.push_back(json!({"kind":"folderDestinationRelease","id":id,"target":target,"destinationHandle":handle}));
                        return Err(error);
                    }
                    if !previous.is_empty() && previous != handle {
                        if state.selections.contains_key(&previous) {
                            release_selection(&mut state, &previous);
                        } else {
                            state.releases.push_back(json!({"kind":"folderDestinationRelease","id":crate::platform::next_id(),"target":target,"destinationHandle":previous}));
                        }
                    }
                    drop(state);
                    return self.retry(recovery, false);
                }
            }
            app.ui.status = if result == "cancel" {
                i18n::t("Recovery folder selection cancelled; output retained").into()
            } else {
                friendly(&packet)
            };
            app.ui.status_error = result == "error";
            return Ok(());
        }
        let form = pending
            .form
            .ok_or("Output selection has no original form")?;
        if result != "success" {
            if let Some(previous) = form
                .original
                .as_str()
                .and_then(|handle| state.selections.get_mut(handle))
            {
                previous.form.generation = form.generation;
            }
            if applicable {
                app.ui.status = if result == "cancel" {
                    i18n::t("Output folder selection cancelled").into()
                } else {
                    friendly(&packet)
                };
                app.ui.status_error = result == "error";
            }
            return Ok(());
        }
        validate_handle(handle)?;
        if !applicable {
            state.releases.push_back(json!({"kind":"folderDestinationRelease","id":id,"target":target,"destinationHandle":handle}));
            return Ok(());
        }
        let previous = form.original.as_str().unwrap_or_default().to_string();
        if previous != handle
            && state
                .selections
                .get(&previous)
                .is_some_and(|selection| !selection.used)
        {
            release_selection(&mut state, &previous);
        }
        let mut form = form;
        form.original = json!(handle);
        let dialog = app
            .ui
            .dialog_mut(form.dialog)
            .ok_or("The output form is closed")?;
        dialog.fields.insert("output".into(), json!(handle));
        dialog
            .fields
            .insert(folder_ui::ready_key("output").into(), json!(true));
        state.selections.insert(
            handle.into(),
            Selection {
                id,
                target: target.into(),
                form,
                used: false,
            },
        );
        app.ui.status = i18n::t("Output folder selected").into();
        app.ui.status_error = false;
        Ok(())
    }

    fn stage_complete(&self, packet: Value, app: &mut PhotocraftApp) -> Result<(), String> {
        let (id, target) = identity(&packet)?;
        let mut state = self.state.borrow_mut();
        if state
            .job
            .as_ref()
            .is_none_or(|job| job.allocation != id || job.target != target || job.stage.is_some())
        {
            return Ok(());
        }
        let result = completion(&packet)?;
        let mut job = state
            .job
            .take()
            .ok_or("Folder export is no longer pending")?;
        if result != "success" || job.cancelled {
            self.platform.release_folder(id);
            if packet["ownerRoot"]
                .as_str()
                .is_some_and(|owner| !owner.is_empty())
            {
                // A cancelled allocation still needs the exact marker and empty tree.
                // A plausible id-prefixed directory alone is not deletion authority.
                remove_owner(&self.stage(&packet, &job)?.owner)?;
            }
            app.ui.status = if job.cancelled || result == "cancel" {
                i18n::fmt(
                    i18n::t("{command} cancelled"),
                    &[("command", command_label(&job.command))],
                )
            } else {
                friendly(&packet)
            };
            app.ui.status_error = result == "error" && !job.cancelled;
            return Ok(());
        }
        let stage = match self.stage(&packet, &job) {
            Ok(stage) => stage,
            Err(error) => {
                self.platform.release_folder(id);
                return Err(error);
            }
        };
        *self
            .prepared
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(PreparedOutput {
            command: job.command.clone(),
            handle: job.handle.clone(),
            tree: stage.tree.to_string_lossy().into_owned(),
        });
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            app.session.execute(&job.command, job.params.clone())
        }))
        .map_err(|_| "Folder processing failed with an internal error".to_string())
        .and_then(|result| result.map_err(|error| error.to_string()));
        self.prepared
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                self.platform.release_folder(id);
                app.ui.status = i18n::t("Folder processing failed; output material was retained").into();
                app.ui.status_error = true;
                return Err(sanitize(&error, &job.params, &stage));
            }
        };
        if let Some(id) = result["job"].as_u64().map(photocraft_engine::jobs::JobId) {
            job.engine_job = Some(id);
            job.stage = Some(stage);
            job.progress = i18n::t("Processing batch steps…").into();
            state.job = Some(job);
            return Ok(());
        }
        if let Err(error) = outputs(&stage.tree, &result) {
            self.platform.release_folder(id);
            show_errors(app, &result, &job.params, &stage);
            return Err(error);
        }
        job.progress = i18n::fmt(
            i18n::t("Publishing {files} processed files…"),
            &[("files", &count(&result, "files").to_string())],
        );
        app.ui.status = job.progress.clone();
        app.ui.status_error = count(&result, "errors") > 0;
        job.stage = Some(stage);
        job.result = Some(result);
        let queued = self.publish(&mut job);
        state.job = Some(job);
        queued
    }

    fn stage(&self, packet: &Value, job: &Job) -> Result<Stage, String> {
        if packet["destinationHandle"] != job.handle {
            return Err("Output stage belongs to another destination".into());
        }
        let owner = owned_job(
            &self.root,
            packet["ownerRoot"]
                .as_str()
                .ok_or("Missing output ownership")?,
            job.allocation,
        )?;
        let tree = owner.join("contents");
        let journal = owner.join("publication.json");
        if packet["stageRoot"].as_str() != tree.to_str()
            || packet["journalPath"].as_str() != journal.to_str()
        {
            return Err("Output stage ownership is invalid".into());
        }
        plain_directory(&tree)?;
        if std::fs::read_dir(&tree)
            .map_err(|_| "Cannot inspect output stage")?
            .next()
            .is_some()
        {
            return Err("The output stage is not empty".into());
        }
        let marker: Owner = serde_json::from_slice(&read_small(&owner.join("owner.json"))?)
            .map_err(|_| "Output ownership marker is invalid")?;
        if marker.version != 1
            || marker.creation_id != job.allocation
            || marker.target != job.target
            || marker.owner_root != owner.to_string_lossy()
        {
            return Err("Output ownership marker does not match this task".into());
        }
        read_small(&journal)?;
        Ok(Stage {
            owner,
            tree,
            journal,
        })
    }

    fn publish(&self, job: &mut Job) -> Result<(), String> {
        if job.recovery.is_none() {
            job.recovery = Some(recovery::Record::capture(job)?);
        }
        job.recovery
            .as_ref()
            .ok_or("Retained folder output is unavailable")?
            .store()?;
        let stage = job.stage.as_ref().ok_or("Processed output is not ready")?;
        let id = *job.attempt.get_or_insert_with(crate::platform::next_id);
        self.platform.send_event(json!({"kind":"folderPublish","id":id,"target":job.target,"destinationHandle":job.handle,
            "ownerRoot":stage.owner,"stageRoot":stage.tree,"limits":{"maxFiles":500,"maxBytes":MAX_BYTES},"policy":"freshSubdirectory"}))?;
        job.publishing = true;
        Ok(())
    }

    fn progress(&self, packet: Value) -> Result<(), String> {
        let (id, target) = identity(&packet)?;
        let mut state = self.state.borrow_mut();
        if let Some(job) = state.job.as_mut().filter(|job| {
            job.publishing && job.attempt == Some(id) && job.target == target && !job.cancelled
        }) {
            let done = packet["processedBytes"]
                .as_u64()
                .filter(|bytes| *bytes <= MAX_BYTES)
                .ok_or("Invalid folder export progress")?;
            let total = packet["totalBytes"]
                .as_u64()
                .filter(|bytes| *bytes <= MAX_BYTES)
                .ok_or("Invalid folder export progress")?;
            if done > total || packet["filesDone"].as_u64().is_none_or(|files| files > 500) {
                return Err("Invalid folder export progress".into());
            }
            job.progress = i18n::fmt(
                i18n::t("Publishing processed files: {done} / {total} MiB"),
                &[
                    ("done", &(done / 1_048_576).to_string()),
                    ("total", &(total / 1_048_576).to_string()),
                ],
            );
        }
        Ok(())
    }

    fn published(&self, packet: Value, app: &mut PhotocraftApp) -> Result<(), String> {
        let (id, target) = identity(&packet)?;
        let mut state = self.state.borrow_mut();
        if state
            .job
            .as_ref()
            .is_none_or(|job| !job.publishing || job.attempt != Some(id) || job.target != target)
        {
            return Ok(());
        }
        let status = completion(&packet)?;
        // Validate the receipt while retaining the running transaction. A malformed
        // callback must not drop its owner or strand an engine continuation.
        let pending = state
            .job
            .as_ref()
            .ok_or("Folder publication is no longer pending")?;
        let mut stage = pending
            .stage
            .clone()
            .ok_or("Folder publication has no owned output")?;
        let encoded_stage = stage.clone();
        let result = pending
            .result
            .as_ref()
            .ok_or("Folder processing result is unavailable")?;
        let preflight_failure = status != "success"
            && packet["ownerRoot"] == ""
            && packet["journalPath"] == ""
            && packet["stageRoot"] == "";
        if !preflight_failure
            && (packet["ownerRoot"].as_str() != stage.owner.to_str()
                || packet["journalPath"].as_str() != stage.journal.to_str())
        {
            return Err("Folder publication ownership is invalid; output retained".into());
        }
        owned_job(
            &self.root,
            stage.owner.to_str().ok_or("Output ownership is invalid")?,
            pending.allocation,
        )?;
        let returned = packet["stageRoot"].as_str().unwrap_or_default();
        if !returned.is_empty() {
            absolute_path(returned)?;
            let returned = Path::new(returned);
            if returned.parent() != Some(stage.owner.as_path())
                || returned
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_none_or(|name| name != "contents" && !publication_name(name))
            {
                return Err(
                    "Folder publication returned an invalid output tree; output retained".into(),
                );
            }
            plain_directory(returned)?;
            stage.tree = returned.to_path_buf();
        }
        let files = count(result, "files");
        let failed = count(result, "errors");
        let partial = packet["partialPossible"]
            .as_bool()
            .ok_or("Folder publication did not report possible partial output")?;
        if packet["retainStage"] != true {
            return Err("Folder publication did not preserve output ownership".into());
        }
        if status == "success"
            && (returned.is_empty() || packet["publishedCount"].as_u64() != Some(files as u64))
        {
            return Err(
                "Folder publication count does not match the processed output; output retained"
                    .into(),
            );
        }
        let mut job = state
            .job
            .take()
            .ok_or("Folder publication is no longer pending")?;
        self.platform.release_folder(job.allocation);
        if let Some(record) = &job.recovery {
            state.recoveries.insert(record.allocation, record.clone());
        }
        let result = job
            .result
            .take()
            .ok_or("Folder processing result is unavailable")?;
        show_errors(app, &result, &job.params, &encoded_stage);
        if status == "success" {
            if returned.is_empty() || packet["publishedCount"].as_u64() != Some(files as u64) {
                return Err(
                    "Folder publication count does not match the processed output; output retained"
                        .into(),
                );
            }
            app.ui.status = i18n::fmt(
                i18n::t("Published {files} processed files; {failed} inputs failed"),
                &[("files", &files.to_string()), ("failed", &failed.to_string())],
            );
            if job.recovery.as_ref().is_some_and(|record| record.legacy) {
                app.ui.status = i18n::fmt(
                    i18n::t("Published {files} retained files; original input failure details unavailable"),
                    &[("files", &files.to_string())],
                );
            }
            app.ui.status_error = failed > 0;
            if let Some(id) = job.engine_job {
                let names = result["files"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter_map(|path| {
                        Path::new(path)
                            .file_name()
                            .and_then(|name| name.to_str())
                            .map(str::to_string)
                    })
                    .collect::<Vec<_>>();
                app.session.complete_automation_publication(id,Ok(json!({"files":names,"errors":result["errors"],"published":true,"status":app.ui.status})));
            }
            state.recoveries.remove(&job.allocation);
            if !state.selections.contains_key(&job.handle) {
                state.releases.push_back(json!({"kind":"folderDestinationRelease","id":crate::platform::next_id(),"target":job.target,"destinationHandle":job.handle}));
            }
            if remove_owner(&stage.owner).is_err() {
                app.ui
                    .status
                    .push_str(i18n::t("; internal output was retained because cleanup failed"));
                app.ui.status_error = true;
            }
        } else {
            if let Some(id) = job.engine_job {
                let error = if status == "cancel" {
                    photocraft_engine::EngineError::Cancelled
                } else {
                    photocraft_engine::EngineError::Other(friendly(&packet))
                };
                app.session.complete_automation_publication(id, Err(error));
            }
            if let Some(mut record) = job.recovery {
                record.tree_name = stage
                    .tree
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or("Retained output name is invalid")?
                    .into();
                record.partial |= partial;
                record.store()?;
                state.recoveries.insert(record.allocation, record);
            }
            let message = if status == "cancel" {
                i18n::t("Folder export cancelled").to_string()
            } else {
                friendly(&packet)
            };
            let suffix = if partial { i18n::t("; partial external output may remain") } else { "" };
            app.ui.status = i18n::fmt(
                i18n::t("{status}; {files} processed files retained; {failed} inputs failed{partial}"),
                &[
                    ("status", &message),
                    ("files", &files.to_string()),
                    ("failed", &failed.to_string()),
                    ("partial", suffix),
                ],
            );
            app.ui.status_error = status == "error" || failed > 0;
        }
        Ok(())
    }
}

fn release_selection(state: &mut State, handle: &str) {
    if let Some(selection) = state.selections.remove(handle) {
        state.releases.push_back(json!({"kind":"folderDestinationRelease","id":selection.id,"target":selection.target,"destinationHandle":handle}));
    }
}

fn identity(packet: &Value) -> Result<(u64, &str), String> {
    let id = packet["id"]
        .as_u64()
        .filter(|id| *id <= 9_007_199_254_740_991)
        .ok_or("Invalid folder request ID")?;
    let target = packet["target"]
        .as_str()
        .filter(|target| !target.is_empty() && target.len() <= 512 && !target.contains('\0'))
        .ok_or("Invalid folder target")?;
    Ok((id, target))
}

fn completion(packet: &Value) -> Result<&str, String> {
    packet["result"]
        .as_str()
        .filter(|result| matches!(*result, "success" | "cancel" | "error"))
        .ok_or_else(|| "Invalid folder completion result".into())
}

fn validate_handle(handle: &str) -> Result<(), String> {
    let bytes = handle.as_bytes();
    if bytes.len() != 36
        || bytes.get(14) != Some(&b'4')
        || !matches!(bytes.get(19), Some(b'8' | b'9' | b'a' | b'b'))
        || bytes.iter().enumerate().any(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *byte != b'-'
            } else {
                !byte.is_ascii_hexdigit()
            }
        })
    {
        return Err("Output folder authorization is invalid".into());
    }
    Ok(())
}

fn publication_name(name: &str) -> bool {
    name.strip_prefix("PhotoCraft-Export-")
        .is_some_and(|suffix| validate_handle(suffix).is_ok())
}

fn read_small(path: &Path) -> Result<Vec<u8>, String> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| "Cannot read folder output metadata")?;
    if !metadata.is_file() || metadata.len() > 65_536 {
        return Err("Folder output metadata is invalid or too large".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| "Cannot read folder output metadata")?
        .take(65_537)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read folder output metadata")?;
    if bytes.len() > 65_536 {
        return Err("Folder output metadata is too large".into());
    }
    Ok(bytes)
}

fn command_label(command: &str) -> &'static str {
    i18n::t(match command {
        "file.automate.batch" => "Batch",
        "file.automate.lensCorrection" => "Lens Correction",
        _ => "Image Processor",
    })
}

fn count(result: &Value, field: &str) -> usize {
    result[field].as_array().map_or(0, Vec::len)
}

fn outputs(root: &Path, result: &Value) -> Result<(), String> {
    let files = result["files"]
        .as_array()
        .ok_or("Folder processing did not return its output files")?;
    if files.is_empty() {
        return Err("No inputs were processed; failed input details are available".into());
    }
    let actual = inspect(root)?;
    let names: Vec<_> = actual
        .iter()
        .filter(|(_, (kind, _))| kind == "file")
        .map(|(name, _)| name.as_str())
        .collect();
    if names.len() != files.len()
        || files.iter().any(|file| {
            file.as_str()
                .and_then(|path| Path::new(path).strip_prefix(root).ok())
                .and_then(Path::to_str)
                .is_none_or(|name| !names.contains(&name))
        })
    {
        return Err(
            "The processed output tree does not match the original engine result; output retained"
                .into(),
        );
    }
    Ok(())
}

fn sanitize(message: &str, params: &Value, stage: &Stage) -> String {
    let mut message = message.to_string();
    if let Some(inputs) = params["input"].as_array() {
        for input in inputs.iter().filter_map(Value::as_str) {
            let name = Path::new(input)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("input image");
            message = message.replace(input, name);
        }
    }
    message = message.replace(&format!("{}/", stage.tree.to_string_lossy()), "");
    message = message.replace(stage.tree.to_string_lossy().as_ref(), "output folder");
    if message.contains(['/', '\\']) || message.len() > 512 {
        i18n::t("The image could not be processed; check its format and available storage").into()
    } else {
        i18n::t(&message).to_string()
    }
}

fn show_errors(app: &mut PhotocraftApp, result: &Value, params: &Value, stage: &Stage) {
    let Some(errors) = result["errors"]
        .as_array()
        .filter(|errors| !errors.is_empty())
    else {
        return;
    };
    let mut lines = vec![i18n::fmt(
        i18n::t("{count} inputs could not be processed:"),
        &[("count", &errors.len().to_string())],
    )];
    for error in errors.iter().take(12) {
        let name = error["file"]
            .as_str()
            .and_then(|path| Path::new(path).file_name())
            .and_then(|name| name.to_str())
            .unwrap_or_else(|| i18n::t("Input image"));
        let error = sanitize(
            error["error"].as_str().unwrap_or_else(|| i18n::t("Processing failed")),
            params,
            stage,
        );
        lines.push(i18n::fmt(
            i18n::t("{name}: {error}"),
            &[("name", name), ("error", &error)],
        ));
    }
    if errors.len() > 12 {
        lines.push(i18n::fmt(
            i18n::t("And {count} more failed inputs"),
            &[("count", &(errors.len() - 12).to_string())],
        ));
    }
    let mut fields = serde_json::Map::new();
    fields.insert("message".into(), json!(lines.join("\n")));
    app.ui
        .open_dialog(photocraft_ui_egui::state::DialogKind::Error, fields);
}

fn friendly(packet: &Value) -> String {
    let message = packet["error"]
        .as_str()
        .filter(|error| !error.is_empty() && error.len() <= 256 && !error.contains(['/', '\\']))
        .unwrap_or_else(|| i18n::t("Folder export failed; check authorization and available storage"));
    i18n::t(message).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::folder_requests::tests::Fixture as Inputs;
    use eframe::App;
    use photocraft_ui_egui::dialogs;

    const HANDLE: &str = "00000000-0000-4000-8000-000000000001";
    const PUBLICATION: &str = "PhotoCraft-Export-00000000-0000-4000-8000-000000000002";

    struct Fixture {
        inputs: Inputs,
        processor: Processor,
        dialog: u64,
    }

    impl Fixture {
        fn new(extra: &str) -> Result<Self, String> {
            Self::for_command(COMMAND, extra)
        }

        fn for_command(command: &str, extra: &str) -> Result<Self, String> {
            let mut inputs = Inputs::new()?;
            let files = FileRequests::new(&inputs.root)?;
            let processor = Processor::new(&inputs.root, &inputs.platform, &files)?;
            processor.attach(&mut inputs.app.services, &mut inputs.app.session);
            if command == "file.automate.batch" {
                inputs
                    .app
                    .ui
                    .actions
                    .list
                    .push(photocraft_ui_egui::actions::Action {
                        name: "Captured invert".into(),
                        steps: vec![
                            ("image.adjustments.invert".into(), json!({})),
                            ("layer.new.layer".into(), json!({"name":"Captured layer"})),
                        ],
                        expanded: true,
                    });
                inputs.app.ui.actions.selected = Some(0);
            }
            let (dialog, request) = inputs.browse(command)?;
            let response = inputs.copied(&request)?;
            let root = Path::new(
                response["resolvedRoot"]
                    .as_str()
                    .ok_or("input root missing")?,
            );
            let manifest = Path::new(
                response["manifestPath"]
                    .as_str()
                    .ok_or("manifest missing")?,
            );
            let mut contents: Value = serde_json::from_slice(
                &std::fs::read(manifest).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
            let mut source = Session::new();
            source
                .execute(
                    "file.new",
                    json!({"width":2,"height":4,"background":"#00ff00"}),
                )
                .map_err(|error| error.to_string())?;
            source
                .execute("layer.new.layer", json!({"name":"Second layer"}))
                .map_err(|error| error.to_string())?;
            let bytes = photocraft_io::export(
                &source.active().ok_or("PSD source missing")?.doc,
                "layers.psd",
                &Default::default(),
            )
            .map_err(|error| error.to_string())?
            .bytes;
            let mut additions = vec![("layers.psd", bytes.clone())];
            if extra == "failures" {
                additions.push(("A.psd", bytes));
                additions.push(("broken.png", b"invalid PNG".to_vec()));
            } else if extra == "lens" {
                let pixels = [180, 120, 60, 255].repeat(16 * 16);
                additions.push((
                    "photo.png",
                    inputs
                        .app
                        .services
                        .encode_png
                        .as_ref()
                        .ok_or("encoder missing")?(16, 16, &pixels)?,
                ));
            }
            for (name, bytes) in additions {
                std::fs::write(root.join(name), &bytes).map_err(|error| error.to_string())?;
                contents["entries"]
                    .as_array_mut()
                    .ok_or("entries missing")?
                    .push(json!({"relativePath":name,"kind":"file","bytes":bytes.len()}));
                contents["fileCount"] =
                    json!(contents["fileCount"].as_u64().ok_or("count missing")? + 1);
                contents["totalBytes"] = json!(
                    contents["totalBytes"].as_u64().ok_or("bytes missing")? + bytes.len() as u64
                );
            }
            std::fs::write(manifest, contents.to_string()).map_err(|error| error.to_string())?;
            inputs.folders.apply(response, &mut inputs.app)?;
            assert!(!folder_ui::ready(
                &inputs.app,
                &inputs
                    .app
                    .ui
                    .dialogs
                    .iter()
                    .find(|form| form.id == dialog)
                    .ok_or("form missing")?
                    .fields
            ));
            let fixture = Self {
                inputs,
                processor,
                dialog,
            };
            Ok(fixture)
        }

        fn event(&self, kind: &str) -> Result<Value, String> {
            let output: Value = serde_json::from_str(&self.inputs.platform.take_json())
                .map_err(|error| error.to_string())?;
            output["events"]
                .as_array()
                .and_then(|events| events.iter().find(|event| event["kind"] == kind))
                .cloned()
                .ok_or_else(|| format!("missing {kind} event: {output}"))
        }

        fn choose(&mut self) -> Result<Value, String> {
            folder_ui::request_destination(&mut self.inputs.app, self.dialog)?;
            let event = self.event("folderDestination")?;
            self.processor.apply(json!({"kind":"folderDestinationComplete","id":event["id"],"target":event["target"],"result":"success","destinationHandle":HANDLE,"error":""}), &mut self.inputs.app)?;
            Ok(event)
        }

        fn visible_form(&mut self) -> String {
            fn text(shape: &egui::epaint::Shape, result: &mut String) {
                match shape {
                    egui::epaint::Shape::Text(text) => {
                        result.push_str(text.galley.text());
                        result.push('\n');
                    }
                    egui::epaint::Shape::Vec(shapes) => {
                        for shape in shapes {
                            text(shape, result);
                        }
                    }
                    _ => {}
                }
            }
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 800.0),
                )),
                ..Default::default()
            };
            let mut frame = eframe::Frame::_new_kittest();
            let mut visible = String::new();
            for _ in 0..4 {
                let mut logic_done = false;
                let output = self.inputs.ctx.run_ui(input.clone(), |ui| {
                    if !logic_done {
                        self.inputs.app.logic(ui.ctx(), &mut frame);
                        logic_done = true;
                    }
                    self.inputs.app.ui(ui, &mut frame);
                });
                visible.clear();
                for clipped in &output.shapes {
                    text(&clipped.shape, &mut visible);
                }
                output.drop_without_applying_deltas();
            }
            visible
        }

        fn start(&mut self, format: &str) -> Result<Value, String> {
            self.start_with(format, json!({"width":2,"height":2}))
        }

        fn start_with(&mut self, format: &str, params: Value) -> Result<Value, String> {
            let selected = self.choose()?;
            let command = self
                .inputs
                .app
                .ui
                .dialog_mut(self.dialog)
                .ok_or("form missing")?
                .fields["__command"]
                .clone();
            assert_eq!(selected["intent"], command);

            let fields = &mut self
                .inputs
                .app
                .ui
                .dialog_mut(self.dialog)
                .ok_or("form missing")?
                .fields;
            fields.insert("format".into(), json!(format));
            for (key, value) in params.as_object().ok_or("params missing")? {
                fields.insert(key.clone(), value.clone());
            }
            let journal = self.inputs.app.session.journal.len();
            let result = dialogs::confirm(&mut self.inputs.app, self.dialog)?;
            assert_eq!(result["pending"], true);
            assert_eq!(
                self.inputs.app.session.journal.len(),
                journal,
                "allocation must not execute a command"
            );
            self.event("folderStageAllocate")
        }

        fn stage(&self, request: &Value) -> Result<Value, String> {
            let id = request["id"].as_u64().ok_or("allocation id missing")?;
            let owner = self.processor.root.join(format!("{id}-ABC123"));
            let tree = owner.join("contents");
            std::fs::create_dir_all(&tree).map_err(|error| error.to_string())?;
            std::fs::write(
                owner.join("owner.json"),
                json!({"version":1,"creationId":id,"target":request["target"],"ownerRoot":owner})
                    .to_string(),
            )
            .map_err(|error| error.to_string())?;
            let journal = owner.join("publication.json");
            std::fs::write(&journal, json!({"version":1,"id":id,"target":request["target"],"phase":"prepared","ownerRoot":owner,"stageRoot":tree,"publicationName":"","destinationUri":"","partialPossible":false,"fileCount":0,"totalBytes":0}).to_string()).map_err(|error| error.to_string())?;
            Ok(
                json!({"kind":"folderStageComplete","id":id,"target":request["target"],"result":"success","destinationHandle":HANDLE,"ownerRoot":owner,"stageRoot":tree,"journalPath":journal,"error":""}),
            )
        }

        fn encoded(&mut self, request: &Value) -> Result<Value, String> {
            let response = self.stage(request)?;
            self.processor.apply(response, &mut self.inputs.app)?;
            let publish = self.event("folderPublish")?;
            assert_eq!(publish["target"], request["target"]);
            assert_eq!(publish["destinationHandle"], HANDLE);
            assert_ne!(publish["id"], request["id"]);
            assert!(self.inputs.app.ui.status.starts_with("Publishing"));
            Ok(publish)
        }

        fn receipt(&self, publish: &Value, result: &str, partial: bool) -> Result<Value, String> {
            let owner = Path::new(publish["ownerRoot"].as_str().ok_or("owner missing")?);
            let old = Path::new(publish["stageRoot"].as_str().ok_or("tree missing")?);
            let name = if old.file_name().and_then(|name| name.to_str()) == Some(PUBLICATION) {
                "PhotoCraft-Export-00000000-0000-4000-8000-000000000003"
            } else {
                PUBLICATION
            };
            let tree = owner.join(name);
            std::fs::rename(old, &tree).map_err(|error| error.to_string())?;
            let count = self
                .processor
                .state
                .borrow()
                .job
                .as_ref()
                .and_then(|job| job.result.as_ref())
                .map(|result| count(result, "files"))
                .ok_or("result missing")?;
            let entries: Vec<Value> = inspect(&tree)?
                .into_iter()
                .map(|(path, (kind, bytes))| json!({"relativePath":path,"kind":kind,"bytes":bytes}))
                .collect();
            let total: u64 = inspect(&tree)?.values().map(|(_, bytes)| bytes).sum();
            std::fs::write(
                owner.join("manifest.json"),
                json!({"version":1,"entries":entries,"fileCount":count,"totalBytes":total})
                    .to_string(),
            )
            .map_err(|error| error.to_string())?;
            std::fs::write(owner.join("publication.json"),json!({"version":1,"id":publish["id"],"target":publish["target"],"phase":if result=="success" {"complete"} else if result=="cancel" {"cancelled"} else {"failed"},"ownerRoot":owner,"stageRoot":old,"publicationName":name,"destinationUri":"content://external-directory","partialPossible":partial,"fileCount":count,"totalBytes":total}).to_string()).map_err(|error| error.to_string())?;
            Ok(
                json!({"kind":"folderPublishComplete","id":publish["id"],"target":publish["target"],"result":result,"ownerRoot":owner,"stageRoot":tree,"journalPath":owner.join("publication.json"),"publicationName":name,"publishedCount":if result=="success" {count} else {0},"partialPossible":partial,"retainStage":true,"error":if result=="error" {"permission denied"} else {""}}),
            )
        }
    }

    fn document_states(
        session: &Session,
    ) -> Vec<(Arc<photocraft_doc::Document>, Option<String>, u64, u64)> {
        session
            .documents()
            .iter()
            .map(|doc| {
                (
                    doc.doc.clone(),
                    doc.path.clone(),
                    doc.revision,
                    doc.saved_revision,
                )
            })
            .collect()
    }

    fn decode_output(result: &Value, name: &str) -> Result<photocraft_doc::Document, String> {
        let path = result["files"]
            .as_array()
            .ok_or("files missing")?
            .iter()
            .filter_map(Value::as_str)
            .find(|path| Path::new(path).file_name().and_then(|name| name.to_str()) == Some(name))
            .ok_or("output missing")?;
        let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
        Ok(photocraft_io::import(name, &bytes)
            .map_err(|error| error.to_string())?
            .document)
    }

    #[test]
    fn batch_source_continuation_holds_real_scratch_and_engine_job_until_publication_receipt()
    -> Result<(), String> {
        for completion in ["success", "error"] {
            let mut f = Fixture::for_command("file.automate.batch", "")?;
            f.inputs.app.session.automation.enabled = true;
            f.processor
                .files
                .attach_sources(&mut f.inputs.app.session, &f.inputs.ctx);
            let original = document_states(&f.inputs.app.session);
            f.inputs
                .app
                .ui
                .dialog_mut(f.dialog)
                .ok_or("form missing")?
                .fields
                .insert(
                    "steps".into(),
                    json!([["file.revert", {}], ["image.adjustments.invert", {}]]),
                );
            let allocation = f.start_with("same", json!({}))?;
            let stage = f.stage(&allocation)?;
            f.processor.apply(stage, &mut f.inputs.app)?;
            let engine = f
                .processor
                .state
                .borrow()
                .job
                .as_ref()
                .and_then(|job| job.engine_job)
                .ok_or("continuation job missing")?;
            let mut reads = 0;
            for _ in 0..200 {
                f.inputs.app.session.poll_jobs();
                f.processor.files.finish_frame(&mut f.inputs.app);
                f.processor.finish_frame(&mut f.inputs.app);
                let text = f.processor.files.take_json();
                if !text.is_empty() {
                    let request: Value =
                        serde_json::from_str(&text).map_err(|error| error.to_string())?;
                    assert_eq!(request["intent"], "file.revert");
                    let old = request["previousPath"]
                        .as_str()
                        .ok_or("source path missing")?;
                    let bytes = std::fs::read(old).map_err(|error| error.to_string())?;
                    let name = Path::new(old)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .ok_or("source name missing")?;
                    let dir = f
                        .inputs
                        .root
                        .join("PhotoCraft/Documents/imports")
                        .join(format!("{}-test", request["id"]));
                    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
                    let fresh = dir.join(name);
                    std::fs::write(&fresh, bytes).map_err(|error| error.to_string())?;
                    f.processor.files.complete_with_path(
                        &mut f.inputs.app,
                        &f.inputs.ctx,
                        request["id"].as_u64().ok_or("id missing")?,
                        "success",
                        fresh.to_str().ok_or("path invalid")?,
                        name,
                        "",
                    )?;
                    reads += 1;
                }
                if f.processor
                    .state
                    .borrow()
                    .job
                    .as_ref()
                    .is_some_and(|job| job.publishing)
                {
                    break;
                }
            }
            assert_eq!(reads, 4);
            assert!(f.inputs.app.session.is_external_job(engine));
            assert!(
                !f.inputs
                    .app
                    .session
                    .journal
                    .iter()
                    .any(|(id, _)| id == "file.automate.batch"),
                "batch journal cannot imply publication before the external receipt"
            );
            let publish = f.event("folderPublish")?;
            let receipt = f.receipt(&publish, completion, false)?;
            let mut malformed = receipt.clone();
            malformed["publishedCount"] = json!(999);
            malformed["result"] = json!("success");
            assert!(f.processor.apply(malformed, &mut f.inputs.app).is_err());
            assert!(f.inputs.app.session.is_external_job(engine));
            assert!(
                f.processor.state.borrow().job.is_some(),
                "bad receipt keeps the pending owner"
            );
            f.processor.apply(receipt, &mut f.inputs.app)?;
            let final_job = f
                .inputs
                .app
                .session
                .jobs_with_recent()
                .into_iter()
                .find(|job| job.id == engine)
                .ok_or("final engine job missing")?;
            assert_eq!(
                final_job.state,
                if completion == "success" {
                    "done"
                } else {
                    "failed"
                }
            );
            assert_eq!(document_states(&f.inputs.app.session), original);
            assert_eq!(
                f.inputs
                    .app
                    .session
                    .journal
                    .iter()
                    .filter(|(id, _)| id == "file.automate.batch")
                    .count(),
                usize::from(completion == "success")
            );
            if completion == "error" {
                assert!(!f.processor.state.borrow().recoveries.is_empty());
            }
        }
        Ok(())
    }

    #[test]
    fn original_batch_keeps_captured_action_steps_same_format_layers_and_scratch_print_ownership()
    -> Result<(), String> {
        let command = "file.automate.batch";
        let mut fixture = Fixture::for_command(command, "")?;
        crate::printing::attach(
            &mut fixture.inputs.app.services,
            &mut fixture.inputs.app.session,
            &fixture.inputs.root,
            &FileRequests::new(&fixture.inputs.root)?,
            &fixture.inputs.platform,
        )?;
        let original = document_states(&fixture.inputs.app.session);
        let visible = fixture.visible_form();
        assert_eq!(visible.matches("Browse").count(), 2);
        let fields = &mut fixture
            .inputs
            .app
            .ui
            .dialog_mut(fixture.dialog)
            .ok_or("form missing")?
            .fields;
        fields["steps"]
            .as_array_mut()
            .ok_or("steps missing")?
            .push(json!(["file.printOneCopy", {}]));
        let request = fixture.start_with("same", json!({}))?;
        assert_eq!(request["intent"], command);
        let params = fixture
            .processor
            .state
            .borrow()
            .job
            .as_ref()
            .ok_or("job missing")?
            .params
            .clone();
        // The original dialog has captured the selected action; edits to the Actions panel
        // while the system authorizes the output do not change that immutable execution.
        fixture.inputs.app.ui.actions.list[0].steps =
            vec![("image.adjustments.desaturate".into(), json!({}))];
        fixture.inputs.app.run(
            "file.new",
            json!({"width":1,"height":1,"name":"Different active doc"}),
        )?;
        let expected_working = document_states(&fixture.inputs.app.session);
        let response = fixture.stage(&request)?;
        fixture.processor.apply(response, &mut fixture.inputs.app)?;
        let output: Value = serde_json::from_str(&fixture.inputs.platform.take_json())
            .map_err(|error| error.to_string())?;
        let events = output["events"].as_array().ok_or("events missing")?;
        let publish = events
            .iter()
            .find(|event| event["kind"] == "folderPublish")
            .ok_or("publication missing")?
            .clone();
        let print_events: Vec<_> = events
            .iter()
            .filter(|event| event["kind"] == "printRequest")
            .collect();
        assert_eq!(print_events.len(), 4);
        let mut print_documents = std::collections::HashSet::new();
        for event in print_events {
            let id = event["documentId"].as_str().ok_or("print owner missing")?;
            assert!(print_documents.insert(id));
            assert!(
                !expected_working
                    .iter()
                    .any(|(document, _, _, _)| document.id.0.to_string() == id)
            );
            assert!(
                std::fs::read(event["pdfPath"].as_str().ok_or("PDF missing")?)
                    .map_err(|error| error.to_string())?
                    .starts_with(b"%PDF-")
            );
        }
        assert_eq!(
            output["printJobs"]
                .as_array()
                .ok_or("pending print jobs missing")?
                .len(),
            4
        );
        let state = fixture.processor.state.borrow();
        let job = state.job.as_ref().ok_or("job missing")?;
        let result = job.result.as_ref().ok_or("result missing")?;
        assert_eq!(count(result, "files"), 4);
        assert_eq!(count(result, "errors"), 0);
        let blue = decode_output(result, "a.png")?;
        let pixel = blue.layers[0]
            .surface()
            .ok_or("blue raster missing")?
            .rgba(0, 0);
        assert!(
            pixel[0] > 0.99 && pixel[1] > 0.99 && pixel[2] < 0.01,
            "captured invert must turn blue into yellow: {pixel:?}"
        );
        let layered = decode_output(result, "layers.psd")?;
        assert_eq!(layered.layers.len(), 3);
        assert_eq!(layered.layers[2].name, "Captured layer");
        drop(state);
        assert_eq!(
            fixture.inputs.app.session.journal.last(),
            Some(&(command.into(), params))
        );
        assert_eq!(
            document_states(&fixture.inputs.app.session),
            expected_working
        );
        assert_eq!(&expected_working[..original.len()], original.as_slice());
        let receipt = fixture.receipt(&publish, "success", false)?;
        fixture.processor.apply(receipt, &mut fixture.inputs.app)?;
        assert_eq!(
            fixture.inputs.app.ui.status,
            "Published 4 processed files; 0 inputs failed"
        );
        assert_eq!(
            document_states(&fixture.inputs.app.session),
            expected_working
        );
        let output: Value = serde_json::from_str(&fixture.inputs.platform.take_json())
            .map_err(|error| error.to_string())?;
        assert_eq!(
            output["printJobs"]
                .as_array()
                .ok_or("prints missing")?
                .len(),
            4,
            "folder publication must not complete system print jobs"
        );
        Ok(())
    }

    #[test]
    fn original_lens_batch_flattens_layered_inputs_and_applies_captured_correction_before_publication()
    -> Result<(), String> {
        let command = "file.automate.lensCorrection";
        let mut fixture = Fixture::for_command(command, "lens")?;
        let original = document_states(&fixture.inputs.app.session);
        let visible = fixture.visible_form();
        assert_eq!(visible.matches("Browse").count(), 2);
        assert!(!visible.contains("FolderPublication"));
        let request = fixture.start_with("same",json!({"profile":"none","autoScale":false,"distortion":-40,"vignetteAmount":-50,"edge":"transparency"}))?;
        let params = fixture
            .processor
            .state
            .borrow()
            .job
            .as_ref()
            .ok_or("job missing")?
            .params
            .clone();
        let publish = fixture.encoded(&request)?;
        let state = fixture.processor.state.borrow();
        let result = state
            .job
            .as_ref()
            .and_then(|job| job.result.as_ref())
            .ok_or("result missing")?;
        assert_eq!(count(result, "files"), 5);
        assert_eq!(count(result, "errors"), 0);
        let layers = decode_output(result, "layers.psd")?;
        assert_eq!(
            layers.layers.len(),
            1,
            "original lens batch flattens its scratch document"
        );
        assert_eq!((layers.size.width, layers.size.height), (2, 4));
        let photo = decode_output(result, "photo.png")?;
        assert_eq!((photo.size.width, photo.size.height), (16, 16));
        let surface = photo.layers[0].surface().ok_or("photo raster missing")?;
        assert!(
            surface.rgba(0, 0)[3] < 0.5,
            "captured distortion clears the outer corner with transparency edge"
        );
        assert!(
            surface.rgba(8, 8)[3] > 0.99,
            "correction retains the center"
        );
        assert!(
            surface.rgba(1, 1)[0] < surface.rgba(8, 8)[0] * 0.75,
            "captured vignette darkens opaque corner pixels"
        );
        drop(state);
        assert_eq!(
            fixture.inputs.app.session.journal.last(),
            Some(&(command.into(), params))
        );
        assert_eq!(document_states(&fixture.inputs.app.session), original);
        assert!(fixture.inputs.app.ui.status.starts_with("Publishing"));
        let receipt = fixture.receipt(&publish, "success", false)?;
        fixture.processor.apply(receipt, &mut fixture.inputs.app)?;
        assert_eq!(
            fixture.inputs.app.ui.status,
            "Published 5 processed files; 0 inputs failed"
        );
        assert_eq!(document_states(&fixture.inputs.app.session), original);
        Ok(())
    }

    #[test]
    fn batch_and_lens_keep_original_decode_claim_errors_and_retained_material_on_failed_publication()
    -> Result<(), String> {
        for command in ["file.automate.batch", "file.automate.lensCorrection"] {
            let mut fixture = Fixture::for_command(command, "failures")?;
            let original = document_states(&fixture.inputs.app.session);
            let request = fixture.start_with("png", json!({"profile":"none"}))?;
            let publish = fixture.encoded(&request)?;
            let state = fixture.processor.state.borrow();
            let result = state
                .job
                .as_ref()
                .and_then(|job| job.result.as_ref())
                .ok_or("result missing")?;
            assert_eq!(count(result, "files"), 4);
            assert_eq!(count(result, "errors"), 2);
            let errors = result["errors"].to_string();
            assert!(errors.contains("same output name"));
            assert!(errors.contains("broken.png"));
            drop(state);
            let receipt = fixture.receipt(&publish, "error", true)?;
            fixture
                .processor
                .apply(receipt.clone(), &mut fixture.inputs.app)?;
            assert!(fixture.inputs.app.ui.status_error);
            assert!(
                fixture
                    .inputs
                    .app
                    .ui
                    .status
                    .contains("4 processed files retained; 2 inputs failed")
            );
            assert!(
                Path::new(
                    receipt["stageRoot"]
                        .as_str()
                        .ok_or("retained outputs missing")?
                )
                .exists()
            );
            assert_eq!(document_states(&fixture.inputs.app.session), original);
            assert!(fixture.inputs.app.ui.dialogs.iter().any(|dialog| {
                dialog.fields["message"].as_str().is_some_and(|message| {
                    message.contains("broken.png") && !message.contains("folder-imports")
                })
            }));
        }
        Ok(())
    }

    #[test]
    fn batch_and_lens_cancel_or_mismatched_grants_never_execute_on_another_command()
    -> Result<(), String> {
        for command in ["file.automate.batch", "file.automate.lensCorrection"] {
            let mut fixture = Fixture::for_command(command, "")?;
            fixture.choose()?;
            let fields = fixture
                .inputs
                .app
                .ui
                .dialog_mut(fixture.dialog)
                .ok_or("form missing")?
                .fields
                .clone();
            let params = photocraft_ui_egui::filter_dialog::params_of(&fields);
            assert!(
                fixture.inputs.app.run(COMMAND, params).is_err(),
                "an output handle cannot authorize a different form command"
            );
            fixture.inputs.app.ui.status = "Folder cancelled".into();
            let result = dialogs::confirm(&mut fixture.inputs.app, fixture.dialog)?;
            assert_eq!(result["pending"], true);
            assert!(!fixture.inputs.app.ui.status.contains("Folder cancelled"));
            let request = fixture.event("folderStageAllocate")?;
            let original = document_states(&fixture.inputs.app.session);
            let journal = fixture.inputs.app.session.journal.clone();
            fixture
                .inputs
                .app
                .services
                .folder_progress
                .as_mut()
                .ok_or("progress missing")?(true);
            fixture.processor.finish_frame(&mut fixture.inputs.app);
            let cancel = fixture.event("folderStageCancel")?;
            assert_eq!(cancel["target"], request["target"]);
            let response = fixture.stage(&request)?;
            fixture
                .processor
                .apply(response.clone(), &mut fixture.inputs.app)?;
            assert!(!Path::new(response["ownerRoot"].as_str().ok_or("stage missing")?).exists());
            assert_eq!(fixture.inputs.app.session.journal, journal);
            assert_eq!(document_states(&fixture.inputs.app.session), original);
            assert!(fixture.processor.state.borrow().job.is_none());
            let mut forged = Fixture::for_command(command, "")?;
            let request = forged.start_with("png", json!({}))?;
            let mut response = forged.stage(&request)?;
            response["result"] = json!("cancel");
            let owner =
                Path::new(response["ownerRoot"].as_str().ok_or("owner missing")?).to_path_buf();
            let mut marker: Value = serde_json::from_slice(
                &std::fs::read(owner.join("owner.json")).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
            marker["target"] = json!("another target");
            std::fs::write(owner.join("owner.json"), marker.to_string())
                .map_err(|error| error.to_string())?;
            assert!(
                forged
                    .processor
                    .apply(response, &mut forged.inputs.app)
                    .is_err()
            );
            assert!(
                owner.exists(),
                "cancel receipts cannot delete a forged allocation owner"
            );
        }
        Ok(())
    }

    #[test]
    fn original_image_processor_encodes_four_formats_and_journals_logical_destination()
    -> Result<(), String> {
        for format in ["png", "jpg", "psd", "tiff"] {
            let mut fixture = Fixture::new("")?;
            let original = document_states(&fixture.inputs.app.session);
            let request = fixture.start(format)?;
            let publish = fixture.encoded(&request)?;
            assert_eq!(document_states(&fixture.inputs.app.session), original);
            let state = fixture.processor.state.borrow();
            let job = state.job.as_ref().ok_or("job missing")?;
            let result = job.result.as_ref().ok_or("result missing")?;
            assert_eq!(count(result, "files"), 4);
            assert_eq!(count(result, "errors"), 0);
            for path in result["files"]
                .as_array()
                .ok_or("files missing")?
                .iter()
                .filter_map(Value::as_str)
            {
                let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
                let name = Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or("name missing")?;
                let document = photocraft_io::import(name, &bytes)
                    .map_err(|error| error.to_string())?
                    .document;
                let pixel = document
                    .layers
                    .iter()
                    .find_map(|layer| layer.surface())
                    .ok_or("decoded output has no raster")?
                    .rgba(0, 0);
                if name.starts_with("layers.") {
                    // Original bicubic sampling gives partial alpha at finite layer edges;
                    // JPEG also mattes that coverage. Preserve the original color semantics.
                    assert!(
                        pixel[1] > 0.9
                            && pixel[1] - pixel[0] > 0.5
                            && pixel[1] - pixel[2] > 0.5
                            && pixel[3] > 0.75,
                        "resized green output differs: {name} {pixel:?}"
                    );
                } else {
                    let expected = if name.starts_with("a.") {
                        [0.0, 0.0, 1.0, 1.0]
                    } else if name.starts_with("b.") {
                        [1.0, 0.0, 0.0, 1.0]
                    } else {
                        [0.0, 1.0, 0.0, 1.0]
                    };
                    let tolerance = if format == "jpg" { 0.03 } else { 0.00001 };
                    for (actual, expected) in pixel.into_iter().zip(expected) {
                        assert!(
                            (actual - expected).abs() <= tolerance,
                            "actual decoded output pixel differs: {name} {pixel:?}"
                        );
                    }
                }
                if name.starts_with("layers.") {
                    assert_eq!((document.size.width, document.size.height), (1, 2));
                    if format == "psd" {
                        assert_eq!(document.layers.len(), 2);
                    }
                } else {
                    assert_eq!(
                        (document.size.width, document.size.height),
                        (1, 1),
                        "dontEnlarge is preserved"
                    );
                }
                match format {
                    "png" => assert!(bytes.starts_with(b"\x89PNG")),
                    "jpg" => assert!(bytes.starts_with(&[255, 216])),
                    "psd" => assert!(bytes.starts_with(b"8BPS")),
                    _ => assert!(bytes.starts_with(b"II") || bytes.starts_with(b"MM")),
                }
            }
            drop(state);
            let journal = fixture
                .inputs
                .app
                .session
                .journal
                .last()
                .ok_or("original journal missing")?;
            assert_eq!(journal.0, COMMAND);
            assert_eq!(journal.1["output"], HANDLE);
            assert_eq!(journal.1["format"], format);
            assert!(!journal.1.to_string().contains("FolderPublication"));
            let receipt = fixture.receipt(&publish, "success", false)?;
            let external = fixture.inputs.root.join("provider-fixture");
            std::fs::create_dir(&external).map_err(|error| error.to_string())?;
            std::fs::write(external.join("marker"), b"untouched")
                .map_err(|error| error.to_string())?;
            let child = external.join(PUBLICATION);
            std::fs::create_dir(&child).map_err(|error| error.to_string())?;
            for entry in std::fs::read_dir(
                receipt["stageRoot"]
                    .as_str()
                    .ok_or("receipt tree missing")?,
            )
            .map_err(|error| error.to_string())?
            {
                let entry = entry.map_err(|error| error.to_string())?;
                std::fs::copy(entry.path(), child.join(entry.file_name()))
                    .map_err(|error| error.to_string())?;
            }
            fixture
                .processor
                .apply(receipt.clone(), &mut fixture.inputs.app)?;
            assert_eq!(
                fixture.inputs.app.ui.status,
                "Published 4 processed files; 0 inputs failed"
            );
            assert_eq!(
                std::fs::read(external.join("marker")).map_err(|error| error.to_string())?,
                b"untouched"
            );
            assert_eq!(
                std::fs::read_dir(&child)
                    .map_err(|error| error.to_string())?
                    .count(),
                4
            );
            assert_eq!(document_states(&fixture.inputs.app.session), original);
            assert!(
                !Path::new(
                    receipt["ownerRoot"]
                        .as_str()
                        .ok_or("receipt owner missing")?
                )
                .exists()
            );
            fixture.processor.apply(receipt, &mut fixture.inputs.app)?;
        }
        Ok(())
    }

    #[test]
    fn per_file_decode_and_case_insensitive_collision_errors_survive_successful_publication()
    -> Result<(), String> {
        let mut fixture = Fixture::new("failures")?;
        let request = fixture.start("png")?;
        let publish = fixture.encoded(&request)?;
        let result = fixture
            .processor
            .state
            .borrow()
            .job
            .as_ref()
            .and_then(|job| job.result.clone())
            .ok_or("result missing")?;
        assert_eq!(count(&result, "files"), 4);
        assert_eq!(count(&result, "errors"), 2);
        assert!(result["errors"].to_string().contains("same output name"));
        let receipt = fixture.receipt(&publish, "success", false)?;
        fixture.processor.apply(receipt, &mut fixture.inputs.app)?;
        assert_eq!(
            fixture.inputs.app.ui.status,
            "Published 4 processed files; 2 inputs failed"
        );
        assert!(fixture.inputs.app.ui.status_error);
        let message = fixture
            .inputs
            .app
            .ui
            .dialogs
            .iter()
            .find_map(|dialog| dialog.fields.get("message").and_then(Value::as_str))
            .ok_or("failed input details missing")?;
        assert!(message.contains("same output name"));
        assert!(message.contains("broken.png"));
        assert!(!message.contains("folder-imports"));
        assert!(!message.contains("FolderPublication"));
        Ok(())
    }

    #[test]
    fn destination_cancel_stale_and_duplicate_preserve_original_target_and_release_once()
    -> Result<(), String> {
        for closed in [false, true] {
            let mut fixture = Fixture::new("")?;
            fixture.choose()?;
            let visible = fixture.visible_form();
            assert!(visible.contains("4 image files"), "{visible}");
            assert!(visible.contains("Selected output folder"), "{visible}");
            assert!(visible.matches("Browse").count() >= 2, "{visible}");
            assert!(!visible.contains(HANDLE));
            assert!(!visible.contains("folder-imports"));
            assert!(!visible.contains("FolderPublication"));
            let original = fixture
                .inputs
                .app
                .ui
                .dialog_mut(fixture.dialog)
                .ok_or("form missing")?
                .fields["output"]
                .clone();
            folder_ui::request_destination(&mut fixture.inputs.app, fixture.dialog)?;
            let request = fixture.event("folderDestination")?;
            if closed {
                fixture.inputs.app.ui.close_dialog(fixture.dialog);
            } else {
                fixture
                    .inputs
                    .app
                    .ui
                    .dialog_mut(fixture.dialog)
                    .ok_or("form missing")?
                    .fields
                    .insert("__folder_output_cancel".into(), json!(true));
            }
            fixture.processor.finish_frame(&mut fixture.inputs.app);
            let _cancel = fixture.event("folderDestinationCancel")?;
            let handle = "00000000-0000-4000-8000-000000000003";
            let response = json!({"kind":"folderDestinationComplete","id":request["id"],"target":request["target"],"result":if closed {"success"} else {"cancel"},"destinationHandle":if closed {handle} else {""},"error":""});
            fixture
                .processor
                .apply(response.clone(), &mut fixture.inputs.app)?;
            fixture.processor.apply(response, &mut fixture.inputs.app)?;
            fixture.processor.finish_frame(&mut fixture.inputs.app);
            let output: Value = serde_json::from_str(&fixture.inputs.platform.take_json())
                .map_err(|error| error.to_string())?;
            let releases = output["events"]
                .as_array()
                .ok_or("events missing")?
                .iter()
                .filter(|event| {
                    event["kind"] == "folderDestinationRelease"
                        && event["destinationHandle"] == handle
                })
                .count();
            assert_eq!(releases, usize::from(closed));
            if !closed {
                assert_eq!(
                    fixture
                        .inputs
                        .app
                        .ui
                        .dialog_mut(fixture.dialog)
                        .ok_or("form missing")?
                        .fields["output"],
                    original
                );
                assert!(
                    fixture
                        .processor
                        .state
                        .borrow()
                        .selections
                        .contains_key(HANDLE)
                );
                assert!(folder_ui::ready(
                    &fixture.inputs.app,
                    &fixture
                        .inputs
                        .app
                        .ui
                        .dialogs
                        .iter()
                        .find(|form| form.id == fixture.dialog)
                        .ok_or("form missing")?
                        .fields
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn allocation_cancel_and_publication_failure_never_mark_working_documents_saved()
    -> Result<(), String> {
        for status in ["allocationCancel", "cancel", "error", "preflightError"] {
            let mut fixture = Fixture::new("")?;
            let original = document_states(&fixture.inputs.app.session);
            let request = fixture.start("png")?;
            if status == "allocationCancel" {
                let mut response = fixture.stage(&request)?;
                fixture
                    .inputs
                    .app
                    .services
                    .folder_progress
                    .as_mut()
                    .ok_or("progress missing")?(true);
                fixture.processor.finish_frame(&mut fixture.inputs.app);
                let cancel = fixture.event("folderStageCancel")?;
                assert_eq!(cancel["id"], request["id"]);
                response["result"] = json!("cancel");
                fixture
                    .processor
                    .apply(response.clone(), &mut fixture.inputs.app)?;
                assert!(
                    !Path::new(response["ownerRoot"].as_str().ok_or("owner missing")?).exists()
                );
            } else {
                let publish = fixture.encoded(&request)?;
                let response = if status == "preflightError" {
                    json!({"kind":"folderPublishComplete","id":publish["id"],"target":publish["target"],"result":"error","ownerRoot":"","stageRoot":"","journalPath":"","publicationName":"","publishedCount":0,"partialPossible":false,"retainStage":true,"error":"permission denied"})
                } else {
                    fixture.receipt(&publish, status, true)?
                };
                fixture
                    .processor
                    .apply(response.clone(), &mut fixture.inputs.app)?;
                if status == "preflightError" {
                    assert!(
                        Path::new(publish["stageRoot"].as_str().ok_or("tree missing")?).exists()
                    );
                    assert!(
                        fixture
                            .inputs
                            .app
                            .ui
                            .status
                            .starts_with("permission denied")
                    );
                } else {
                    assert!(
                        Path::new(response["stageRoot"].as_str().ok_or("tree missing")?).exists()
                    );
                    assert!(
                        Path::new(response["journalPath"].as_str().ok_or("journal missing")?)
                            .exists()
                    );
                    assert!(
                        fixture
                            .inputs
                            .app
                            .ui
                            .status
                            .contains("partial external output may remain")
                    );
                }
            }
            assert_eq!(document_states(&fixture.inputs.app.session), original);
            assert!(fixture.processor.state.borrow().job.is_none());
        }
        Ok(())
    }

    #[test]
    fn captured_processor_inputs_survive_active_document_switch_and_late_cancel_cannot_change_success()
    -> Result<(), String> {
        let mut fixture = Fixture::new("")?;
        let request = fixture.start("png")?;
        fixture.inputs.app.run(
            "file.new",
            json!({"width":17,"height":9,"name":"New active document"}),
        )?;
        let documents = document_states(&fixture.inputs.app.session);
        let publish = fixture.encoded(&request)?;
        fixture
            .inputs
            .app
            .services
            .folder_progress
            .as_mut()
            .ok_or("progress missing")?(true);
        fixture.processor.finish_frame(&mut fixture.inputs.app);
        let cancel = fixture.event("folderPublishCancel")?;
        assert_eq!(cancel["id"], publish["id"]);
        let response = fixture.receipt(&publish, "success", false)?;
        fixture.processor.apply(response, &mut fixture.inputs.app)?;
        assert_eq!(document_states(&fixture.inputs.app.session), documents);
        assert!(fixture.inputs.app.ui.status.starts_with("Published 4"));
        Ok(())
    }

    #[test]
    fn forged_stage_and_expired_logical_handle_cannot_grant_ambient_write_authority()
    -> Result<(), String> {
        let mut fixture = Fixture::new("")?;
        let request = fixture.start("png")?;
        let params = fixture
            .processor
            .state
            .borrow()
            .job
            .as_ref()
            .map(|job| job.params.clone())
            .ok_or("captured params missing")?;
        let response = fixture.stage(&request)?;
        let owner = PathBuf::from(response["ownerRoot"].as_str().ok_or("owner missing")?);
        let mut marker: Value = serde_json::from_slice(
            &std::fs::read(owner.join("owner.json")).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        marker["target"] = json!("another target");
        std::fs::write(owner.join("owner.json"), marker.to_string())
            .map_err(|error| error.to_string())?;
        assert!(
            fixture
                .processor
                .apply(response, &mut fixture.inputs.app)
                .is_err()
        );
        assert!(owner.exists());
        assert!(
            fixture
                .inputs
                .app
                .session
                .execute(COMMAND, params.clone())
                .is_err()
        );
        let files = FileRequests::new(&fixture.inputs.root)?;
        let restarted = Processor::new(&fixture.inputs.root, &fixture.inputs.platform, &files)?;
        restarted.attach(
            &mut fixture.inputs.app.services,
            &mut fixture.inputs.app.session,
        );
        assert!(
            fixture.inputs.app.run(COMMAND, params).is_err(),
            "a journal's expired handle must require a new authorization"
        );
        assert!(!fixture.inputs.root.join(HANDLE).exists());
        Ok(())
    }

    #[test]
    fn repeated_original_command_reuses_authorization_with_fresh_output_job() -> Result<(), String>
    {
        let mut fixture = Fixture::new("")?;
        let request = fixture.start("png")?;
        let params = fixture
            .processor
            .state
            .borrow()
            .job
            .as_ref()
            .map(|job| job.params.clone())
            .ok_or("captured params missing")?;
        let first = fixture.encoded(&request)?;
        let receipt = fixture.receipt(&first, "success", false)?;
        fixture.processor.apply(receipt, &mut fixture.inputs.app)?;
        assert_eq!(fixture.inputs.app.run(COMMAND, params)?["pending"], true);
        let second_request = fixture.event("folderStageAllocate")?;
        assert_ne!(second_request["id"], request["id"]);
        assert_eq!(second_request["target"], request["target"]);
        let second = fixture.encoded(&second_request)?;
        assert_ne!(second["ownerRoot"], first["ownerRoot"]);
        let receipt = fixture.receipt(&second, "success", false)?;
        fixture.processor.apply(receipt, &mut fixture.inputs.app)?;
        assert_eq!(
            fixture.inputs.app.ui.status,
            "Published 4 processed files; 0 inputs failed"
        );
        Ok(())
    }

    #[test]
    fn bounded_output_backpressure_can_cancel_encoded_unpublished_material_without_waiting_forever()
    -> Result<(), String> {
        let mut fixture = Fixture::new("")?;
        let request = fixture.start("png")?;
        let response = fixture.stage(&request)?;
        for _ in 0..64 {
            let id = crate::platform::next_id();
            fixture
                .inputs
                .platform
                .send_event(json!({"kind":"test","id":id}))?;
        }
        assert!(
            fixture
                .processor
                .apply(response.clone(), &mut fixture.inputs.app)
                .is_err()
        );
        assert!(
            fixture
                .processor
                .state
                .borrow()
                .job
                .as_ref()
                .is_some_and(|job| job.stage.is_some() && !job.publishing)
        );
        fixture
            .inputs
            .app
            .services
            .folder_progress
            .as_mut()
            .ok_or("progress missing")?(true);
        fixture.processor.finish_frame(&mut fixture.inputs.app);
        assert!(fixture.processor.state.borrow().job.is_none());
        assert!(Path::new(response["stageRoot"].as_str().ok_or("tree missing")?).exists());
        assert!(
            fixture
                .inputs
                .app
                .ui
                .status
                .contains("processed files retained")
        );
        Ok(())
    }
    #[test]
    fn retained_folder_retry_publishes_exact_bytes_without_reprocessing_inputs_or_changing_documents()
    -> Result<(), String> {
        let mut fixture = Fixture::new("")?;
        let request = fixture.start("png")?;
        let publish = fixture.encoded(&request)?;
        let receipt = fixture.receipt(&publish, "error", true)?;
        let tree = Path::new(receipt["stageRoot"].as_str().ok_or("tree missing")?);
        let expected = inspect(tree)?;
        let journal = fixture.inputs.app.session.journal.len();
        let original = document_states(&fixture.inputs.app.session);
        fixture.processor.apply(receipt, &mut fixture.inputs.app)?;
        assert!(fixture.visible_form().contains("Recover folder outputs"));
        let allocation = request["id"].as_u64().ok_or("id missing")?;
        // Retry must not need the original inputs or replay actions.
        std::fs::remove_dir_all(&fixture.processor.inputs).map_err(|error| error.to_string())?;
        fixture.processor.retry(allocation, false)?;
        let retry = fixture.event("folderPublish")?;
        assert_ne!(retry["id"], publish["id"]);
        assert_eq!(retry["target"], publish["target"]);
        assert_eq!(
            inspect(Path::new(
                retry["stageRoot"].as_str().ok_or("tree missing")?
            ))?,
            expected
        );
        assert_eq!(fixture.inputs.app.session.journal.len(), journal);
        let receipt = fixture.receipt(&retry, "success", false)?;
        fixture.processor.apply(receipt, &mut fixture.inputs.app)?;
        assert!(fixture.processor.state.borrow().recoveries.is_empty());
        assert_eq!(document_states(&fixture.inputs.app.session), original);
        Ok(())
    }

    #[test]
    fn restarted_folder_recovery_keeps_identity_and_can_choose_another_authorized_folder()
    -> Result<(), String> {
        let mut fixture = Fixture::new("")?;
        let request = fixture.start("png")?;
        let publish = fixture.encoded(&request)?;
        let receipt = fixture.receipt(&publish, "cancel", false)?;
        fixture.processor.apply(receipt, &mut fixture.inputs.app)?;
        let original = document_states(&fixture.inputs.app.session);
        let processor = Processor::new(
            &fixture.inputs.root,
            &fixture.inputs.platform,
            &fixture.processor.files,
        )?;
        let allocation = request["id"].as_u64().ok_or("id missing")?;
        assert_eq!(processor.state.borrow().recoveries.len(), 1);
        processor.retry(allocation, true)?;
        let selected = fixture.event("folderDestination")?;
        assert_eq!(selected["target"], publish["target"]);
        processor.apply(json!({"kind":"folderDestinationComplete","id":selected["id"],"target":selected["target"],"result":"cancel","destinationHandle":"","error":""}),&mut fixture.inputs.app)?;
        assert!(processor.state.borrow().job.is_none());
        assert_eq!(processor.state.borrow().recoveries.len(), 1);
        processor.retry(allocation, true)?;
        let selected = fixture.event("folderDestination")?;
        let replacement = "00000000-0000-4000-8000-000000000004";
        processor.apply(json!({"kind":"folderDestinationComplete","id":selected["id"],"target":selected["target"],"result":"success","destinationHandle":replacement,"error":""}),&mut fixture.inputs.app)?;
        let retry = fixture.event("folderPublish")?;
        assert_eq!(retry["destinationHandle"], replacement);
        assert_eq!(retry["target"], publish["target"]);
        assert_ne!(retry["id"], publish["id"]);
        assert_eq!(document_states(&fixture.inputs.app.session), original);
        Ok(())
    }

    #[test]
    fn folder_recovery_refuses_changed_bytes_links_owner_and_journal_without_publication()
    -> Result<(), String> {
        let mut fixture = Fixture::new("")?;
        let request = fixture.start("png")?;
        let publish = fixture.encoded(&request)?;
        let receipt = fixture.receipt(&publish, "error", false)?;
        let tree = PathBuf::from(receipt["stageRoot"].as_str().ok_or("tree missing")?);
        let owner = PathBuf::from(receipt["ownerRoot"].as_str().ok_or("owner missing")?);
        fixture.processor.apply(receipt, &mut fixture.inputs.app)?;
        let allocation = request["id"].as_u64().ok_or("id missing")?;
        let file = std::fs::read_dir(&tree)
            .map_err(|error| error.to_string())?
            .next()
            .ok_or("file missing")?
            .map_err(|error| error.to_string())?
            .path();
        let bytes = std::fs::read(&file).map_err(|error| error.to_string())?;
        let mut changed = bytes.clone();
        if let Some(byte) = changed.first_mut() {
            *byte ^= 1;
        }
        std::fs::write(&file, &changed).map_err(|error| error.to_string())?;
        assert!(fixture.processor.retry(allocation, false).is_err());
        std::fs::write(&file, &bytes).map_err(|error| error.to_string())?;
        std::fs::remove_file(&file).map_err(|error| error.to_string())?;
        std::os::unix::fs::symlink(owner.join("owner.json"), &file)
            .map_err(|error| error.to_string())?;
        assert!(fixture.processor.retry(allocation, false).is_err());
        std::fs::remove_file(&file).map_err(|error| error.to_string())?;
        std::fs::write(&file, &bytes).map_err(|error| error.to_string())?;
        for name in ["owner.json", "publication.json"] {
            let path = owner.join(name);
            let original = std::fs::read(&path).map_err(|error| error.to_string())?;
            let mut marker: Value =
                serde_json::from_slice(&original).map_err(|error| error.to_string())?;
            marker["target"] = json!("another task");
            std::fs::write(&path, marker.to_string()).map_err(|error| error.to_string())?;
            assert!(fixture.processor.retry(allocation, false).is_err());
            std::fs::write(&path, original).map_err(|error| error.to_string())?;
        }
        assert!(fixture.processor.state.borrow().job.is_none());
        assert_eq!(fixture.processor.state.borrow().recoveries.len(), 1);
        assert!(tree.exists());
        Ok(())
    }
    #[test]
    fn older_folder_outputs_require_new_destination_and_do_not_invent_input_failure_counts()
    -> Result<(), String> {
        let mut fixture = Fixture::new("")?;
        let request = fixture.start("png")?;
        let publish = fixture.encoded(&request)?;
        let receipt = fixture.receipt(&publish, "error", true)?;
        let owner = PathBuf::from(receipt["ownerRoot"].as_str().ok_or("owner missing")?);
        fixture.processor.apply(receipt, &mut fixture.inputs.app)?;
        std::fs::remove_file(owner.join("recovery.json")).map_err(|error| error.to_string())?;
        let recovered = Processor::new(
            &fixture.inputs.root,
            &fixture.inputs.platform,
            &fixture.processor.files,
        )?;
        let allocation = request["id"].as_u64().ok_or("id missing")?;
        let state = recovered.state.borrow();
        let record = state
            .recoveries
            .get(&allocation)
            .ok_or("legacy record missing")?;
        assert!(record.summary().unknown_failures);
        assert!(record.summary().partial);
        assert!(record.handle.is_empty());
        drop(state);
        assert!(recovered.retry(allocation, false).is_err());
        recovered.retry(allocation, true)?;
        assert_eq!(
            fixture.event("folderDestination")?["target"],
            publish["target"]
        );
        Ok(())
    }
}
