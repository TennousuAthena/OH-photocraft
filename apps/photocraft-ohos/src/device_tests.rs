//! Opt-in device control. This transport never supplies picker results or filesystem authority.
use std::collections::BTreeMap;
use std::path::{Component, Path};
use std::sync::{Arc, Mutex, mpsc};

use photocraft_ui_egui::{ControlRequest, PhotocraftApp};
use serde::Deserialize;
use serde_json::{Value, json};

const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MAX_RESULT_BYTES: usize = 1024 * 1024;
const MAX_PENDING: usize = 32;

pub(crate) type SharedMailbox = Arc<Mutex<Mailbox>>;

pub(crate) struct Mailbox {
    run_id: String,
    next_ticket: u64,
    pending: BTreeMap<String, mpsc::Receiver<Value>>,
    snapshot: String,
}

pub(crate) struct DeviceSession {
    control: mpsc::Sender<ControlRequest>,
    pub(crate) mailbox: SharedMailbox,
    pixels: Option<(u64, u64, Value)>,
    native_key_events: u64,
    native_pointer_events: u64,
    native_text_events: u64,
    last_native_key_code: Option<i32>,
    last_file_completion: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireRequest {
    method: String,
    #[serde(default = "empty_params")]
    params: Value,
}

fn empty_params() -> Value {
    json!({})
}

fn run_id_for_root(root: &Path, leaf: &str) -> Result<Option<String>, String> {
    if !root.is_absolute()
        || root
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err("device test storage roots must be absolute and normalized".into());
    }
    let parts: Vec<_> = root.components().collect();
    let marker = Component::Normal("PhotoCraftTestRuns".as_ref());
    let Some(index) = parts.iter().position(|part| *part == marker) else {
        return Ok(None);
    };
    if index + 3 != parts.len() || root.file_name().and_then(|part| part.to_str()) != Some(leaf) {
        return Err(
            "device test root must end in PhotoCraftTestRuns/<runId>/files or cache".into(),
        );
    }
    let run_id = parts
        .get(index + 1)
        .and_then(|part| part.as_os_str().to_str())
        .ok_or("invalid device test run ID")?;
    if run_id.is_empty()
        || run_id.len() > 64
        || !run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("invalid device test run ID".into());
    }
    Ok(Some(run_id.into()))
}

pub(crate) fn run_id(files: &Path, cache: &Path) -> Result<Option<String>, String> {
    let files_id = run_id_for_root(files, "files")?;
    let cache_id = run_id_for_root(cache, "cache")?;
    if files_id != cache_id {
        return Err("device test files and cache must belong to the same run".into());
    }
    Ok(files_id)
}

impl DeviceSession {
    pub(crate) fn new(
        files: &Path,
        cache: &Path,
    ) -> Result<Option<(Self, mpsc::Receiver<ControlRequest>)>, String> {
        let Some(run_id) = run_id(files, cache)? else {
            return Ok(None);
        };
        let (control, receiver) = mpsc::channel();
        Ok(Some((
            Self {
                control,
                mailbox: Arc::new(Mutex::new(Mailbox {
                    run_id,
                    next_ticket: 0,
                    pending: BTreeMap::new(),
                    snapshot: String::new(),
                })),
                pixels: None,
                native_key_events: 0,
                native_pointer_events: 0,
                native_text_events: 0,
                last_native_key_code: None,
                last_file_completion: Value::Null,
            },
            receiver,
        )))
    }

    pub(crate) fn enqueue(&self, request: ControlRequest) -> Result<(), String> {
        self.control
            .send(request)
            .map_err(|_| "device test control channel stopped".into())
    }

    pub(crate) fn native_key(&mut self, code: i32, text: &str) {
        self.native_key_events = self.native_key_events.saturating_add(1);
        self.last_native_key_code = Some(code);
        if !text.is_empty() {
            self.native_text_events = self.native_text_events.saturating_add(1);
        }
    }

    pub(crate) fn native_pointer(&mut self) {
        self.native_pointer_events = self.native_pointer_events.saturating_add(1);
    }

    pub(crate) fn native_ime_commit(&mut self, text: &str) {
        if !text.is_empty() {
            self.native_text_events = self.native_text_events.saturating_add(1);
        }
    }

    pub(crate) fn file_completion(&mut self, id: u64, result: &str, name: &str, accepted: bool) {
        self.last_file_completion =
            json!({"id":id,"result":result,"displayName":name,"accepted":accepted});
    }

    pub(crate) fn capture(&mut self, app: &PhotocraftApp, ctx: &egui::Context, mut runtime: Value) {
        runtime["input"] = json!({"nativeKeyEvents":self.native_key_events,
            "nativePointerEvents":self.native_pointer_events,"nativeTextEvents":self.native_text_events,
            "lastNativeKeyCode":self.last_native_key_code});
        runtime["lastFileCompletion"] = self.last_file_completion.clone();
        let pixels = app.session.active().map(|state| {
            let key = (state.doc.id.0, state.revision);
            if self.pixels.as_ref().is_none_or(|(id, revision, _)| (*id, *revision) != key) {
                let width = state.doc.size.width;
                let height = state.doc.size.height;
                let value = if u64::from(width) * u64::from(height) <= 4096 {
                    match photocraft_io::export(&state.doc, "device-oracle.png", &Default::default())
                        .map_err(|error| error.to_string())
                        .and_then(|output| photocraft_codecs::decode(&output.bytes).map_err(|error| error.to_string()))
                    {
                        Ok(image) => {
                            let rgba = image.to_rgba8();
                            json!({"width":width,"height":height,"rgba8":rgba,"blake3":blake3::hash(&rgba).to_hex().to_string()})
                        }
                        Err(error) => json!({"width":width,"height":height,"error":error}),
                    }
                } else {
                    json!({"width":width,"height":height,"omitted":"device pixel oracle is limited to 4096 pixels"})
                };
                self.pixels = Some((key.0, key.1, value));
            }
            self.pixels.as_ref().map(|(_, _, value)| value.clone()).unwrap_or(Value::Null)
        }).unwrap_or(Value::Null);
        let mut mailbox = self
            .mailbox
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let canvas = app.last_canvas_rect;
        mailbox.snapshot = bounded_json(json!({
            "runId":mailbox.run_id,"runtime":runtime,"ui":photocraft_ui_egui::control::inspect(app, ctx),
            "canvasRect":[canvas.min.x,canvas.min.y,canvas.width(),canvas.height()],"documentPixels":pixels
        }));
    }
}

fn contains_path(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            matches!(
                key.as_str(),
                "path" | "paths" | "resolvedPath" | "sourceUri" | "destinationUri" | "uri"
            ) || contains_path(value)
        }),
        Value::Array(values) => values.iter().any(contains_path),
        _ => false,
    }
}

fn autosave_values(value: &Value) -> bool {
    let Some(values) = value.as_object() else {
        return false;
    };
    if values.len() != 1 {
        return false;
    }
    let Some(file_handling) = values.get("fileHandling").and_then(Value::as_object) else {
        return false;
    };
    !file_handling.is_empty()
        && file_handling
            .iter()
            .all(|(field, value)| match field.as_str() {
                "autosave" | "recoverOnLaunch" => value.is_boolean(),
                "autosaveMinutes" => value
                    .as_u64()
                    .is_some_and(|minutes| (1..=240).contains(&minutes)),
                _ => false,
            })
}

fn parse_request(text: &str) -> Result<WireRequest, String> {
    if text.len() > MAX_REQUEST_BYTES {
        return Err("device test request exceeds 64 KiB".into());
    }
    let request: WireRequest = serde_json::from_str(text).map_err(|error| error.to_string())?;
    if !request.params.is_object() || contains_path(&request.params) {
        return Err("device tests cannot supply filesystem paths or picker results".into());
    }
    let allowed = match request.method.as_str() {
        "ui.inspect" | "ui.menu.list" | "ui.key" | "ui.type" | "ui.click" | "ui.move"
        | "ui.pointer" | "ui.dialog.confirm" | "ui.dialog.cancel" => true,
        "ui.close.save" => request
            .params
            .as_object()
            .is_some_and(|params| params.is_empty()),
        "ui.menu.invoke" => {
            if request.params.get("id").is_some() && request.params.get("command").is_some() {
                return Err("device test menu requests must supply only id or command".into());
            }
            let id = request
                .params
                .get("id")
                .or_else(|| request.params.get("command"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if id == "document.activate" {
                request.params.get("id").and_then(Value::as_str) == Some("document.activate")
                    && request
                        .params
                        .as_object()
                        .is_some_and(|params| params.len() == 2)
                    && request
                        .params
                        .get("params")
                        .and_then(Value::as_object)
                        .is_some_and(|params| {
                            params.len() == 1
                                && params.get("document").and_then(Value::as_u64).is_some()
                        })
            } else {
                matches!(
                    id,
                    "file.new"
                        | "file.open"
                        | "file.save"
                        | "file.saveAs"
                        | "file.saveACopy"
                        | "file.export.exportAs"
                        | "file.export.quickExportAsPng"
                        | "file.close"
                        | "file.exit"
                        | "file.clearRecent"
                        | "image.adjustments.invert"
                        | "edit.undo"
                        | "edit.redo"
                        | "edit.copy"
                        | "edit.paste"
                        | "edit.preferences.fileHandling"
                        | "layer.new.layer"
                        | "view.fitOnScreen"
                        | "view.actualPixels"
                ) || id
                    .strip_prefix("file.openRecent.")
                    .is_some_and(|suffix| suffix.parse::<u32>().is_ok())
            }
        }
        "ui.dialog.open" => matches!(
            request.params.get("kind").and_then(Value::as_str),
            Some("newDocument" | "about")
        ),
        "ui.dialog.set" => match request.params.get("field").and_then(Value::as_str) {
            Some("values") => request.params.get("value").is_some_and(autosave_values),
            Some(
                "name" | "width" | "height" | "resolution" | "resolutionDpi" | "mode" | "depth"
                | "background",
            ) => true,
            _ => false,
        },
        _ => false,
    };
    if !allowed {
        return Err("device test control method or command is not allowed".into());
    }
    if request
        .params
        .get("events")
        .and_then(Value::as_array)
        .is_some_and(|events| events.len() > 64)
    {
        return Err("device test pointer event batch exceeds 64 events".into());
    }
    if request
        .params
        .get("count")
        .is_some_and(|value| value.as_u64().is_none_or(|count| count > 3))
    {
        return Err("device test click count must be at most 3".into());
    }
    Ok(request)
}

fn bounded_json(value: Value) -> String {
    let text = value.to_string();
    if text.len() <= MAX_RESULT_BYTES {
        text
    } else {
        json!({"ok":false,"error":"device test result exceeds 1 MiB"}).to_string()
    }
}

pub(crate) fn submit(
    mailbox: &SharedMailbox,
    run_id: &str,
    text: &str,
    enqueue: impl FnOnce(ControlRequest) -> Result<(), String>,
) -> Result<String, String> {
    let request = parse_request(text)?;
    let (control, response) = ControlRequest::new(request.method, request.params);
    let ticket = {
        let mut state = mailbox.lock().unwrap_or_else(|error| error.into_inner());
        if state.run_id != run_id {
            return Err("device test run ID does not match initialized storage".into());
        }
        if state.pending.len() >= MAX_PENDING {
            return Err("device test result mailbox is full; poll outstanding tickets".into());
        }
        state.next_ticket = state
            .next_ticket
            .checked_add(1)
            .ok_or("device test ticket sequence exhausted")?;
        let ticket = state.next_ticket.to_string();
        state.pending.insert(ticket.clone(), response);
        ticket
    };
    if let Err(error) = enqueue(control) {
        mailbox
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pending
            .remove(&ticket);
        return Err(error);
    }
    Ok(ticket)
}

pub(crate) fn poll(mailbox: &SharedMailbox, run_id: &str, ticket: &str) -> Result<String, String> {
    let mut state = mailbox.lock().unwrap_or_else(|error| error.into_inner());
    if state.run_id != run_id {
        return Err("device test run ID does not match initialized storage".into());
    }
    let response = state
        .pending
        .get(ticket)
        .ok_or("unknown or already consumed device test ticket")?
        .try_recv();
    match response {
        Ok(value) => {
            state.pending.remove(ticket);
            Ok(bounded_json(value))
        }
        Err(mpsc::TryRecvError::Empty) => Ok(String::new()),
        Err(mpsc::TryRecvError::Disconnected) => {
            state.pending.remove(ticket);
            Ok(
                json!({"ok":false,"error":"device test control response channel stopped"})
                    .to_string(),
            )
        }
    }
}

pub(crate) fn snapshot(mailbox: &SharedMailbox, run_id: &str) -> Result<String, String> {
    let state = mailbox.lock().unwrap_or_else(|error| error.into_inner());
    if state.run_id != run_id {
        return Err("device test run ID does not match initialized storage".into());
    }
    Ok(state.snapshot.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Result<(DeviceSession, mpsc::Receiver<ControlRequest>), String> {
        DeviceSession::new(
            Path::new("/files/PhotoCraftTestRuns/run_1/files"),
            Path::new("/cache/PhotoCraftTestRuns/run_1/cache"),
        )?
        .ok_or("missing isolated session".into())
    }

    #[test]
    fn isolation_rejects_partial_mismatched_and_traversal_roots() -> Result<(), String> {
        assert_eq!(
            run_id(Path::new("/normal/files"), Path::new("/normal/cache"))?,
            None
        );
        assert!(
            run_id(
                Path::new("/a/PhotoCraftTestRuns/one/files"),
                Path::new("/b/PhotoCraftTestRuns/two/cache")
            )
            .is_err()
        );
        assert!(
            run_id(
                Path::new("/a/PhotoCraftTestRuns/one/files"),
                Path::new("/normal/cache")
            )
            .is_err()
        );
        assert!(
            run_id(
                Path::new("/a/../PhotoCraftTestRuns/one/files"),
                Path::new("/b/PhotoCraftTestRuns/one/cache")
            )
            .is_err()
        );
        assert!(
            run_id(
                Path::new("/a/PhotoCraftTestRuns/one/files/extra"),
                Path::new("/b/PhotoCraftTestRuns/one/cache")
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn mailbox_is_nonblocking_bounded_and_run_owned() -> Result<(), String> {
        let (session, receiver) = session()?;
        let mut requests = Vec::new();
        for _ in 0..MAX_PENDING {
            let ticket = submit(
                &session.mailbox,
                "run_1",
                r#"{"method":"ui.inspect"}"#,
                |request| session.enqueue(request),
            )?;
            assert_eq!(poll(&session.mailbox, "run_1", &ticket)?, "");
            requests.push((
                ticket,
                receiver.try_recv().map_err(|error| error.to_string())?,
            ));
        }
        assert!(
            submit(
                &session.mailbox,
                "run_1",
                r#"{"method":"ui.inspect"}"#,
                |request| session.enqueue(request)
            )
            .is_err()
        );
        let (ticket, request) = requests.first().ok_or("missing request")?;
        assert!(poll(&session.mailbox, "other", ticket).is_err());
        request
            .reply
            .send(json!({"ok":true,"result":{"actual":"response"}}))
            .map_err(|error| error.to_string())?;
        assert_eq!(
            serde_json::from_str::<Value>(&poll(&session.mailbox, "run_1", ticket)?)
                .map_err(|error| error.to_string())?["result"]["actual"],
            "response"
        );
        assert!(poll(&session.mailbox, "run_1", ticket).is_err());
        Ok(())
    }

    #[test]
    fn rejected_submission_releases_its_slot_and_cannot_supply_file_authority() -> Result<(), String>
    {
        let (session, _) = session()?;
        for text in [
            r#"{"method":"app.open","params":{"path":"external.png"}}"#,
            r#"{"method":"ui.menu.invoke","params":{"id":"file.open","params":{"path":"external.png"}}}"#,
            r#"{"method":"engine.execute","params":{"command":"file.open"}}"#,
            r#"{"method":"ui.dialog.set","params":{"field":"path","value":"external.png"}}"#,
            r#"{"method":"completeFileRequest","params":{}}"#,
            r#"{"method":"ui.dialog.set","params":{"field":"values","value":{"fileHandling":{"recentFiles":["external.png"]}}}}"#,
            r#"{"method":"ui.dialog.set","params":{"field":"values","value":{"scratchDisks":{"disks":[{"path":"/external"}]}}}}"#,
        ] {
            assert!(submit(&session.mailbox, "run_1", text, |_| Ok(())).is_err());
        }
        for _ in 0..MAX_PENDING + 1 {
            assert!(
                submit(
                    &session.mailbox,
                    "run_1",
                    r#"{"method":"ui.inspect"}"#,
                    |_| Err("full worker queue".into())
                )
                .is_err()
            );
        }
        assert!(
            session
                .mailbox
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .pending
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn document_activation_admits_only_the_original_document_index_parameter() {
        assert!(parse_request(r#"{"method":"ui.menu.invoke","params":{"id":"document.activate","params":{"document":1}}}"#).is_ok());
        for params in [
            json!({"id":"document.activate"}),
            json!({"command":"document.activate","params":{"document":0}}),
            json!({"id":"document.activate","params":{"document":0},"document":1}),
            json!({"id":"document.activate","params":{"document":0,"other":true}}),
            json!({"id":"document.activate","params":{"document":-1}}),
            json!({"id":"document.activate","params":{"document":1.5}}),
            json!({"id":"document.activate","params":{"document":"1"}}),
        ] {
            assert!(
                parse_request(&json!({"method":"ui.menu.invoke","params":params}).to_string())
                    .is_err()
            );
        }
    }

    #[test]
    fn device_preferences_only_admit_original_autosave_settings() {
        assert!(parse_request(r#"{"method":"ui.dialog.set","params":{"dialog":1,"field":"values","value":{"fileHandling":{"autosave":true,"autosaveMinutes":1,"recoverOnLaunch":true}}}}"#).is_ok());
        assert!(parse_request(r#"{"method":"ui.dialog.set","params":{"dialog":1,"field":"values","value":{"fileHandling":{"autosaveMinutes":0}}}}"#).is_err());
        assert!(parse_request(r#"{"method":"ui.close.save","params":{}}"#).is_ok());
        assert!(parse_request(r#"{"method":"ui.close.save","params":{"force":true}}"#).is_err());
    }

    #[test]
    fn menu_requests_cannot_hide_an_unapproved_command_behind_an_allowed_id() {
        for command in ["layer.delete", "file.open"] {
            let request =
                json!({"method":"ui.menu.invoke", "params":{"id":"file.open","command":command}});
            assert!(parse_request(&request.to_string()).is_err());
        }
        assert!(
            parse_request(r#"{"method":"ui.menu.invoke","params":{"command":"file.open"}}"#)
                .is_ok()
        );
        assert!(
            parse_request(r#"{"method":"ui.menu.invoke","params":{"id":"file.open"}}"#).is_ok()
        );
    }
}
