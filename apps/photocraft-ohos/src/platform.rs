//! Native UI work is pulled from a bounded mailbox. The render worker never
//! calls the IME or system clipboard, and native callbacks never wait for it.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use craft_ohos_platform::input::InputState;
use egui::{Event, ImeEvent, Key};
use photocraft_ui_egui::{PhotocraftApp, i18n};
use serde_json::{Value, json};

const MAX_EVENTS: usize = 64;
const MAX_TEXT: usize = 1024 * 1024;
const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_IMAGE_PATH_BYTES: usize = 4096;

#[derive(Clone)]
pub struct Platform {
    cache: PathBuf,
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    ime: Option<TextSnapshot>,
    cursor: egui::CursorIcon,
    viewport: Option<(bool, bool)>,
    events: VecDeque<Value>,
    paste: Option<Paste>,
    ready_image: Option<(u32, u32, Vec<u8>)>,
    ready_paste: bool,
    images: HashMap<u64, PathBuf>,
    prints: HashMap<u64, PrintJob>,
    error: Option<String>,
    close: Option<CloseState>,
    editing_actions: Vec<Event>,
    folder_slot: Option<u64>,
}

#[derive(Clone)]
struct CloseState {
    id: u64,
    pending: bool,
}

struct PrintJob {
    path: PathBuf,
    document: u64,
}

#[derive(Clone)]
struct Paste {
    id: u64,
    intent: String,
    target: PasteTarget,
    authorized: bool,
}

#[derive(Clone)]
enum PasteTarget {
    Text { target: String, owner: String },
    Document(Option<u64>),
}

#[derive(Clone)]
pub struct TextSnapshot {
    target: String,
    owner: String,
    text: String,
    anchor: usize,
    caret: usize,
    composition: Option<(usize, usize)>,
    output: egui::output::IMEOutput,
    density: f32,
    zoom: f32,
}

impl TextSnapshot {
    fn json(&self) -> Value {
        let rect = |r: egui::Rect| [r.min.x, r.min.y, r.width(), r.height()];
        json!({
            "target": self.target, "text": self.text,
            "selectionStart": char_to_utf16(&self.text, self.anchor),
            "selectionEnd": char_to_utf16(&self.text, self.caret),
            "compositionStart": self.composition.map(|(start, _)| char_to_utf16(&self.text, start)),
            "compositionEnd": self.composition.map(|(start, len)| char_to_utf16(&self.text, start + len)),
            "rect": rect(self.output.rect), "cursorRect": rect(self.output.cursor_rect),
            "purpose": match self.output.purpose { egui::IMEPurpose::Normal => "normal", egui::IMEPurpose::Password => "password", egui::IMEPurpose::Terminal => "terminal" },
            "interrupt": self.output.should_interrupt_composition,
            "zoom": self.zoom, "density": self.density,
        })
    }
}

pub struct KeyInput<'a> {
    pub code: i32,
    pub pressed: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub text: &'a str,
}

pub struct Input(pub(crate) Value);

impl Input {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > MAX_TEXT {
            return Err("platform input exceeds 1 MiB".into());
        }
        let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let kind = string(&value, "kind")?;
        if !matches!(
            kind,
            "imePreedit"
                | "imeCommit"
                | "imeCancel"
                | "imeDelete"
                | "imeSelection"
                | "imeMove"
                | "imeAction"
                | "imeEnter"
                | "clipboard"
                | "platformComplete"
                | "closeRequested"
                | "clipboardAuthorize"
                | "viewportState"
                | "printingUpdate"
                | "folderProgress"
                | "folderComplete"
                | "folderDestinationComplete"
                | "folderStageComplete"
                | "folderPublishProgress"
                | "folderPublishComplete"
                | "recentSourcesChanged"
        ) {
            return Err("unknown platform input".into());
        }
        Ok(Self(value))
    }
}

/// Per-worker IME state. Native callbacks use only a target token and enqueue.
#[derive(Default)]
pub struct Editor {
    composition: Option<OriginalSelection>,
    restore_selection: Option<(String, usize, usize)>,
}

struct OriginalSelection {
    target: String,
    owner: String,
    selected: String,
    anchor: usize,
    caret: usize,
}

static GLOBAL: OnceLock<Mutex<Option<Platform>>> = OnceLock::new();

pub fn output_json() -> String {
    let platform = GLOBAL
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    platform.map_or_else(
        || {
            json!({"ime": null, "cursorIcon":"Default", "events": [], "printJobs": [], "closePending": false, "closeState": null})
                .to_string()
        },
        |platform| platform.take_json(),
    )
}

impl Platform {
    pub fn new(cache: &Path) -> Result<Self, String> {
        if !cache.is_absolute() {
            return Err("clipboard requires an absolute cacheDir".into());
        }
        let cache = cache.join("Clipboard");
        std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
        Ok(Self {
            cache,
            state: Arc::default(),
        })
    }

    pub fn install_global(&self) {
        *GLOBAL
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(self.clone());
    }

    fn push(&self, mut event: Value) -> Result<u64, String> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.events.len() >= MAX_EVENTS {
            return Err("system output queue is full".into());
        }
        let id = next_id();
        event["id"] = json!(id);
        state.events.push_back(event);
        Ok(id)
    }

    pub(crate) fn send_event(&self, event: Value) -> Result<(), String> {
        let _id = event["id"]
            .as_u64()
            .filter(|id| *id <= 9_007_199_254_740_991)
            .ok_or("invalid system request ID")?;
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.events.len() >= MAX_EVENTS {
            return Err("system output queue is full".into());
        }
        state.events.push_back(event);
        Ok(())
    }

    pub(crate) fn acquire_folder(&self, id: u64) -> Result<(), String> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.folder_slot.is_some() {
            return Err("Finish the current folder operation first".into());
        }
        state.folder_slot = Some(id);
        Ok(())
    }

    pub(crate) fn release_folder(&self, id: u64) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.folder_slot == Some(id) {
            state.folder_slot = None;
        }
    }

    pub fn take_json(&self) -> String {
        let (ime, cursor, events, close, prints) = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.ime.clone(),
                state.cursor,
                state.events.drain(..).collect::<Vec<_>>(),
                state.close.clone(),
                state
                    .prints
                    .iter()
                    .map(|(id, job)| json!({"id":id,"documentId":job.document.to_string()}))
                    .collect::<Vec<_>>(),
            )
        };
        json!({"ime": ime.map(|ime| ime.json()), "cursorIcon":format!("{cursor:?}"), "events": events, "printJobs":prints, "closePending": close.as_ref().is_some_and(|close| close.pending), "closeState": close.map(|close| json!({"id": close.id, "pending": close.pending}))}).to_string()
    }

    /// Actual OS window state, never the requested target state, drives the
    /// original title bar's maximize/restore toggle on the next UI pass.
    pub fn prepare_input(&self, input: &mut egui::RawInput) {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((fullscreen, maximized)) = state.viewport
            && let Some(root) = input.viewports.get_mut(&egui::ViewportId::ROOT)
        {
            root.fullscreen = Some(fullscreen);
            root.maximized = Some(maximized);
        }
    }

    fn current(&self) -> Option<TextSnapshot> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .ime
            .clone()
    }

    pub fn error(&self, message: &str) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).error = Some(message.into());
    }

    pub(crate) fn check_print_capacity(&self) -> Result<(), String> {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.prints.len() >= 8 {
            return Err("Finish an existing print job first (maximum 8)".into());
        }
        if state.events.len() >= MAX_EVENTS {
            return Err("system output queue is full".into());
        }
        Ok(())
    }

    pub(crate) fn queue_print(
        &self,
        id: u64,
        path: &Path,
        document: u64,
        copies: u32,
        printer: Option<&str>,
    ) -> Result<(), String> {
        self.check_print_capacity()?;
        if id > 9_007_199_254_740_991 {
            return Err("invalid print request id".into());
        }
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.prints.len() >= 8 || state.events.len() >= MAX_EVENTS {
            return Err("system print/output queue is full".into());
        }
        if state.prints.contains_key(&id) {
            return Err("print request id already exists".into());
        }
        state.prints.insert(
            id,
            PrintJob {
                path: path.into(),
                document,
            },
        );
        state
            .events
            .push_back(json!({"kind":"printRequest", "id":id, "pdfPath":path.to_string_lossy(), "documentId":document.to_string(), "requestedCopies":copies, "requestedPrinter":printer}));
        Ok(())
    }

    fn printing_update(&self, packet: &Value, app: &mut PhotocraftApp) -> Result<(), String> {
        let id = number(packet, "id")? as u64;
        let phase = string(packet, "state")?;
        let terminal = match phase {
            "submitted" | "blocked" | "monitoringError" => false,
            "succeeded" | "failed" | "cancelled" | "rejected" => true,
            _ => return Err("unknown system print state".into()),
        };
        if packet["terminal"].as_bool() != Some(terminal) {
            return Err("system print terminal flag does not match its state".into());
        }
        let error = string(packet, "error")?;
        let path = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if !state.prints.contains_key(&id) {
                return Err("print request is no longer pending".into());
            }
            if terminal {
                state.prints.remove(&id).map(|job| job.path)
            } else {
                None
            }
        };
        if let Some(path) = path {
            crate::printing::cleanup(&path);
        }
        let status = i18n::t(match phase {
            "submitted" => "System printing request accepted; final job state is unconfirmed",
            "blocked" => "System print job is blocked and can resume",
            "monitoringError" => "Print status monitoring failed; job result is unknown",
            "succeeded" => "System print job completed",
            "failed" => "System print job failed",
            "cancelled" => "System print job cancelled",
            "rejected" => "System print request was rejected",
            _ => return Err("unknown system print state".into()),
        });
        app.ui.status = if error.is_empty() {
            status.into()
        } else {
            i18n::fmt(
                i18n::t("{status}: {error}"),
                &[("status", status), ("error", i18n::t(error))],
            )
        };
        app.ui.status_error = matches!(phase, "failed" | "rejected" | "monitoringError");
        Ok(())
    }

    pub fn attach_services(&self, hooks: &mut photocraft_ui_egui::Services) {
        let copy = self.clone();
        hooks.clipboard_set_image = Some(Box::new(move |width, height, rgba| {
            let result = copy.copy_image(width, height, rgba);
            if let Err(error) = &result {
                copy.error(error);
            }
            result.map(|_| ())
        }));
        let get = self.clone();
        hooks.clipboard_get_image = Some(Box::new(move || {
            get.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .ready_image
                .take()
        }));
        let request = self.clone();
        hooks.clipboard_request = Some(Box::new(move |intent, document| {
            {
                let mut state = request.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.ready_paste {
                    state.ready_paste = false;
                    return Ok(false);
                }
            }
            request.request_paste(intent, PasteTarget::Document(document))?;
            Ok(true)
        }));
    }

    fn request_paste(&self, intent: &str, target: PasteTarget) -> Result<(), String> {
        if self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .paste
            .is_some()
        {
            return Err("clipboard paste is already pending".into());
        }
        let text_only = matches!(target, PasteTarget::Text { .. });
        let id = self.push(json!({"kind": "paste", "textOnly": text_only}))?;
        self.state.lock().unwrap_or_else(|e| e.into_inner()).paste = Some(Paste {
            id,
            intent: intent.into(),
            target,
            authorized: false,
        });
        Ok(())
    }

    fn copy_image(&self, width: u32, height: u32, rgba: &[u8]) -> Result<u64, String> {
        if self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .images
            .len()
            >= MAX_EVENTS
        {
            return Err("clipboard image acknowledgements are pending".into());
        }
        let size = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or("clipboard image dimensions overflow")?;
        if width == 0 || height == 0 || size != rgba.len() || size > MAX_IMAGE_BYTES {
            return Err("invalid or oversized clipboard image".into());
        }
        let image = photocraft_codecs::Image::from_u8(
            width,
            height,
            photocraft_codecs::ChannelLayout::Rgba,
            rgba.to_vec(),
        )
        .map_err(|e| e.to_string())?;
        let png =
            photocraft_codecs::encode(&image, photocraft_codecs::Format::Png, &Default::default())
                .map_err(|e| e.to_string())?;
        let path = self.cache.join(format!("{}.png", next_id()));
        photocraft_format::atomic_write(&path, &png).map_err(|e| e.to_string())?;
        let id = match self.push(json!({"kind": "copyImage", "path": path.to_string_lossy(), "width": width, "height": height})) {
            Ok(id) => id,
            Err(error) => { let _ = std::fs::remove_file(&path); return Err(error); }
        };
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .images
            .insert(id, path);
        Ok(id)
    }

    /// Convert output after the UI pass. AccessKit supplies the focused egui
    /// TextEdit's real surrounding text through public egui APIs.
    pub fn capture(
        &self,
        app: &mut PhotocraftApp,
        ctx: &egui::Context,
        editor: &mut Editor,
        output: &mut egui::FullOutput,
        density: f32,
    ) -> Result<(), String> {
        editor.restore(app, ctx);
        let mut ime = output.platform_output.ime.and_then(|ime| {
            let snapshot = snapshot(app, ctx, output, ime, density)?;
            (snapshot.text.len() <= MAX_TEXT).then_some(snapshot)
        });
        {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.cursor = output.platform_output.cursor_icon;
            if let Some(snapshot) = &mut ime {
                // Reacquiring the same widget still invalidates old client callbacks.
                snapshot.target = state
                    .ime
                    .as_ref()
                    .filter(|previous| previous.owner == snapshot.owner)
                    .map(|previous| previous.target.clone())
                    .unwrap_or_else(|| format!("{}@{}", snapshot.owner, next_id()));
                if snapshot.composition.is_none()
                    && editor
                        .composition
                        .as_ref()
                        .is_some_and(|original| original.target == snapshot.target)
                {
                    snapshot.composition = Some((
                        snapshot.anchor.min(snapshot.caret),
                        snapshot.anchor.abs_diff(snapshot.caret),
                    ));
                }
            }
            state.ime = ime;
        }
        for command in std::mem::take(&mut output.platform_output.commands) {
            match command {
                egui::OutputCommand::CopyText(text) => {
                    self.push(json!({"kind": "copyText", "text": text}))?;
                }
                egui::OutputCommand::CopyImage(image) => {
                    let bytes = image
                        .pixels
                        .iter()
                        .flat_map(|pixel| pixel.to_srgba_unmultiplied())
                        .collect::<Vec<_>>();
                    self.copy_image(image.size[0] as u32, image.size[1] as u32, &bytes)?;
                }
                egui::OutputCommand::OpenUrl(url) => {
                    self.push(json!({"kind": "openUrl", "url": url.url, "newTab": url.new_tab}))?;
                }
            }
        }
        let (close, cancel) = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|root| {
                (
                    root.commands
                        .iter()
                        .any(|command| matches!(command, egui::ViewportCommand::Close)),
                    root.commands
                        .iter()
                        .any(|command| matches!(command, egui::ViewportCommand::CancelClose)),
                )
            })
            .unwrap_or_default();
        if let Some(root) = output.viewport_output.get_mut(&egui::ViewportId::ROOT) {
            for command in std::mem::take(&mut root.commands) {
                match command {
                    egui::ViewportCommand::RequestCut => {
                        self.queue_editing_action(Event::Cut)?;
                        ctx.request_repaint();
                    }
                    egui::ViewportCommand::RequestCopy => {
                        self.queue_editing_action(Event::Copy)?;
                        ctx.request_repaint();
                    }
                    egui::ViewportCommand::RequestPaste => {
                        if let Some(snapshot) = self.current() {
                            self.request_paste("text", text_paste_target(snapshot))?;
                        } else {
                            self.request_paste(
                                "edit.paste",
                                PasteTarget::Document(
                                    app.session.active().map(|state| state.doc.id.0),
                                ),
                            )?;
                        }
                    }
                    egui::ViewportCommand::Fullscreen(enabled) => {
                        self.push(
                            json!({"kind":"viewport", "command":"fullscreen", "enabled":enabled}),
                        )?;
                    }
                    egui::ViewportCommand::Maximized(enabled) => {
                        self.push(
                            json!({"kind":"viewport", "command":"maximized", "enabled":enabled}),
                        )?;
                    }
                    egui::ViewportCommand::StartDrag => {
                        self.push(json!({"kind":"viewport", "command":"startDrag"}))?;
                    }
                    _ => {}
                }
            }
        }
        if close && !cancel {
            if app.allow_native_close() || app.session.documents().iter().all(|st| !st.is_dirty()) {
                self.push(json!({"kind": "close"}))?;
                if let Some(close) = &mut self.state.lock().unwrap_or_else(|e| e.into_inner()).close
                {
                    close.pending = false;
                }
            } else {
                // A viewport Close is a request, not authorization to lose work.
                photocraft_ui_egui::menus::invoke(app, ctx, "file.exit", Value::Null)?;
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        }
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(close) = &mut state.close
                && !app.native_close_pending()
            {
                close.pending = false;
            }
        }
        if let Some(error) = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .error
            .take()
        {
            app.ui.status = i18n::t(&error).to_string();
            app.ui.status_error = true;
        }
        Ok(())
    }

    fn queue_editing_action(&self, event: Event) -> Result<(), String> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.editing_actions.len() >= MAX_EVENTS {
            return Err("text editing action queue is full".into());
        }
        state.editing_actions.push(event);
        Ok(())
    }

    pub fn take_editing_actions(&self) -> Vec<Event> {
        std::mem::take(
            &mut self
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .editing_actions,
        )
    }

    pub fn key(
        &self,
        input: &mut InputState,
        app: &PhotocraftApp,
        ctx: &egui::Context,
        event: KeyInput<'_>,
    ) -> Result<(), String> {
        let KeyInput {
            code,
            pressed,
            ctrl,
            shift,
            alt,
            text,
        } = event;
        let key = match craft_ohos_platform::input::key_from_ohos(code) {
            Some(Key::C) => Some('c'),
            Some(Key::V) => Some('v'),
            Some(Key::X) => Some('x'),
            _ => None,
        };
        if ctrl && !alt && key.is_some() {
            if pressed {
                if key == Some('v') {
                    if let Some(snapshot) = self.current() {
                        self.request_paste("text", text_paste_target(snapshot))?;
                    } else {
                        self.request_paste(
                            if shift {
                                "edit.pasteSpecial.pasteInPlace"
                            } else {
                                "edit.paste"
                            },
                            PasteTarget::Document(app.session.active().map(|st| st.doc.id.0)),
                        )?;
                    }
                    return Ok(());
                }
                if ctx.text_edit_focused() || app.ui.text_edit.is_some() {
                    input.event(if key == Some('x') {
                        Event::Cut
                    } else {
                        Event::Copy
                    });
                    return Ok(());
                }
            } else if key == Some('v') {
                return Ok(());
            }
        }
        input.key(code, pressed, ctrl, shift, alt, text);
        Ok(())
    }

    pub fn apply(
        &self,
        packet: Input,
        app: &mut PhotocraftApp,
        ctx: &egui::Context,
        input: &mut InputState,
        editor: &mut Editor,
    ) -> Result<(), String> {
        let packet = packet.0;
        let kind = string(&packet, "kind")?;
        if kind == "printingUpdate" {
            self.printing_update(&packet, app)?;
            ctx.request_repaint();
            return Ok(());
        }
        if kind == "viewportState" {
            let fullscreen = packet["fullscreen"]
                .as_bool()
                .ok_or("fullscreen state must be boolean")?;
            let maximized = packet["maximized"]
                .as_bool()
                .ok_or("maximized state must be boolean")?;
            self.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .viewport = Some((fullscreen, maximized));
            ctx.request_repaint();
            return Ok(());
        }
        if kind == "clipboardAuthorize" {
            let id = number(&packet, "id")? as u64;
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let current = state.ime.clone();
            let paste = state
                .paste
                .as_mut()
                .filter(|paste| paste.id == id)
                .ok_or("clipboard paste is no longer pending")?;
            if let PasteTarget::Text { target, owner } = &paste.target
                && (target_token(app, ctx).as_ref() != Some(owner)
                    || current.is_some_and(|current| current.target != *target))
            {
                return Err("text paste target changed before authorization".into());
            }
            // A visible OS PasteButton temporarily detaches the native IME.
            // This authorizes only its existing request, never a new editor.
            paste.authorized = true;
            return Ok(());
        }
        if kind == "closeRequested" {
            let id = number(&packet, "id")? as u64;
            self.state.lock().unwrap_or_else(|e| e.into_inner()).close =
                Some(CloseState { id, pending: true });
            photocraft_ui_egui::menus::invoke(app, ctx, "file.exit", Value::Null)?;
            ctx.request_repaint();
            return Ok(());
        }
        if kind == "platformComplete" {
            let id = number(&packet, "id")? as u64;
            if let Some(path) = self
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .images
                .remove(&id)
            {
                let _ = std::fs::remove_file(path);
            }
            if packet["result"] == "error" {
                return Err(string(&packet, "error")?.into());
            }
            return Ok(());
        }
        if kind == "clipboard" {
            return self.complete_paste(&packet, app, ctx, input);
        }
        let target = string(&packet, "target")?;
        let Some(current) = self.current().filter(|current| {
            current.target == target
                && target_token(app, ctx).as_deref() == Some(current.owner.as_str())
        }) else {
            // Detach and focus changes legitimately leave old callback packets
            // in flight. They cannot edit a newly focused document/widget.
            if editor
                .composition
                .as_ref()
                .is_some_and(|original| original.target == target)
            {
                editor.composition = None;
            }
            return Ok(());
        };
        match kind {
            "imePreedit" => {
                let text = string(&packet, "text")?;
                if text.is_empty() {
                    editor.cancel(input);
                } else {
                    if editor
                        .composition
                        .as_ref()
                        .is_none_or(|composition| composition.target != target)
                    {
                        let start = current.anchor.min(current.caret);
                        editor.composition = Some(OriginalSelection {
                            target: target.into(),
                            owner: current.owner.clone(),
                            selected: current
                                .text
                                .chars()
                                .skip(start)
                                .take(current.anchor.abs_diff(current.caret))
                                .collect(),
                            anchor: current.anchor,
                            caret: current.caret,
                        });
                    }
                    input.event(Event::Ime(ImeEvent::Preedit {
                        text: text.into(),
                        active_range_chars: None,
                    }));
                }
            }
            "imeCommit" => {
                editor.composition = None;
                input.event(Event::Ime(ImeEvent::Commit(
                    string(&packet, "text")?.into(),
                )));
            }
            "imeCancel" => editor.cancel(input),
            "imeDelete" => {
                let (a, b) = current
                    .composition
                    .map(|(start, len)| (start, start + len))
                    .unwrap_or((
                        current.anchor.min(current.caret),
                        current.anchor.max(current.caret),
                    ));
                let before16 =
                    char_to_utf16(&current.text, a).saturating_sub(number(&packet, "before")?);
                let after16 =
                    char_to_utf16(&current.text, b).saturating_add(number(&packet, "after")?);
                let before = a - utf16_to_char(&current.text, before16, false);
                if let Some(original) = &mut editor.composition {
                    original.anchor = original.anchor.saturating_sub(before);
                    original.caret = original.caret.saturating_sub(before);
                }
                input.event(Event::Ime(ImeEvent::DeleteSurrounding {
                    before_chars: before,
                    after_chars: utf16_to_char(&current.text, after16, true).saturating_sub(b),
                }));
            }
            "imeSelection" => {
                set_selection(
                    app,
                    ctx,
                    &current.owner,
                    utf16_to_char(&current.text, number(&packet, "start")?, false),
                    utf16_to_char(&current.text, number(&packet, "end")?, true),
                )?;
            }
            "imeMove" => {
                let key = match string(&packet, "direction")? {
                    "left" => Key::ArrowLeft,
                    "right" => Key::ArrowRight,
                    "up" => Key::ArrowUp,
                    "down" => Key::ArrowDown,
                    _ => return Err("unknown IME movement".into()),
                };
                tap(input, key, egui::Modifiers::NONE);
            }
            "imeEnter" => tap(input, Key::Enter, egui::Modifiers::NONE),
            "imeAction" => match string(&packet, "action")? {
                "selectAll" => tap(input, Key::A, egui::Modifiers::COMMAND),
                "copy" => input.event(Event::Copy),
                "cut" => input.event(Event::Cut),
                "paste" => self.request_paste("text", text_paste_target(current))?,
                _ => return Err("unknown IME editing action".into()),
            },
            _ => return Err("unknown IME input".into()),
        }
        ctx.request_repaint();
        Ok(())
    }

    fn complete_paste(
        &self,
        packet: &Value,
        app: &mut PhotocraftApp,
        ctx: &egui::Context,
        input: &mut InputState,
    ) -> Result<(), String> {
        let image_path = packet["imagePath"].as_str().unwrap_or_default();
        // Retain the validated cache file's cleanup guard even for a stale completion.
        // A matching failed completion must release its request before path validation
        // returns an error, so the next explicit paste can retry.
        let owned = OwnedPaste::new(&self.cache, image_path);
        let id = number(packet, "id")? as u64;
        let pending = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !state.paste.as_ref().is_some_and(|paste| paste.id == id) {
                return Err("clipboard paste is no longer pending".into());
            }
            state.paste.take().ok_or("clipboard paste is missing")?
        };
        let _owned = owned?;
        match string(packet, "result")? {
            "cancel" => return Ok(()),
            "error" => return Err(string(packet, "error")?.into()),
            "success" => {}
            _ => return Err("unknown clipboard result".into()),
        }
        match pending.target {
            PasteTarget::Text { target, owner } => {
                let snapshot = self.current();
                let same_generation = snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.target == target);
                let authorization_blur = pending.authorized && snapshot.is_none();
                if target_token(app, ctx).as_deref() != Some(owner.as_str())
                    || !(same_generation || authorization_blur)
                {
                    return Err("text paste target changed".into());
                }
                let text = packet["text"].as_str().unwrap_or_default();
                if !text.is_empty() {
                    input.event(Event::Paste(text.into()));
                }
            }
            PasteTarget::Document(document) => {
                let active = app.session.active().map(|st| st.doc.id.0);
                if let Some(document) = document {
                    let index = app
                        .session
                        .documents()
                        .iter()
                        .position(|st| st.doc.id.0 == document)
                        .ok_or("image paste document was closed")?;
                    app.session.set_active(index);
                }
                let image = self.read_image(packet["imagePath"].as_str().unwrap_or_default());
                let result = match image {
                    Ok(image) => {
                        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        state.ready_image = image;
                        state.ready_paste = true;
                        drop(state);
                        photocraft_ui_egui::menus::invoke(app, ctx, &pending.intent, json!({}))
                            .map(|_| ())
                    }
                    Err(error) => Err(error),
                };
                if pending.intent != "file.newFromClipboard"
                    && let Some(index) = active.and_then(|id| {
                        app.session
                            .documents()
                            .iter()
                            .position(|st| st.doc.id.0 == id)
                    })
                {
                    app.session.set_active(index);
                }
                result?;
                app.sync_views();
            }
        }
        ctx.request_repaint();
        Ok(())
    }

    fn read_image(&self, path: &str) -> Result<Option<(u32, u32, Vec<u8>)>, String> {
        if path.is_empty() {
            return Ok(None);
        }
        let path = Path::new(path);
        if !path.is_absolute()
            || !path.starts_with(&self.cache)
            || path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err("clipboard image must be in cacheDir".into());
        }
        let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
        if size > MAX_IMAGE_BYTES as u64 {
            return Err("clipboard image exceeds 64 MiB".into());
        }
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let options = photocraft_codecs::DecodeOptions {
            limits: photocraft_codecs::Limits {
                max_width: 16384,
                max_height: 16384,
                max_pixels: (MAX_IMAGE_BYTES / 4) as u64,
                max_alloc: MAX_IMAGE_BYTES as u64,
            },
            ..Default::default()
        };
        let image = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            photocraft_codecs::decode_with(&bytes, &options)
        }))
        .map_err(|_| "clipboard decoder failed".to_string())?
        .map_err(|e| e.to_string())?;
        let size = (image.width() as usize)
            .checked_mul(image.height() as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or("clipboard dimensions overflow")?;
        if size > MAX_IMAGE_BYTES {
            return Err("decoded clipboard image exceeds 64 MiB".into());
        }
        Ok(Some((image.width(), image.height(), image.to_rgba8())))
    }
}

fn text_paste_target(snapshot: TextSnapshot) -> PasteTarget {
    PasteTarget::Text {
        target: snapshot.target,
        owner: snapshot.owner,
    }
}

impl Editor {
    fn cancel(&mut self, input: &mut InputState) {
        if let Some(original) = self.composition.take() {
            input.event(Event::Ime(ImeEvent::Commit(original.selected)));
            self.restore_selection = Some((original.owner, original.anchor, original.caret));
        }
    }

    fn restore(&mut self, app: &mut PhotocraftApp, ctx: &egui::Context) {
        if let Some((target, anchor, caret)) = self.restore_selection.take() {
            let _ = set_selection(app, ctx, &target, anchor, caret);
            ctx.request_repaint();
        }
    }
}

fn snapshot(
    app: &PhotocraftApp,
    ctx: &egui::Context,
    output: &egui::FullOutput,
    ime: egui::output::IMEOutput,
    density: f32,
) -> Option<TextSnapshot> {
    let target = target_token(app, ctx)?;
    let (text, anchor, caret, composition) = if !ctx.text_edit_focused()
        && let Some(edit) = &app.ui.text_edit
    {
        (
            photocraft_ui_egui::type_tool::editing_text(app)?,
            edit.anchor,
            edit.caret,
            edit.preedit,
        )
    } else {
        let id = ctx.memory(|memory| memory.focused())?;
        let tree = output.platform_output.accesskit_update.as_ref()?;
        let nodes: HashMap<_, _> = tree.nodes.iter().map(|(id, node)| (*id, node)).collect();
        let parent = nodes.get(&id.accesskit_id())?;
        let text = parent
            .children()
            .iter()
            .filter_map(|id| nodes.get(id).and_then(|node| node.value()))
            .collect::<String>();
        let range = egui::TextEdit::load_state(ctx, id)?.cursor.char_range()?;
        (text, range.secondary.index.0, range.primary.index.0, None)
    };
    Some(TextSnapshot {
        owner: target.clone(),
        target,
        text,
        anchor,
        caret,
        composition,
        output: ime,
        density,
        zoom: ctx.zoom_factor(),
    })
}

fn target_token(app: &PhotocraftApp, ctx: &egui::Context) -> Option<String> {
    if ctx.text_edit_focused() {
        ctx.memory(|memory| memory.focused())
            .map(|id| format!("egui:{}", id.value()))
    } else if let Some(edit) = &app.ui.text_edit {
        Some(format!(
            "doc:{}:{}:{}",
            app.session.active()?.doc.id.0,
            edit.layer,
            edit.session
        ))
    } else {
        None
    }
}

fn set_selection(
    app: &mut PhotocraftApp,
    ctx: &egui::Context,
    target: &str,
    anchor: usize,
    caret: usize,
) -> Result<(), String> {
    if target_token(app, ctx).as_deref() != Some(target) {
        return Err("text selection target changed".into());
    }
    if target.starts_with("doc:")
        && let Some(edit) = &mut app.ui.text_edit
    {
        edit.anchor = anchor;
        edit.caret = caret;
    } else {
        let id = ctx
            .memory(|memory| memory.focused())
            .ok_or("no focused text widget")?;
        let mut state = egui::TextEdit::load_state(ctx, id).ok_or("text widget state missing")?;
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(anchor),
                egui::text::CCursor::new(caret),
            )));
        state.store(ctx, id);
    }
    ctx.request_repaint();
    Ok(())
}

fn tap(input: &mut InputState, key: Key, modifiers: egui::Modifiers) {
    input.event(key_event(key, modifiers));
    input.event(Event::Key {
        key,
        physical_key: None,
        pressed: false,
        repeat: false,
        modifiers,
    });
}

fn key_event(key: Key, modifiers: egui::Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

pub fn char_to_utf16(text: &str, character: usize) -> usize {
    text.chars().take(character).map(char::len_utf16).sum()
}

/// UTF-16 offsets inside a surrogate pair snap outwards for replacement ranges.
pub fn utf16_to_char(text: &str, units: usize, round_up: bool) -> usize {
    let mut offset = 0;
    for (index, character) in text.chars().enumerate() {
        if offset == units {
            return index;
        }
        offset += character.len_utf16();
        if offset > units {
            return index + usize::from(round_up);
        }
    }
    text.chars().count()
}

fn string<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value[field]
        .as_str()
        .ok_or_else(|| format!("missing platform {field}"))
}
fn number(value: &Value, field: &str) -> Result<usize, String> {
    value[field]
        .as_u64()
        .filter(|n| *n <= 9_007_199_254_740_991)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| format!("invalid platform {field}"))
}
pub(crate) fn next_id() -> u64 {
    static IDS: OnceLock<AtomicU64> = OnceLock::new();
    IDS.get_or_init(|| {
        AtomicU64::new(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|time| time.as_millis() as u64)
                .unwrap_or(1),
        )
    })
    .fetch_add(1, Ordering::Relaxed)
}

// Ownership transfers only after a packet was enqueued. Every worker exit path
// removes a validated paste PNG, including stale targets and decode failures.
struct OwnedPaste(Option<PathBuf>);
impl OwnedPaste {
    fn new(cache: &Path, path: &str) -> Result<Self, String> {
        if path.is_empty() {
            return Ok(Self(None));
        }
        if path.len() > MAX_IMAGE_PATH_BYTES {
            return Err("clipboard image path exceeds 4096 bytes".into());
        }
        let path = PathBuf::from(path);
        if !path.is_absolute()
            || !path.starts_with(cache)
            || path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err("clipboard image must be in cacheDir/Clipboard".into());
        }
        // A symlink to a retained document is not owned clipboard cache data.
        let metadata = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("clipboard image must be a cache file".into());
        }
        let canonical = path.canonicalize().map_err(|e| e.to_string())?;
        if !canonical.starts_with(cache.canonicalize().map_err(|e| e.to_string())?) {
            return Err("clipboard image escapes its cache".into());
        }
        Ok(Self(Some(path)))
    }
}
impl Drop for OwnedPaste {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::App;

    struct Fixture {
        platform: Platform,
        app: PhotocraftApp,
        ctx: egui::Context,
        input: InputState,
        editor: Editor,
        root: PathBuf,
        time: f64,
    }
    impl Fixture {
        fn new() -> Result<Self, String> {
            let root = std::env::temp_dir().join(format!(
                "photocraft-platform-{}-{}",
                std::process::id(),
                next_id()
            ));
            let platform = Platform::new(&root)?;
            let mut services = crate::services::services(&root);
            platform.attach_services(&mut services);
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            PhotocraftApp::setup_context(&ctx, photocraft_ui_egui::theme::ThemeKind::Pro);
            Ok(Self {
                platform,
                app: PhotocraftApp::new(photocraft_engine::Session::new(), services),
                ctx,
                input: InputState::default(),
                editor: Editor::default(),
                root,
                time: 0.0,
            })
        }
        fn apply(&mut self, value: Value) -> Result<(), String> {
            self.platform.apply(
                Input::parse(&value.to_string())?,
                &mut self.app,
                &self.ctx,
                &mut self.input,
                &mut self.editor,
            )
        }
        fn text_tick(&mut self, text: &mut String) -> Result<(), String> {
            self.text_tick_with_ime(text, true)
        }
        fn text_tick_with_ime(
            &mut self,
            text: &mut String,
            report_ime: bool,
        ) -> Result<(), String> {
            self.time += 0.01;
            for event in self.platform.take_editing_actions() {
                self.input.event(event);
            }
            let mut raw = self.input.take([1280, 800], 1.0, self.time);
            self.platform.prepare_input(&mut raw);
            let mut output = self.ctx.run_ui(raw, |ui| {
                let response =
                    ui.add(egui::TextEdit::multiline(text).id(egui::Id::new("platform-text")));
                response.request_focus();
                if !report_ime {
                    ui.ctx().output_mut(|output| output.ime = None);
                }
            });
            let result =
                self.platform
                    .capture(&mut self.app, &self.ctx, &mut self.editor, &mut output, 1.0);
            output.drop_without_applying_deltas();
            result
        }
        fn app_tick(&mut self) -> Result<(), String> {
            self.time += 0.01;
            let mut raw = self.input.take([1280, 800], 1.0, self.time);
            self.platform.prepare_input(&mut raw);
            self.app.raw_input_hook(&self.ctx, &mut raw);
            let mut frame = eframe::Frame::_new_kittest();
            let mut logic_done = false;
            let mut output = self.ctx.run_ui(raw, |ui| {
                if !logic_done {
                    self.app.logic(ui.ctx(), &mut frame);
                    logic_done = true;
                }
                self.app.ui(ui, &mut frame);
            });
            let result =
                self.platform
                    .capture(&mut self.app, &self.ctx, &mut self.editor, &mut output, 1.0);
            output.drop_without_applying_deltas();
            result
        }
        fn output(&self) -> Result<Value, String> {
            serde_json::from_str(&self.platform.take_json()).map_err(|error| error.to_string())
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn real_text_edit_chinese_composition_cancel_selection_and_surrogate_deletion()
    -> Result<(), String> {
        let mut fixture = Fixture::new()?;
        let mut text = "甲😀乙".to_string();
        fixture.text_tick(&mut text)?;
        fixture.text_tick(&mut text)?;
        let target = fixture
            .platform
            .current()
            .ok_or("missing text snapshot")?
            .target;
        fixture.apply(json!({"kind":"imeSelection","target":target,"start":1,"end":3}))?;
        fixture.text_tick(&mut text)?;
        let snapshot = fixture.output()?;
        assert_eq!(snapshot["ime"]["text"], text);
        assert_eq!(snapshot["ime"]["selectionStart"], 1);
        assert_eq!(snapshot["ime"]["selectionEnd"], 3);
        fixture.apply(json!({"kind":"imePreedit","target":target,"text":"中文"}))?;
        fixture.text_tick(&mut text)?;
        assert_eq!(text, "甲中文乙");
        fixture.apply(json!({"kind":"imeCancel","target":target}))?;
        fixture.text_tick(&mut text)?;
        assert_eq!(text, "甲😀乙");
        let snapshot = fixture.output()?;
        assert_eq!(snapshot["ime"]["selectionStart"], 1);
        assert_eq!(snapshot["ime"]["selectionEnd"], 3);
        fixture.apply(json!({"kind":"imePreedit","target":target,"text":"候"}))?;
        fixture.text_tick(&mut text)?;
        fixture.apply(json!({"kind":"imeCommit","target":target,"text":"中文"}))?;
        fixture.text_tick(&mut text)?;
        assert_eq!(text, "甲中文乙");
        fixture.apply(json!({"kind":"imeSelection","target":target,"start":1,"end":3}))?;
        fixture.text_tick(&mut text)?;
        fixture.apply(json!({"kind":"imeDelete","target":target,"before":1,"after":1}))?;
        fixture.text_tick(&mut text)?;
        assert_eq!(text, "中文");
        assert_eq!(char_to_utf16("a😀b", 2), 3);
        assert_eq!(utf16_to_char("a😀b", 2, false), 1);
        assert_eq!(utf16_to_char("a😀b", 2, true), 2);
        Ok(())
    }

    #[test]
    fn text_clipboard_only_reads_on_paste_and_does_not_accept_a_changed_target()
    -> Result<(), String> {
        let mut fixture = Fixture::new()?;
        let mut text = "中文".to_string();
        fixture.text_tick(&mut text)?;
        fixture.text_tick(&mut text)?;
        assert_eq!(fixture.output()?["events"], json!([]));
        let target = fixture.platform.current().ok_or("missing snapshot")?.target;
        fixture.apply(json!({"kind":"imeSelection","target":target,"start":0,"end":2}))?;
        fixture.text_tick(&mut text)?;
        fixture.apply(json!({"kind":"imeAction","target":target,"action":"copy"}))?;
        fixture.text_tick(&mut text)?;
        let copied = fixture.output()?;
        assert_eq!(copied["events"][0]["kind"], "copyText");
        assert_eq!(copied["events"][0]["text"], "中文");
        fixture
            .ctx
            .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
        fixture.text_tick(&mut text)?;
        let request = fixture.output()?;
        assert_eq!(request["events"][0]["textOnly"], true);
        fixture.apply(json!({"kind":"clipboard","id":request["events"][0]["id"],"result":"success","text":"粘贴😀","imagePath":"","error":""}))?;
        fixture.text_tick(&mut text)?;
        assert_eq!(text, "粘贴😀");
        fixture.apply(json!({"kind":"imeAction","target":target,"action":"paste"}))?;
        let changed_paste = fixture.output()?["events"][0].clone();
        fixture
            .ctx
            .memory_mut(|memory| memory.surrender_focus(egui::Id::new("platform-text")));
        assert!(fixture.apply(json!({"kind":"clipboard","id":changed_paste["id"],"result":"success","text":"wrong target","imagePath":"","error":""})).is_err());
        fixture.apply(json!({"kind":"imeCommit","target":target,"text":"stale"}))?;
        fixture.text_tick(&mut text)?;
        fixture.text_tick(&mut text)?;
        assert_ne!(
            fixture
                .platform
                .current()
                .ok_or("new focused snapshot missing")?
                .target,
            target
        );
        fixture.apply(json!({"kind":"imeCommit","target":target,"text":"old client"}))?;
        fixture.text_tick(&mut text)?;
        assert_eq!(text, "粘贴😀");
        Ok(())
    }

    #[test]
    fn visible_paste_authorization_preserves_the_original_utf16_selection_during_ime_detach()
    -> Result<(), String> {
        let mut fixture = Fixture::new()?;
        let mut text = "甲😀乙".to_string();
        fixture.text_tick(&mut text)?;
        fixture.text_tick(&mut text)?;
        let target = fixture.platform.current().ok_or("missing snapshot")?.target;
        fixture.apply(json!({"kind":"imeSelection","target":target,"start":1,"end":3}))?;
        fixture.text_tick(&mut text)?;
        fixture.apply(json!({"kind":"imePreedit","target":target,"text":"候"}))?;
        fixture.text_tick(&mut text)?;
        fixture.apply(json!({"kind":"imeAction","target":target,"action":"paste"}))?;
        let request = fixture.output()?["events"][0].clone();
        fixture.apply(json!({"kind":"clipboardAuthorize","id":request["id"]}))?;
        fixture.apply(json!({"kind":"imeCancel","target":target}))?;
        fixture.text_tick_with_ime(&mut text, false)?;
        assert_eq!(text, "甲😀乙");
        assert!(fixture.platform.current().is_none());
        fixture.apply(json!({"kind":"clipboard","id":request["id"],"result":"success","text":"中文","imagePath":"","error":""}))?;
        fixture.text_tick(&mut text)?;
        assert_eq!(text, "甲中文乙");

        let target = fixture
            .platform
            .current()
            .ok_or("missing new snapshot")?
            .target;
        fixture.apply(json!({"kind":"imeAction","target":target,"action":"paste"}))?;
        let cancelled = fixture.output()?["events"][0].clone();
        fixture.apply(json!({"kind":"clipboardAuthorize","id":cancelled["id"]}))?;
        fixture.apply(json!({"kind":"clipboard","id":cancelled["id"],"result":"cancel","text":"","imagePath":"","error":""}))?;
        fixture.text_tick(&mut text)?;
        assert_eq!(text, "甲中文乙");
        assert!(fixture.apply(json!({"kind":"clipboard","id":cancelled["id"],"result":"success","text":"late","imagePath":"","error":""})).is_err());
        Ok(())
    }

    #[test]
    fn paste_authorization_cannot_resurrect_a_changed_editor_generation() -> Result<(), String> {
        let mut fixture = Fixture::new()?;
        let mut text = "中文".to_string();
        fixture.text_tick(&mut text)?;
        fixture.text_tick(&mut text)?;
        let target = fixture.platform.current().ok_or("missing snapshot")?.target;
        fixture.apply(json!({"kind":"imeAction","target":target,"action":"paste"}))?;
        let request = fixture.output()?["events"][0].clone();
        assert!(
            fixture
                .apply(json!({"kind":"clipboardAuthorize","id":0}))
                .is_err()
        );
        fixture.apply(json!({"kind":"clipboardAuthorize","id":request["id"]}))?;
        fixture
            .ctx
            .memory_mut(|memory| memory.surrender_focus(egui::Id::new("platform-text")));
        fixture.text_tick(&mut text)?;
        fixture.text_tick(&mut text)?;
        assert_ne!(
            fixture
                .platform
                .current()
                .ok_or("missing reacquired snapshot")?
                .target,
            target
        );
        assert!(fixture.apply(json!({"kind":"clipboard","id":request["id"],"result":"success","text":"wrong editor","imagePath":"","error":""})).is_err());
        fixture.text_tick(&mut text)?;
        assert_eq!(text, "中文");
        Ok(())
    }

    #[test]
    fn cursor_and_window_commands_keep_actual_viewport_state_in_the_ui_input() -> Result<(), String>
    {
        let mut fixture = Fixture::new()?;
        fixture.apply(json!({"kind":"viewportState","fullscreen":false,"maximized":true}))?;
        assert!(
            fixture
                .apply(json!({"kind":"viewportState","fullscreen":0,"maximized":true}))
                .is_err()
        );
        let mut raw = fixture.input.take([1280, 800], 1.0, 0.01);
        fixture.platform.prepare_input(&mut raw);
        let mut output = fixture.ctx.run_ui(raw, |ui| {
            assert_eq!(
                ui.ctx().input(|input| input.viewport().maximized),
                Some(true)
            );
            assert_eq!(
                ui.ctx().input(|input| input.viewport().fullscreen),
                Some(false)
            );
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Maximized(false));
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        });
        fixture.platform.capture(
            &mut fixture.app,
            &fixture.ctx,
            &mut fixture.editor,
            &mut output,
            1.0,
        )?;
        output.drop_without_applying_deltas();
        let output = fixture.output()?;
        assert_eq!(output["cursorIcon"], "Crosshair");
        let events = output["events"].as_array().ok_or("missing events")?;
        assert_eq!(events.len(), 3);
        for event in events {
            assert!(
                event["id"]
                    .as_u64()
                    .is_some_and(|id| id <= 9_007_199_254_740_991)
            );
        }
        assert_eq!(events[0]["command"], "fullscreen");
        assert_eq!(events[0]["enabled"], true);
        assert_eq!(events[1]["command"], "maximized");
        assert_eq!(events[1]["enabled"], false);
        assert_eq!(events[2]["command"], "startDrag");
        assert_eq!(fixture.output()?["cursorIcon"], "Crosshair");
        Ok(())
    }

    #[test]
    fn real_document_image_copy_and_paste_keep_target_and_cleanup_owned_pngs() -> Result<(), String>
    {
        let mut fixture = Fixture::new()?;
        let bytes = fixture
            .app
            .services
            .encode_png
            .as_ref()
            .ok_or("encoder missing")?(1, 1, &[255, 0, 0, 255])?;
        fixture.app.open_bytes("source.png", &bytes)?;
        fixture.app.run("select.all", json!({}))?;
        fixture.app.run("edit.copy", json!({}))?;
        let output = fixture.output()?;
        let request = &output["events"][0];
        assert_eq!(request["kind"], "copyImage");
        let path = request["path"].as_str().ok_or("copy path missing")?;
        assert_eq!(
            photocraft_codecs::decode(&std::fs::read(path).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?
                .to_rgba8(),
            [255, 0, 0, 255]
        );
        fixture.apply(
            json!({"kind":"platformComplete","id":request["id"],"result":"success","error":""}),
        )?;
        assert!(!Path::new(path).exists());
        photocraft_ui_egui::menus::invoke(&mut fixture.app, &fixture.ctx, "edit.paste", json!({}))?;
        let target = fixture
            .app
            .session
            .active()
            .ok_or("document missing")?
            .doc
            .id;
        let count = fixture
            .app
            .session
            .active()
            .ok_or("document missing")?
            .doc
            .layers
            .len();
        let request = fixture.output()?["events"][0].clone();
        assert_eq!(request["kind"], "paste");
        assert_eq!(request["textOnly"], false);
        fixture
            .app
            .session
            .execute("file.new", json!({"name":"other","width":2,"height":2}))
            .map_err(|error| error.to_string())?;
        let other = fixture
            .app
            .session
            .active()
            .ok_or("other document missing")?
            .doc
            .id;
        let paste = fixture.platform.cache.join("incoming.png");
        std::fs::write(&paste, &bytes).map_err(|error| error.to_string())?;
        fixture.apply(json!({"kind":"clipboard","id":request["id"],"result":"success","imagePath":paste.to_string_lossy(),"text":"","error":""}))?;
        assert!(!paste.exists());
        assert_eq!(
            fixture.app.session.active().ok_or("other missing")?.doc.id,
            other
        );
        assert_eq!(
            fixture
                .app
                .session
                .documents()
                .iter()
                .find(|state| state.doc.id == target)
                .ok_or("target missing")?
                .doc
                .layers
                .len(),
            count + 1
        );
        photocraft_ui_egui::menus::invoke(&mut fixture.app, &fixture.ctx, "edit.paste", json!({}))?;
        let request = fixture.output()?["events"][0].clone();
        let invalid = fixture.platform.cache.join("invalid.png");
        std::fs::write(&invalid, b"invalid image").map_err(|error| error.to_string())?;
        assert!(fixture.apply(json!({"kind":"clipboard","id":request["id"],"result":"success","imagePath":invalid.to_string_lossy(),"text":"","error":""})).is_err());
        assert!(!invalid.exists());
        Ok(())
    }

    #[test]
    fn rejected_clipboard_image_completion_allows_retry_without_consuming_a_new_request()
    -> Result<(), String> {
        let mut fixture = Fixture::new()?;
        let bytes = fixture
            .app
            .services
            .encode_png
            .as_ref()
            .ok_or("encoder missing")?(1, 1, &[0, 255, 0, 255])?;
        fixture.app.open_bytes("first.png", &bytes)?;
        fixture
            .app
            .session
            .execute("file.new", json!({"width":2,"height":2}))
            .map_err(|error| error.to_string())?;
        fixture.app.sync_views();
        let documents = fixture
            .app
            .session
            .documents()
            .iter()
            .map(|state| (state.doc.id, state.doc.layers.len()))
            .collect::<Vec<_>>();
        fixture.app.session.set_active(0);
        photocraft_ui_egui::menus::invoke(&mut fixture.app, &fixture.ctx, "edit.paste", json!({}))?;
        let rejected = fixture.output()?["events"][0].clone();
        fixture.app.session.set_active(1);
        let foreign = fixture.root.join("retained.png");
        std::fs::write(&foreign, &bytes).map_err(|error| error.to_string())?;
        let failure = fixture.apply(json!({"kind":"clipboard","id":rejected["id"],
            "result":"success","imagePath":foreign.to_string_lossy(),"text":"","error":""}));
        assert_eq!(
            failure,
            Err("clipboard image must be in cacheDir/Clipboard".into())
        );
        assert!(foreign.exists(), "a rejected foreign path is never deleted");
        assert_eq!(
            fixture.app.session.active().ok_or("active missing")?.doc.id,
            documents[1].0
        );
        assert_eq!(
            fixture
                .app
                .session
                .documents()
                .iter()
                .map(|state| (state.doc.id, state.doc.layers.len()))
                .collect::<Vec<_>>(),
            documents
        );

        for invalid_path in [
            "x".repeat(MAX_IMAGE_PATH_BYTES + 1),
            fixture
                .platform
                .cache
                .join("missing.png")
                .to_string_lossy()
                .into_owned(),
        ] {
            photocraft_ui_egui::menus::invoke(
                &mut fixture.app,
                &fixture.ctx,
                "edit.paste",
                json!({}),
            )?;
            let request = fixture.output()?["events"][0].clone();
            assert!(
                fixture
                    .apply(json!({"kind":"clipboard","id":request["id"],
                "result":"success","imagePath":invalid_path,"text":"","error":""}))
                    .is_err()
            );
        }

        photocraft_ui_egui::menus::invoke(&mut fixture.app, &fixture.ctx, "edit.paste", json!({}))?;
        let retry = fixture.output()?["events"][0].clone();
        assert_ne!(retry["id"], rejected["id"]);
        fixture.app.session.set_active(0);
        let stale_image = fixture.platform.cache.join("stale.png");
        std::fs::write(&stale_image, &bytes).map_err(|error| error.to_string())?;
        assert_eq!(
            fixture.apply(json!({"kind":"clipboard","id":rejected["id"],
            "result":"success","imagePath":stale_image.to_string_lossy(),"text":"","error":""})),
            Err("clipboard paste is no longer pending".into())
        );
        assert!(
            !stale_image.exists(),
            "stale owned PNGs still transfer cleanup responsibility"
        );
        let image = fixture.platform.cache.join("retry.png");
        std::fs::write(&image, &bytes).map_err(|error| error.to_string())?;
        fixture.apply(
            json!({"kind":"clipboard","id":retry["id"],"result":"success",
            "imagePath":image.to_string_lossy(),"text":"","error":""}),
        )?;
        assert!(!image.exists());
        assert_eq!(
            fixture.app.session.active().ok_or("active missing")?.doc.id,
            documents[0].0
        );
        assert_eq!(
            fixture.app.session.documents()[0].doc.layers.len(),
            documents[0].1
        );
        assert_eq!(
            fixture.app.session.documents()[1].doc.layers.len(),
            documents[1].1 + 1
        );
        Ok(())
    }

    #[test]
    fn output_and_owned_image_queues_are_bounded() -> Result<(), String> {
        let fixture = Fixture::new()?;
        for _ in 0..MAX_EVENTS {
            fixture
                .platform
                .push(json!({"kind":"copyText","text":"x"}))?;
        }
        assert!(
            fixture
                .platform
                .push(json!({"kind":"copyText","text":"overflow"}))
                .is_err()
        );
        assert_eq!(
            fixture.output()?["events"]
                .as_array()
                .ok_or("events missing")?
                .len(),
            MAX_EVENTS
        );
        assert!(Input::parse(&"x".repeat(MAX_TEXT + 1)).is_err());
        Ok(())
    }

    #[test]
    fn title_bar_close_ack_tracks_original_unsaved_prompt_and_clean_exit() -> Result<(), String> {
        let mut fixture = Fixture::new()?;
        fixture
            .app
            .session
            .execute("file.new", json!({"width":2,"height":2}))
            .map_err(|error| error.to_string())?;
        fixture
            .app
            .session
            .execute("layer.new.layer", json!({"name":"unsaved"}))
            .map_err(|error| error.to_string())?;
        fixture.app.sync_views();
        assert!(
            fixture
                .app
                .session
                .active()
                .ok_or("missing doc")?
                .is_dirty()
        );
        fixture.apply(json!({"kind":"closeRequested","id":987}))?;
        fixture.app_tick()?;
        let output = fixture.output()?;
        assert_eq!(output["closeState"], json!({"id":987,"pending":true}));
        assert_eq!(output["events"], json!([]));
        assert!(fixture.app.native_close_pending());
        assert_eq!(fixture.app.session.documents().len(), 1);
        // Escape is the original modal's Cancel action.
        fixture.app_tick()?;
        fixture
            .input
            .event(key_event(Key::Escape, egui::Modifiers::NONE));
        fixture.app_tick()?;
        assert!(!fixture.app.native_close_pending());
        assert_eq!(
            fixture.output()?["closeState"],
            json!({"id":987,"pending":false})
        );
        assert_eq!(fixture.app.session.documents().len(), 1);
        fixture
            .app
            .session
            .active_mut()
            .ok_or("missing doc")?
            .saved_revision = fixture.app.session.active().ok_or("missing doc")?.revision;
        fixture.apply(json!({"kind":"closeRequested","id":988}))?;
        fixture.app_tick()?;
        let output = fixture.output()?;
        assert_eq!(output["closeState"], json!({"id":988,"pending":false}));
        assert_eq!(output["events"][0]["kind"], "close");
        Ok(())
    }
}
