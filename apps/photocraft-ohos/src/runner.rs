use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use craft_ohos_platform::{input::InputState, surface::GpuSurface};
use eframe::App;
use photocraft_engine::Session;
use photocraft_ui_egui::{PhotocraftApp, theme::ThemeKind};

use crate::file_requests::FileRequests;
use crate::folder_requests::FolderRequests;
use crate::platform::{Editor, Platform};
use crate::processor::Processor;
use crate::{Command, fonts, services};

pub struct Runner {
    app: PhotocraftApp,
    ctx: egui::Context,
    frame: eframe::Frame,
    input: InputState,
    surface: Option<GpuSurface>,
    render_state: Option<eframe::egui_wgpu::RenderState>,
    repaint: Arc<Mutex<Option<Instant>>>,
    next_repaint: Instant,
    density: f32,
    frames: u64,
    ui_presented: bool,
    files: PathBuf,
    cache: PathBuf,
    file_requests: FileRequests,
    folder_requests: FolderRequests,
    processor: Processor,
    recent: crate::recent::RecentSources,
    platform: Platform,
    editor: Editor,
    pending_output: Option<egui::FullOutput>,
    ui_size: [u32; 2],
    #[cfg(feature = "device-tests")]
    device_tests: Option<crate::device_tests::DeviceSession>,
}

impl Runner {
    pub fn new(files: &str, cache: &str, density: f32) -> Result<Self, String> {
        let (files, cache) = (PathBuf::from(files), PathBuf::from(cache));
        if !files.is_absolute() || !cache.is_absolute() {
            return Err("UIAbility sandbox paths must be absolute".into());
        }
        std::fs::create_dir_all(&files).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let repaint: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(Some(Instant::now())));
        let signal = repaint.clone();
        ctx.set_request_repaint_callback(move |info| {
            if info.viewport_id == egui::ViewportId::ROOT {
                let when = Instant::now()
                    .checked_add(info.delay)
                    .unwrap_or_else(|| Instant::now() + Duration::from_secs(86400));
                let mut pending = signal.lock().unwrap_or_else(|e| e.into_inner());
                *pending = Some(pending.map_or(when, |previous| previous.min(when)));
            }
        });
        fonts::install(&ctx);
        fonts::install_document_fonts();
        PhotocraftApp::setup_context(&ctx, ThemeKind::Pro);
        let file_requests = FileRequests::new(&files)?;
        let platform = Platform::new(&cache)?;
        platform.install_global();
        let mut hooks = services::services_with_files(&files, &file_requests);
        let recent = crate::recent::RecentSources::new(&files);
        recent.attach(&mut hooks, &file_requests);
        platform.attach_services(&mut hooks);
        let folder_requests = FolderRequests::new(&files, &platform, &file_requests)?;
        folder_requests.attach(&mut hooks);
        let mut session = Session::new();
        session.automation.enabled = true;
        file_requests.attach_sources(&mut session, &ctx);
        let processor = Processor::new(&files, &platform, &file_requests)?;
        processor.attach(&mut hooks, &mut session);
        crate::printing::attach(&mut hooks, &mut session, &files, &file_requests, &platform)?;
        let mut app = PhotocraftApp::new(session, hooks);
        photocraft_ui_egui::i18n::set_current(photocraft_ui_egui::i18n::Lang::from_pref(
            &app.session.prefs().interface.language,
        ));
        if let Err(error) = recent.refresh(&mut app) {
            app.ui.status = error;
            app.ui.status_error = true;
        }
        #[cfg(feature = "device-tests")]
        let device_tests = match crate::device_tests::DeviceSession::new(&files, &cache)? {
            Some((session, receiver)) => {
                app = app.with_control(receiver);
                Some(session)
            }
            None => None,
        };
        let runner = Self {
            app,
            ctx,
            frame: eframe::Frame::_new_kittest(),
            input: InputState::default(),
            surface: None,
            render_state: None,
            repaint,
            next_repaint: Instant::now(),
            density,
            frames: 0,
            ui_presented: false,
            files,
            cache,
            file_requests,
            folder_requests,
            processor,
            recent,
            platform,
            editor: Editor::default(),
            pending_output: None,
            ui_size: [1280, 800],
            #[cfg(feature = "device-tests")]
            device_tests,
        };
        #[cfg(feature = "device-tests")]
        {
            let mut runner = runner;
            runner.capture_test_snapshot();
            Ok(runner)
        }
        #[cfg(not(feature = "device-tests"))]
        Ok(runner)
    }

    pub fn handle(&mut self, command: Command) -> Result<(), String> {
        match command {
            Command::SystemLanguage(tag) => {
                photocraft_ui_egui::i18n::set_system_language(&tag);
                photocraft_ui_egui::i18n::set_current(photocraft_ui_egui::i18n::Lang::from_pref(
                    &self.app.session.prefs().interface.language,
                ));
                self.ctx.request_repaint();
            }
            Command::Density(density) => {
                self.density = density;
                self.ctx.request_repaint();
            }
            Command::Surface(address, width, height) => {
                // Drop the previous surface before using the new native window.
                self.surface = None;
                let window = NonNull::new(address as *mut c_void).ok_or("null native window")?;
                // SAFETY: A synchronous bridge RPC retains window until Destroy has completed.
                let surface =
                    unsafe { GpuSurface::new(window, width, height, self.render_state.clone())? };
                crate::logger::info(&format!(
                    "GPU adapter: {:?}; {}",
                    surface.state.adapter.get_info().backend,
                    surface.state.adapter.get_info().name
                ));
                // Keep document compositing on the upstream CPU path for this first spike.
                // GLES still renders all egui widgets/textures. GPU compositor verification is
                // a separate gate because its compute shaders require additional capabilities.
                self.render_state = Some(surface.state.clone());
                self.surface = Some(surface);
                if width > 0 && height > 0 {
                    self.ui_size = [width, height];
                }
                self.ui_presented = false;
                self.next_repaint = Instant::now();
                self.ctx.request_repaint();
                self.record()?;
            }
            Command::Resize(width, height) => {
                if width > 0 && height > 0 {
                    self.ui_size = [width, height];
                }
                if let Some(surface) = &mut self.surface {
                    surface.resize(width, height)?;
                }
                self.ctx.request_repaint();
            }
            Command::Destroy => {
                self.surface = None;
            }
            Command::Frame(timestamp) => {
                self.paint(timestamp)?;
            }
            Command::Pointer(x, y, action, button, pressure, tilt_x, tilt_y) => {
                #[cfg(feature = "device-tests")]
                if let Some(session) = &mut self.device_tests {
                    session.native_pointer();
                }
                self.input.pointer(x, y, action, button);
                // Native bridge uses negative pressure for a mouse, nonnegative for a stylus.
                if pressure >= 0.0 && action != 3 && action != 4 {
                    self.app
                        .stylus
                        .feed
                        .set(Some(photocraft_ui_egui::stylus::PenSample {
                            pressure,
                            tilt_x,
                            tilt_y,
                            rotation: 0.0,
                            eraser: false,
                        }));
                } else {
                    self.app.stylus.feed.set(None);
                }
                self.ctx.request_repaint();
            }
            Command::Key(code, pressed, ctrl, shift, alt, text) => {
                #[cfg(feature = "device-tests")]
                if let Some(session) = &mut self.device_tests {
                    session.native_key(code, &text);
                }
                self.platform.key(
                    &mut self.input,
                    &self.app,
                    &self.ctx,
                    crate::platform::KeyInput {
                        code,
                        pressed,
                        ctrl,
                        shift,
                        alt,
                        text: &text,
                    },
                )?;
                self.ctx.request_repaint();
            }
            Command::Zoom(factor) => {
                self.input.zoom(factor);
                self.ctx.request_repaint();
            }
            Command::Scroll(dx, dy) => {
                self.input.scroll(dx, dy);
                self.ctx.request_repaint();
            }
            Command::Open(path) => {
                if let Err(error) = self
                    .validate_path(&path)
                    .and_then(|()| self.app.open_path(&path).map(|_| ()))
                {
                    self.app.ui.status = error.clone();
                    self.app.ui.status_error = true;
                    self.ctx.request_repaint();
                    return Err(error);
                }
                self.ctx.request_repaint();
                self.record()?;
            }
            Command::Save(path) => {
                self.validate_path(&path)?;
                let document = &self.app.session.active().ok_or("no active document")?.doc;
                let export = self
                    .app
                    .services
                    .export
                    .as_ref()
                    .ok_or("no document exporter")?;
                let (bytes, _) = export(document, &path, &Default::default())?;
                photocraft_format::atomic_write(Path::new(&path), &bytes)
                    .map_err(|e| e.to_string())?;
                self.app.ui.status = "PNG exported".into();
                self.ctx.request_repaint();
            }
            Command::TakeFileRequest(reply) => {
                let _ = reply.send(self.file_requests.take_json());
            }
            Command::PrepareFileSave(id, name, reply) => {
                let result = self.file_requests.prepare_save(&mut self.app, id, &name);
                if let Err(error) = &result {
                    self.app.ui.status = error.clone();
                    self.app.ui.status_error = true;
                }
                let _ = reply.send(result);
                self.ctx.request_repaint();
            }
            Command::CompleteFileRequest(id, result, resolved_path, name, error) => {
                let completion = self.file_requests.complete_with_path(
                    &mut self.app,
                    &self.ctx,
                    id,
                    &result,
                    &resolved_path,
                    &name,
                    &error,
                );
                #[cfg(feature = "device-tests")]
                if let Some(session) = &mut self.device_tests {
                    session.file_completion(id, &result, &name, completion.is_ok());
                }
                #[cfg(feature = "device-tests")]
                self.capture_test_snapshot();
                completion?;
                self.ctx.request_repaint();
                self.flush_ui();
                self.record()?;
            }
            Command::PlatformInput(packet) => {
                #[cfg(feature = "device-tests")]
                if packet.0["kind"] == "imeCommit"
                    && let Some(session) = &mut self.device_tests
                {
                    session.native_ime_commit(packet.0["text"].as_str().unwrap_or(""));
                }
                // Flush earlier keys first. Each queued packet sees the real
                // updated editor, including surrogate pairs and composition.
                self.flush_ui();
                let result = if packet.0["kind"] == "recentSourcesChanged" {
                    self.recent.refresh(&mut self.app)
                } else if matches!(
                    packet.0["kind"].as_str(),
                    Some("folderComplete" | "folderProgress")
                ) {
                    self.folder_requests.apply(packet.0, &mut self.app)
                } else if matches!(
                    packet.0["kind"].as_str(),
                    Some(
                        "folderDestinationComplete"
                            | "folderStageComplete"
                            | "folderPublishProgress"
                            | "folderPublishComplete"
                    )
                ) {
                    self.processor.apply(packet.0, &mut self.app)
                } else {
                    self.platform.apply(
                        packet,
                        &mut self.app,
                        &self.ctx,
                        &mut self.input,
                        &mut self.editor,
                    )
                };
                if let Err(error) = &result {
                    self.report_error(error);
                }
                self.flush_ui();
                result?;
            }
            Command::FileError(error) => {
                self.app.ui.status = error;
                self.app.ui.status_error = true;
                self.ctx.request_repaint();
            }
            #[cfg(feature = "device-tests")]
            Command::TestControl(request) => {
                self.device_tests
                    .as_ref()
                    .ok_or("device tests require isolated storage")?;
                if request.method == "ui.close.save" {
                    self.flush_ui();
                    let response = if self.app.native_close_pending() {
                        self.app.save_unsaved_changes(&self.ctx);
                        serde_json::json!({"ok":true,"result":{"closePending":self.app.native_close_pending()}})
                    } else {
                        serde_json::json!({"ok":false,"error":"the original unsaved-changes prompt is not pending"})
                    };
                    let _ = request.reply.send(response);
                } else {
                    self.device_tests
                        .as_ref()
                        .ok_or("device tests require isolated storage")?
                        .enqueue(request)?;
                }
                self.ctx.request_repaint();
            }
        }
        #[cfg(feature = "device-tests")]
        self.capture_test_snapshot();
        Ok(())
    }

    #[cfg(feature = "device-tests")]
    pub(crate) fn device_mailbox(&self) -> Option<crate::device_tests::SharedMailbox> {
        self.device_tests
            .as_ref()
            .map(|session| session.mailbox.clone())
    }

    #[cfg(feature = "device-tests")]
    fn capture_test_snapshot(&mut self) {
        if let Some(session) = &mut self.device_tests {
            session.capture(&self.app, &self.ctx, serde_json::json!({
                "frames":self.frames,"uiPresented":self.ui_presented,
                "sizePixels":self.surface.as_ref().map(|surface| surface.size),"density":self.density,
                "storageRoots":{"files":&self.files,"cache":&self.cache},
                "fileOperationPending":self.file_requests.is_pending(),
                "closePending":self.app.native_close_pending(),
                "backend":self.surface.as_ref().map(|surface| format!("{:?}", surface.state.adapter.get_info().backend))
            }));
        }
    }

    fn validate_path(&self, path: &str) -> Result<(), String> {
        let path = Path::new(path);
        if !path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            || !(path.starts_with(&self.files) || path.starts_with(&self.cache))
        {
            return Err("file must be staged in PhotoCraft's sandbox".into());
        }
        Ok(())
    }

    fn paint(&mut self, timestamp: u64) -> Result<(), String> {
        let now = Instant::now();
        {
            let mut pending = self.repaint.lock().unwrap_or_else(|e| e.into_inner());
            let due = pending.is_some_and(|when| when <= now);
            if !due && now < self.next_repaint {
                return Ok(());
            }
            if due {
                *pending = None;
            }
        }
        let Some(surface) = &self.surface else {
            return Ok(());
        };
        if surface.size.contains(&0) {
            return Ok(());
        }
        let output = self.run_ui(timestamp as f64 / 1_000_000_000.0);
        let mut combined = self.pending_output.take().unwrap_or_default();
        combined.append(output);
        let output = combined;
        let delay = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|viewport| viewport.repaint_delay)
            .unwrap_or(Duration::from_secs(1));
        self.next_repaint = now
            .checked_add(delay)
            .unwrap_or(now + Duration::from_secs(86400));
        let has_ui = !output.shapes.is_empty();
        if self
            .surface
            .as_mut()
            .ok_or("surface disappeared")?
            .paint(&self.ctx, output)?
        {
            self.frames = self.frames.saturating_add(1);
            let first_ui = has_ui && !self.ui_presented;
            if first_ui {
                self.ui_presented = true;
                crate::logger::info("PhotoCraft UI first frame presented");
            }
            if first_ui || self.frames == 1 || self.frames.is_multiple_of(120) {
                self.record()?;
            }
        }
        Ok(())
    }

    pub fn report_error(&mut self, error: &str) {
        self.app.ui.status = error.into();
        self.app.ui.status_error = true;
        self.ctx.request_repaint();
    }

    fn flush_ui(&mut self) {
        let output = self.run_ui(self.ctx.input(|input| input.time));
        self.pending_output
            .get_or_insert_with(Default::default)
            .append(output);
        self.ctx.request_repaint();
    }

    fn run_ui(&mut self, time: f64) -> egui::FullOutput {
        for event in self.platform.take_editing_actions() {
            self.input.event(event);
        }
        let mut input =
            self.input
                .take_with_zoom(self.ui_size, self.density, time, self.ctx.zoom_factor());
        self.platform.prepare_input(&mut input);
        self.app.raw_input_hook(&self.ctx, &mut input);
        let mut logic_done = false;
        let mut output = self.ctx.run_ui(input, |ui| {
            if !logic_done {
                self.app.logic(ui.ctx(), &mut self.frame);
                logic_done = true;
            }
            self.app.ui(ui, &mut self.frame);
        });
        self.file_requests.finish_frame(&mut self.app);
        self.folder_requests.finish_frame(&mut self.app);
        self.processor.finish_frame(&mut self.app);
        if let Err(error) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.platform.capture(
                &mut self.app,
                &self.ctx,
                &mut self.editor,
                &mut output,
                self.density,
            )
        }))
        .unwrap_or_else(|_| Err("PhotoCraft recovered from a platform output panic".into()))
        {
            self.report_error(&error);
        }
        #[cfg(feature = "device-tests")]
        self.capture_test_snapshot();
        output
    }

    fn record(&self) -> Result<(), String> {
        let backend = self
            .surface
            .as_ref()
            .map(|s| format!("{:?}", s.state.adapter.get_info().backend));
        let adapter = self
            .surface
            .as_ref()
            .map(|s| s.state.adapter.get_info().name);
        let size = self.surface.as_ref().map(|s| s.size);
        let data = serde_json::json!({ "upstream": "4337a62", "backend": backend, "adapter": adapter,
            "size_pixels": size, "density": self.density, "frames": self.frames,
            "document": self.app.session.active().map(|st| st.doc.name.clone()), "gpu_compositor": false,
            "ui_presented": self.ui_presented });
        photocraft_format::atomic_write(
            &self.files.join("photocraft-runtime.json"),
            data.to_string().as_bytes(),
        )
        .map_err(|e| e.to_string())
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        if let Some(output) = self.pending_output.take() {
            output.drop_without_applying_deltas();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_language_updates_render_chinese_menus_and_preserve_explicit_preferences()
    -> Result<(), String> {
        use photocraft_ui_egui::i18n;

        // The UI language is process-wide. Keep this integration test independent of other
        // rendering tests that can run in parallel with English labels.
        const CHILD: &str = "PHOTOCRAFT_SYSTEM_LANGUAGE_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(
                std::env::current_exe().map_err(|error| error.to_string())?,
            )
            .args([
                "--exact",
                "runner::tests::system_language_updates_render_chinese_menus_and_preserve_explicit_preferences",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .map_err(|error| error.to_string())?;
            assert!(
                output.status.success(),
                "isolated localization test failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return Ok(());
        }
        let root = std::env::temp_dir().join(format!(
            "photocraft-system-language-{}-{}",
            std::process::id(),
            crate::platform::next_id()
        ));
        let cache = root.join("cache");
        // SAFETY: A null pointer is rejected before read_string dereferences it.
        assert!(!unsafe { crate::craft_set_system_language(std::ptr::null()) });
        let tag = std::ffi::CString::new("zh-Hans-CN").map_err(|error| error.to_string())?;
        // SAFETY: tag is a valid C string kept alive through this call.
        assert!(unsafe { crate::craft_set_system_language(tag.as_ptr()) });
        assert_eq!(i18n::system_lang().code(), "zh-hans");
        let mut runner = Runner::new(
            root.to_str().ok_or("files path")?,
            cache.to_str().ok_or("cache path")?,
            1.0,
        )?;
        assert_eq!(runner.app.session.prefs().interface.language, "auto");

        fn visible_text(shape: &egui::epaint::Shape, text: &mut String) {
            match shape {
                egui::epaint::Shape::Text(shape) => {
                    text.push_str(shape.galley.text());
                    text.push('\n');
                }
                egui::epaint::Shape::Vec(shapes) => {
                    for shape in shapes {
                        visible_text(shape, text);
                    }
                }
                _ => {}
            }
        }
        fn menu_text(runner: &mut Runner) -> String {
            let mut text = String::new();
            let start = runner.ctx.input(|input| input.time);
            // egui's first frame can be a layout-only pass.
            for frame in 1..=4 {
                let output = runner.run_ui(start + f64::from(frame) / 60.0);
                text.clear();
                for shape in &output.shapes {
                    visible_text(&shape.shape, &mut text);
                }
                output.drop_without_applying_deltas();
            }
            text
        }

        let simplified = menu_text(&mut runner);
        assert!(simplified.contains("文件"), "{simplified}");
        assert!(simplified.contains("编辑"), "{simplified}");
        runner.handle(Command::SystemLanguage("zh-Hant-TW".into()))?;
        let traditional = menu_text(&mut runner);
        assert!(traditional.contains("檔案"), "{traditional}");
        assert!(traditional.contains("編輯"), "{traditional}");
        assert_eq!(runner.app.session.prefs().interface.language, "auto");

        runner
            .app
            .session
            .edit_prefs(|prefs| prefs.interface.language = "en".into());
        runner.handle(Command::SystemLanguage("zh-CN".into()))?;
        let explicit = menu_text(&mut runner);
        assert!(explicit.contains("File"), "{explicit}");
        assert!(!explicit.contains("文件"), "{explicit}");
        assert_eq!(runner.app.session.prefs().interface.language, "en");

        runner
            .app
            .session
            .edit_prefs(|prefs| prefs.interface.language = "auto".into());
        assert!(menu_text(&mut runner).contains("文件"));
        runner.handle(Command::SystemLanguage("fr-FR".into()))?;
        assert!(menu_text(&mut runner).contains("File"));
        assert_eq!(runner.app.session.prefs().interface.language, "auto");
        drop(runner);
        std::fs::remove_dir_all(root).map_err(|error| error.to_string())?;
        Ok(())
    }

    #[test]
    fn worker_clipboard_failure_is_visible_without_vsync_and_a_retry_imports_the_image()
    -> Result<(), String> {
        use serde_json::{Value, json};
        let root = std::env::temp_dir().join(format!(
            "photocraft-paste-retry-{}-{}",
            std::process::id(),
            crate::platform::next_id()
        ));
        let cache = root.join("cache");
        let mut runner = Runner::new(
            root.to_str().ok_or("files path")?,
            cache.to_str().ok_or("cache path")?,
            1.0,
        )?;
        let png = runner
            .app
            .services
            .encode_png
            .as_ref()
            .ok_or("encoder missing")?(1, 1, &[255, 0, 0, 255])?;
        runner.app.open_bytes("target.png", &png)?;
        let layers = runner
            .app
            .session
            .active()
            .ok_or("target missing")?
            .doc
            .layers
            .len();
        photocraft_ui_egui::menus::invoke(&mut runner.app, &runner.ctx, "edit.paste", json!({}))?;
        let request: Value = serde_json::from_str(&runner.platform.take_json())
            .map_err(|error| error.to_string())?;
        let foreign = cache.join("PhotoCraft/Clipboard/retained.png");
        std::fs::create_dir_all(foreign.parent().ok_or("foreign parent")?)
            .map_err(|error| error.to_string())?;
        std::fs::write(&foreign, &png).map_err(|error| error.to_string())?;
        let completion = json!({"kind":"clipboard","id":request["events"][0]["id"],
            "result":"success","imagePath":foreign.to_string_lossy(),"text":"","error":""});
        assert_eq!(
            runner.handle(Command::PlatformInput(crate::platform::Input::parse(
                &completion.to_string()
            )?)),
            Err("clipboard image must be in cacheDir/Clipboard".into())
        );
        assert!(runner.app.ui.status_error);
        assert_eq!(
            runner.app.ui.status,
            "clipboard image must be in cacheDir/Clipboard"
        );
        assert!(
            runner.pending_output.is_some(),
            "the error is presented without waiting for native VSync"
        );
        assert_eq!(
            runner
                .app
                .session
                .active()
                .ok_or("target missing")?
                .doc
                .layers
                .len(),
            layers
        );
        assert!(foreign.exists());
        photocraft_ui_egui::menus::invoke(&mut runner.app, &runner.ctx, "edit.paste", json!({}))?;
        let request: Value = serde_json::from_str(&runner.platform.take_json())
            .map_err(|error| error.to_string())?;
        let image = cache.join("Clipboard/retry.png");
        std::fs::write(&image, &png).map_err(|error| error.to_string())?;
        let completion = json!({"kind":"clipboard","id":request["events"][0]["id"],
            "result":"success","imagePath":image.to_string_lossy(),"text":"","error":""});
        runner.handle(Command::PlatformInput(crate::platform::Input::parse(
            &completion.to_string(),
        )?))?;
        assert_eq!(
            runner
                .app
                .session
                .active()
                .ok_or("target missing")?
                .doc
                .layers
                .len(),
            layers + 1
        );
        assert!(!runner.app.ui.status_error);
        assert!(!image.exists());
        drop(runner);
        std::fs::remove_dir_all(root).map_err(|error| error.to_string())?;
        Ok(())
    }

    #[cfg(feature = "device-tests")]
    #[test]
    fn device_transport_uses_original_picker_requests_and_waits_for_synthetic_input()
    -> Result<(), String> {
        use serde_json::{Value, json};
        let root = std::env::temp_dir().join(format!(
            "photocraft-device-transport-{}",
            std::process::id()
        ));
        let files = root.join("PhotoCraftTestRuns/transport-host/files");
        let cache = root.join("PhotoCraftTestRuns/transport-host/cache");
        let mut runner = Runner::new(
            files.to_str().ok_or("files path")?,
            cache.to_str().ok_or("cache path")?,
            1.0,
        )?;
        let mailbox = runner.device_mailbox().ok_or("missing device mailbox")?;
        let run_id = "transport-host";
        let initial: Value =
            serde_json::from_str(&crate::device_tests::snapshot(&mailbox, run_id)?)
                .map_err(|error| error.to_string())?;
        assert_eq!(initial["runtime"]["storageRoots"]["files"], json!(&files));
        assert_eq!(initial["runtime"]["storageRoots"]["cache"], json!(&cache));
        let ticket = crate::device_tests::submit(
            &mailbox,
            run_id,
            r#"{"method":"ui.menu.invoke","params":{"id":"file.open"}}"#,
            |request| runner.handle(Command::TestControl(request)),
        )?;
        assert_eq!(crate::device_tests::poll(&mailbox, run_id, &ticket)?, "");
        tick(&mut runner, 0.0);
        let response: Value =
            serde_json::from_str(&crate::device_tests::poll(&mailbox, run_id, &ticket)?)
                .map_err(|error| error.to_string())?;
        assert_eq!(response["ok"], true);
        assert!(
            runner.app.session.active().is_none(),
            "menu invocation must wait for the real picker boundary"
        );
        let request: Value = serde_json::from_str(&runner.file_requests.take_json())
            .map_err(|error| error.to_string())?;
        assert_eq!(request["kind"], "open");
        assert_eq!(request["intent"], "file.open");
        let id = request["id"].as_u64().ok_or("file request ID")?;
        let path = files.join("input.png");
        let png = runner
            .app
            .services
            .encode_png
            .as_ref()
            .ok_or("PNG encoder")?(1, 1, &[255, 0, 0, 255])?;
        std::fs::write(&path, png).map_err(|error| error.to_string())?;
        runner.handle(Command::CompleteFileRequest(
            id,
            "success".into(),
            path.to_string_lossy().into_owned(),
            "input.png".into(),
            String::new(),
        ))?;
        tick(&mut runner, 1.0);
        let ticket = crate::device_tests::submit(
            &mailbox,
            run_id,
            r#"{"method":"ui.key","params":{"key":"I","ctrl":true,"command":true}}"#,
            |request| runner.handle(Command::TestControl(request)),
        )?;
        tick(&mut runner, 2.0);
        assert_eq!(
            crate::device_tests::poll(&mailbox, run_id, &ticket)?,
            "",
            "synthetic key reply must wait for subsequent input frames"
        );
        tick(&mut runner, 3.0);
        let pending = crate::device_tests::poll(&mailbox, run_id, &ticket)?;
        let response = if pending.is_empty() {
            tick(&mut runner, 4.0);
            crate::device_tests::poll(&mailbox, run_id, &ticket)?
        } else {
            pending
        };
        assert_eq!(
            serde_json::from_str::<Value>(&response).map_err(|error| error.to_string())?["ok"],
            true
        );
        let first = crate::device_tests::snapshot(&mailbox, run_id)?;
        assert_eq!(
            crate::device_tests::snapshot(&mailbox, run_id)?,
            first,
            "snapshots do not consume state"
        );
        let snapshot: Value = serde_json::from_str(&first).map_err(|error| error.to_string())?;
        assert_eq!(
            snapshot["documentPixels"]["rgba8"],
            json!([0, 255, 255, 255])
        );
        assert_eq!(
            snapshot["runtime"]["input"]["nativeKeyEvents"], 0,
            "synthetic control must not count as native input"
        );
        runner.handle(Command::Key(2023, true, false, false, false, String::new()))?;
        let snapshot: Value =
            serde_json::from_str(&crate::device_tests::snapshot(&mailbox, run_id)?)
                .map_err(|error| error.to_string())?;
        assert_eq!(snapshot["runtime"]["input"]["nativeKeyEvents"], 1);
        let call = |runner: &mut Runner, method: &str, params: Value, time: f64| {
            let request = json!({"method":method,"params":params}).to_string();
            let ticket = crate::device_tests::submit(&mailbox, run_id, &request, |request| {
                runner.handle(Command::TestControl(request))
            })?;
            tick(runner, time);
            let response = crate::device_tests::poll(&mailbox, run_id, &ticket)?;
            serde_json::from_str::<Value>(&response).map_err(|error| error.to_string())
        };
        let prefs = call(
            &mut runner,
            "ui.menu.invoke",
            json!({"id":"edit.preferences.fileHandling"}),
            5.0,
        )?;
        let dialog = prefs["result"]["dialog"]
            .as_u64()
            .ok_or("preferences dialog")?;
        assert_eq!(
            call(
                &mut runner,
                "ui.dialog.set",
                json!({"dialog":dialog,"field":"values",
            "value":{"fileHandling":{"autosave":true,"autosaveMinutes":1,"recoverOnLaunch":true}}}),
                6.0
            )?["ok"],
            true
        );
        assert_eq!(
            call(
                &mut runner,
                "ui.dialog.confirm",
                json!({"dialog":dialog}),
                7.0
            )?["ok"],
            true
        );
        assert_eq!(runner.app.session.prefs().file_handling.autosave_minutes, 1);
        assert!(runner.app.session.prefs().file_handling.recover_on_launch);
        assert_eq!(
            call(
                &mut runner,
                "ui.menu.invoke",
                json!({"id":"file.exit"}),
                8.0
            )?["ok"],
            true
        );
        assert!(runner.app.native_close_pending());
        assert!(!runner.app.allow_native_close());
        assert_eq!(
            call(&mut runner, "ui.close.save", json!({}), 9.0)?["ok"],
            true
        );
        let snapshot: Value =
            serde_json::from_str(&crate::device_tests::snapshot(&mailbox, run_id)?)
                .map_err(|error| error.to_string())?;
        assert_eq!(snapshot["runtime"]["closePending"], true);
        assert_eq!(snapshot["runtime"]["fileOperationPending"], true);
        let save: Value = serde_json::from_str(&runner.file_requests.take_json())
            .map_err(|error| error.to_string())?;
        assert_eq!(save["kind"], "save");
        assert!(
            !runner.app.allow_native_close(),
            "close waits for real publication"
        );
        let state = runner.app.session.active().ok_or("close kept document")?;
        assert_ne!(state.saved_revision, state.revision);
        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }

    fn tick(runner: &mut Runner, time: f64) {
        let output = runner.run_ui(time);
        output.drop_without_applying_deltas();
    }

    #[cfg(feature = "device-tests")]
    #[test]
    fn device_document_activation_keeps_edits_with_the_original_document() -> Result<(), String> {
        use serde_json::{Value, json};
        let root = std::env::temp_dir().join(format!(
            "photocraft-device-documents-{}",
            std::process::id()
        ));
        let files = root.join("PhotoCraftTestRuns/document-host/files");
        let cache = root.join("PhotoCraftTestRuns/document-host/cache");
        let mut runner = Runner::new(
            files.to_str().ok_or("files path")?,
            cache.to_str().ok_or("cache path")?,
            1.0,
        )?;
        let mailbox = runner.device_mailbox().ok_or("missing device mailbox")?;
        let mut frame = 0.0;
        let mut call = |runner: &mut Runner, method: &str, params: Value| {
            let request = json!({"method":method,"params":params}).to_string();
            let ticket =
                crate::device_tests::submit(&mailbox, "document-host", &request, |request| {
                    runner.handle(Command::TestControl(request))
                })?;
            tick(runner, frame);
            frame += 1.0;
            let response = crate::device_tests::poll(&mailbox, "document-host", &ticket)?;
            serde_json::from_str::<Value>(&response).map_err(|error| error.to_string())
        };
        for name in ["first", "second"] {
            let opened = call(&mut runner, "ui.menu.invoke", json!({"id":"file.new"}))?;
            let dialog = opened["result"]["dialog"]
                .as_u64()
                .ok_or("new document dialog")?;
            for (field, value) in [
                ("name", json!(name)),
                ("width", json!(1)),
                ("height", json!(1)),
                ("background", json!("white")),
            ] {
                assert_eq!(
                    call(
                        &mut runner,
                        "ui.dialog.set",
                        json!({"dialog":dialog,"field":field,"value":value})
                    )?["ok"],
                    true
                );
            }
            assert_eq!(
                call(&mut runner, "ui.dialog.confirm", json!({"dialog":dialog}))?["ok"],
                true
            );
        }
        assert_eq!(runner.app.session.documents().len(), 2);
        assert_eq!(runner.app.session.active_index(), Some(1));
        let first_revision = runner.app.session.documents()[0].revision;
        assert_eq!(
            call(
                &mut runner,
                "ui.menu.invoke",
                json!({"id":"image.adjustments.invert"})
            )?["ok"],
            true
        );
        let second_revision = runner.app.session.documents()[1].revision;
        assert_eq!(runner.app.session.documents()[0].revision, first_revision);
        assert_eq!(
            call(
                &mut runner,
                "ui.menu.invoke",
                json!({"id":"document.activate","params":{"document":0}})
            )?["ok"],
            true
        );
        let first: Value =
            serde_json::from_str(&crate::device_tests::snapshot(&mailbox, "document-host")?)
                .map_err(|error| error.to_string())?;
        assert_eq!(first["ui"]["session"]["active"], 0);
        assert_eq!(
            first["documentPixels"]["rgba8"],
            json!([255, 255, 255, 255])
        );
        assert_eq!(
            call(
                &mut runner,
                "ui.menu.invoke",
                json!({"id":"image.adjustments.invert"})
            )?["ok"],
            true
        );
        let edited_first_revision = runner.app.session.documents()[0].revision;
        assert_ne!(edited_first_revision, first_revision);
        assert_eq!(runner.app.session.documents()[1].revision, second_revision);
        assert_eq!(
            call(
                &mut runner,
                "ui.menu.invoke",
                json!({"id":"document.activate","params":{"document":1}})
            )?["ok"],
            true
        );
        let second: Value =
            serde_json::from_str(&crate::device_tests::snapshot(&mailbox, "document-host")?)
                .map_err(|error| error.to_string())?;
        assert_eq!(second["ui"]["session"]["active"], 1);
        assert_eq!(second["documentPixels"]["rgba8"], json!([0, 0, 0, 255]));
        assert_eq!(
            call(
                &mut runner,
                "ui.menu.invoke",
                json!({"id":"document.activate","params":{"document":99}})
            )?["ok"],
            false
        );
        assert_eq!(runner.app.session.active_index(), Some(1));
        assert_eq!(
            runner.app.session.documents()[0].revision,
            edited_first_revision
        );
        assert_eq!(runner.app.session.documents()[1].revision, second_revision);
        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }

    #[test]
    fn png_open_keyboard_edit_export_preserves_working_document() -> Result<(), String> {
        let root = std::env::temp_dir().join(format!("photocraft-workflow-{}", std::process::id()));
        let mut runner = Runner::new(
            root.to_string_lossy().as_ref(),
            root.join("cache").to_string_lossy().as_ref(),
            1.0,
        )?;
        let image = root.join("input.png");
        let bytes = runner
            .app
            .services
            .encode_png
            .as_ref()
            .ok_or("missing PNG encoder")?(1, 1, &[255, 0, 0, 255])?;
        std::fs::write(&image, bytes).map_err(|e| e.to_string())?;
        runner.handle(Command::Open(image.to_string_lossy().into_owned()))?;
        tick(&mut runner, 0.0);
        tick(&mut runner, 1.0);
        runner.handle(Command::Key(2025, true, true, false, false, String::new()))?;
        tick(&mut runner, 2.0);
        let output = root.join("cache/output.png");
        runner.handle(Command::Save(output.to_string_lossy().into_owned()))?;
        let exported = std::fs::read(output).map_err(|e| e.to_string())?;
        let pixels = photocraft_codecs::decode(&exported)
            .map_err(|e| e.to_string())?
            .to_rgba8();
        assert_eq!(pixels, [0, 255, 255, 255]);
        let state = runner.app.session.active().ok_or("missing document")?;
        assert_eq!(state.path.as_deref(), image.to_str());
        assert_ne!(state.saved_revision, state.revision);
        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }

    #[test]
    fn renders_upstream_ui_without_a_desktop_window() -> Result<(), String> {
        let root = std::env::temp_dir().join(format!("photocraft-headless-{}", std::process::id()));
        let mut runner = Runner::new(
            root.to_string_lossy().as_ref(),
            root.join("cache").to_string_lossy().as_ref(),
            1.0,
        )?;
        let mut shapes = 0;
        for frame in 0..3 {
            let input = runner.input.take([1280, 800], 1.0, f64::from(frame));
            let output = runner.ctx.run_ui(input, |ui| {
                runner.app.logic(ui.ctx(), &mut runner.frame);
                runner.app.ui(ui, &mut runner.frame);
            });
            shapes = output.shapes.len();
            output.drop_without_applying_deltas();
        }
        assert!(shapes > 0);
        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }
    #[test]
    fn document_ime_chinese_cancel_utf16_and_queued_deletes_use_fresh_text() -> Result<(), String> {
        let root =
            std::env::temp_dir().join(format!("photocraft-document-ime-{}", std::process::id()));
        let mut runner = Runner::new(
            root.to_str().ok_or("invalid root")?,
            root.join("cache").to_str().ok_or("invalid cache")?,
            1.0,
        )?;
        runner
            .app
            .session
            .execute("file.new", serde_json::json!({"width":400,"height":200}))
            .map_err(|error| error.to_string())?;
        runner.app.sync_views();
        runner.app.ui.tool = photocraft_ui_egui::state::Tool::Type;
        photocraft_ui_egui::type_tool::pointer_up(&mut runner.app, [50.0, 100.0], [50.0, 100.0]);
        runner.flush_ui();
        runner.flush_ui();
        let output: serde_json::Value = serde_json::from_str(&runner.platform.take_json())
            .map_err(|error| error.to_string())?;
        let target = output["ime"]["target"]
            .as_str()
            .ok_or("document IME snapshot missing")?
            .to_string();
        let send = |runner: &mut Runner, mut value: serde_json::Value| {
            value["target"] = serde_json::json!(target);
            runner.handle(Command::PlatformInput(crate::platform::Input::parse(
                &value.to_string(),
            )?))
        };
        send(
            &mut runner,
            serde_json::json!({"kind":"imeCommit","text":"甲😀乙"}),
        )?;
        assert_eq!(
            photocraft_ui_egui::type_tool::editing_text(&runner.app).as_deref(),
            Some("甲😀乙")
        );
        send(
            &mut runner,
            serde_json::json!({"kind":"imeSelection","start":1,"end":3}),
        )?;
        send(
            &mut runner,
            serde_json::json!({"kind":"imePreedit","text":"中文"}),
        )?;
        assert_eq!(
            photocraft_ui_egui::type_tool::editing_text(&runner.app).as_deref(),
            Some("甲中文乙")
        );
        send(&mut runner, serde_json::json!({"kind":"imeCancel"}))?;
        assert_eq!(
            photocraft_ui_egui::type_tool::editing_text(&runner.app).as_deref(),
            Some("甲😀乙")
        );
        send(
            &mut runner,
            serde_json::json!({"kind":"imeSelection","start":0,"end":4}),
        )?;
        send(
            &mut runner,
            serde_json::json!({"kind":"imeCommit","text":"ab😀c"}),
        )?;
        send(
            &mut runner,
            serde_json::json!({"kind":"imeDelete","before":1,"after":0}),
        )?;
        send(
            &mut runner,
            serde_json::json!({"kind":"imeDelete","before":2,"after":0}),
        )?;
        assert_eq!(
            photocraft_ui_egui::type_tool::editing_text(&runner.app).as_deref(),
            Some("ab")
        );
        // The retained texture deltas are consumed by Runner::drop even without a surface.
        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }
    #[test]
    fn queued_window_exit_waits_for_save_cancel_failure_and_actual_publication()
    -> Result<(), String> {
        let root =
            std::env::temp_dir().join(format!("photocraft-window-exit-{}", std::process::id()));
        let mut runner = Runner::new(
            root.to_str().ok_or("invalid root")?,
            root.join("cache").to_str().ok_or("invalid cache")?,
            1.0,
        )?;
        runner
            .app
            .session
            .execute("file.new", serde_json::json!({"width":2,"height":2}))
            .map_err(|error| error.to_string())?;
        runner.app.run("layer.new.layer", serde_json::json!({}))?;
        runner.handle(Command::PlatformInput(crate::platform::Input::parse(
            r#"{"kind":"closeRequested","id":42}"#,
        )?))?;
        for result in ["cancel", "error"] {
            runner.app.save_unsaved_changes(&runner.ctx);
            runner.flush_ui();
            let request: serde_json::Value =
                serde_json::from_str(&runner.file_requests.take_json())
                    .map_err(|error| error.to_string())?;
            runner.handle(Command::CompleteFileRequest(
                request["id"].as_u64().ok_or("missing id")?,
                result.into(),
                String::new(),
                String::new(),
                "publish failed".into(),
            ))?;
            // Completion must publish close state without a native VSync.
            assert!(runner.app.native_close_pending());
            assert_eq!(runner.app.session.documents().len(), 1);
            let output: serde_json::Value = serde_json::from_str(&runner.platform.take_json())
                .map_err(|error| error.to_string())?;
            assert_eq!(
                output["closeState"],
                serde_json::json!({"id":42,"pending":true})
            );
            assert_eq!(output["events"], serde_json::json!([]));
        }
        runner.app.save_unsaved_changes(&runner.ctx);
        runner.flush_ui();
        let request: serde_json::Value = serde_json::from_str(&runner.file_requests.take_json())
            .map_err(|error| error.to_string())?;
        runner.handle(Command::CompleteFileRequest(
            request["id"].as_u64().ok_or("missing id")?,
            "success".into(),
            String::new(),
            String::new(),
            String::new(),
        ))?;
        let output: serde_json::Value = serde_json::from_str(&runner.platform.take_json())
            .map_err(|error| error.to_string())?;
        assert_eq!(
            output["closeState"],
            serde_json::json!({"id":42,"pending":false})
        );
        assert_eq!(output["events"][0]["kind"], "close");
        assert_eq!(runner.app.session.documents().len(), 1);
        assert!(!runner.app.session.active().ok_or("missing doc")?.is_dirty());
        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }
}
