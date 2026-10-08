//! Original engine PDF rendering, followed by explicit system printing or URI
//! publication. This adapter never starts a desktop spooler process.

use std::path::{Path, PathBuf};

use photocraft_engine::Session;
use photocraft_ui_egui::{Services, i18n};
use serde_json::{Value, json};

use crate::file_requests::FileRequests;
use crate::platform::Platform;

pub fn attach(
    hooks: &mut Services,
    session: &mut Session,
    files: &Path,
    requests: &FileRequests,
    platform: &Platform,
) -> Result<(), String> {
    if !files.is_absolute() {
        return Err("printing requires an absolute filesDir".into());
    }
    let root = files.join("PhotoCraft/Documents/Printing");
    std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let spool_root = root.clone();
    let spool_platform = platform.clone();
    session.print_spool = Some(std::sync::Arc::new(move |job| {
        submit(job, &spool_root, &spool_platform).map_err(photocraft_engine::EngineError::Other)
    }));
    let files = requests.clone();
    hooks.print = Some(Box::new(move |session, command, params| {
        run(session, command, params, &root, &files)
    }));
    Ok(())
}

fn run(
    session: &mut Session,
    command: &str,
    params: &Value,
    root: &Path,
    requests: &FileRequests,
) -> Result<Value, String> {
    let original = match params {
        Value::Null => json!({}),
        Value::Object(_) => params.clone(),
        _ => return Err("print settings must be an object".into()),
    };
    // One Copy retains the original engine's last-settings merge and copy count.
    let output = original["output"].as_str().unwrap_or_default().trim();
    let dry = original["dryRun"].as_bool().unwrap_or(false);
    let export = !output.is_empty() || (original["send"] == false && !dry);
    if !export && !dry {
        // This engine hook also receives nested scripts, print events and Batch
        // scratch sessions. The command keeps its original parameter journal.
        return session
            .execute(command, original)
            .map_err(|error| error.to_string());
    }
    let path = if export {
        let suggested = if output.is_empty() {
            "PhotoCraft.pdf"
        } else {
            output
        };
        if !Path::new(suggested)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
        {
            return Err("Print to PDF requires a .pdf destination name".into());
        }
        PathBuf::from(requests.stage_export(suggested, "file.printToPdf")?)
    } else {
        let directory = loop {
            let id = crate::platform::next_id();
            let directory = root.join(id.to_string());
            match std::fs::create_dir(&directory) {
                Ok(()) => break directory,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.to_string()),
            }
        };
        directory.join("PhotoCraft.pdf")
    };
    let mut render = original.clone();
    render["output"] = json!(path.to_string_lossy());
    render["send"] = json!(false);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        session.execute(command, render)
    }))
    .map_err(|_| "Print rendering failed with an internal error".to_string())
    .and_then(|result| result.map_err(|error| error.to_string()));
    let mut result = match result {
        Ok(result) => result,
        Err(error) => {
            if export {
                requests.abort_stage();
            } else {
                cleanup(&path);
            }
            return Err(error);
        }
    };
    // Recording an action must retain its user's print intent, never a staging
    // filename which would turn later playback into an unrelated PDF export.
    if let Some((id, recorded)) = session.journal.last_mut()
        && id == command
    {
        *recorded = original.clone();
    }
    if let Some(remembered) = session
        .file_menu
        .last_print
        .as_mut()
        .and_then(Value::as_object_mut)
    {
        if let Some(send) = original.get("send") {
            remembered.insert("send".into(), send.clone());
        } else {
            remembered.remove("send");
        }
    }
    let status = if export {
        i18n::t("Waiting to publish PDF").to_string()
    } else {
        i18n::t("PDF rendered for dry run; not submitted to a printer").to_string()
    };
    result["pending"] = json!(export || !dry);
    result["status"] = json!(status);
    Ok(result)
}

fn submit(
    job: &photocraft_engine::print_cmds::PrintSpool<'_>,
    root: &Path,
    platform: &Platform,
) -> Result<photocraft_engine::print_cmds::PrintSpoolResult, String> {
    platform.check_print_capacity()?;
    let (id, directory) = loop {
        let id = crate::platform::next_id();
        let directory = root.join(id.to_string());
        match std::fs::create_dir(&directory) {
            Ok(()) => break (id, directory),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    };
    let path = directory.join("PhotoCraft.pdf");
    if let Err(error) = photocraft_format::atomic_write(&path, job.pdf)
        .map_err(|error| error.to_string())
        .and_then(|()| platform.queue_print(id, &path, job.document.0, job.copies, job.printer))
    {
        cleanup(&path);
        return Err(error);
    }
    Ok(photocraft_engine::print_cmds::PrintSpoolResult {
        pdf: path.to_string_lossy().into_owned(),
        request: id,
        status: i18n::t("Waiting for system printing; confirm printer and copy count in the system dialog").into(),
    })
}

pub(crate) fn cleanup(path: &Path) {
    let _ = std::fs::remove_file(path);
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir(parent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{Editor, Input};
    use craft_ohos_platform::input::InputState;
    use photocraft_ui_egui::PhotocraftApp;

    struct Fixture {
        root: PathBuf,
        app: PhotocraftApp,
        platform: Platform,
        requests: FileRequests,
        ctx: egui::Context,
        input: InputState,
        editor: Editor,
    }

    impl Fixture {
        fn new() -> Result<Self, String> {
            let root = std::env::temp_dir()
                .join(format!("photocraft-print-{}", crate::platform::next_id()));
            let requests = FileRequests::new(&root)?;
            let platform = Platform::new(&root.join("cache"))?;
            let mut hooks = crate::services::services_with_files(&root, &requests);
            platform.attach_services(&mut hooks);
            let mut session = Session::new();
            attach(&mut hooks, &mut session, &root, &requests, &platform)?;
            let mut app = PhotocraftApp::new(session, hooks);
            app.run(
                "file.new",
                json!({"width":3,"height":2,"name":"Print original.psd","background":"#3366cc"}),
            )?;
            app.run("layer.new.layer", json!({"name":"unsaved"}))?;
            Ok(Self {
                root,
                app,
                platform,
                requests,
                ctx: egui::Context::default(),
                input: InputState::default(),
                editor: Editor::default(),
            })
        }

        fn event(&self) -> Result<Value, String> {
            let output: Value = serde_json::from_str(&self.platform.take_json())
                .map_err(|error| error.to_string())?;
            output["events"]
                .as_array()
                .and_then(|events| events.first())
                .cloned()
                .ok_or("missing print event".into())
        }

        fn update(
            &mut self,
            id: &Value,
            phase: &str,
            terminal: bool,
            error: &str,
        ) -> Result<(), String> {
            self.platform.apply(Input::parse(&json!({"kind":"printingUpdate","id":id,"state":phase,"terminal":terminal,"error":error}).to_string())?, &mut self.app, &self.ctx, &mut self.input, &mut self.editor)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn original_print_dialog_submits_the_original_engine_pdf_without_a_desktop_spooler()
    -> Result<(), String> {
        let mut fixture = Fixture::new()?;
        let before = fixture.app.session.active().ok_or("no document")?.clone();
        let dialog = photocraft_ui_egui::menus::invoke(
            &mut fixture.app,
            &fixture.ctx,
            "file.print",
            json!({}),
        )?["dialog"]
            .as_u64()
            .ok_or("no print dialog")?;
        let fields = &mut fixture
            .app
            .ui
            .dialog_mut(dialog)
            .ok_or("missing dialog")?
            .fields;
        fields.insert("printer".into(), json!("Office printer"));
        fields.insert("paper".into(), json!("a4"));
        fields.insert("orientation".into(), json!("landscape"));
        fields.insert("copies".into(), json!(3));
        fields.insert("cornerCropMarks".into(), json!(true));
        fields.insert("scaleToFit".into(), json!(true));
        fields.insert("colorHandling".into(), json!("photocraftManages"));
        fields.insert("printerProfile".into(), json!("coated-cmyk"));
        let mut expected = Value::Object(
            fields
                .iter()
                .filter(|(key, _)| !key.starts_with("__"))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        );
        let result = photocraft_ui_egui::dialogs::confirm(&mut fixture.app, dialog)?;
        assert_eq!(result["pending"], true);
        assert_eq!(result["sent"], false);
        assert!(result["command"].is_null());
        assert!(
            fixture
                .app
                .ui
                .status
                .starts_with("Waiting for system printing")
        );
        assert!(!fixture.app.ui.status.contains("Printed to"));
        let event = fixture.event()?;
        assert_eq!(event["kind"], "printRequest");
        assert!(
            event["id"]
                .as_u64()
                .is_some_and(|id| id <= 9_007_199_254_740_991)
        );
        let path = event["pdfPath"].as_str().ok_or("no PDF path")?;
        let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
        assert!(bytes.starts_with(b"%PDF-1.4"));
        assert!(String::from_utf8_lossy(&bytes).contains("/MediaBox [0 0 841.89 595.28]"));
        assert!(String::from_utf8_lossy(&bytes).contains("/N 4 /Alternate /DeviceCMYK"));
        assert!(
            Path::new(path).starts_with(
                fixture
                    .root
                    .canonicalize()
                    .map_err(|error| error.to_string())?
                    .join("PhotoCraft/Documents/Printing")
            )
        );
        let mut original = Session::new();
        original.add_document((*before.doc).clone(), before.path.clone());
        let oracle = fixture.root.join("oracle.pdf");
        expected["output"] = json!(oracle.to_string_lossy());
        expected["send"] = json!(false);
        original
            .execute("file.print", expected)
            .map_err(|error| error.to_string())?;
        assert_eq!(
            bytes,
            std::fs::read(oracle).map_err(|error| error.to_string())?
        );
        let after = fixture.app.session.active().ok_or("document lost")?;
        assert_eq!(after.doc, before.doc);
        assert_eq!(
            (after.path.clone(), after.revision, after.saved_revision),
            (before.path, before.revision, before.saved_revision)
        );
        assert!(after.is_dirty());
        Ok(())
    }

    #[test]
    fn print_one_copy_and_recorded_actions_keep_the_user_intent_and_last_layout()
    -> Result<(), String> {
        let mut fixture = Fixture::new()?;
        photocraft_ui_egui::actions::start_recording(&mut fixture.app);
        let settings = json!({"paper":"a5","orientation":"landscape","scale":50,"copies":3});
        fixture.app.run("file.print", settings.clone())?;
        let first = fixture.event()?;
        photocraft_ui_egui::actions::stop_recording(&mut fixture.app);
        let recorded = fixture
            .app
            .ui
            .actions
            .list
            .first()
            .ok_or("missing action")?;
        assert_eq!(recorded.steps, vec![("file.print".into(), settings)]);
        let copy = photocraft_ui_egui::menus::invoke(
            &mut fixture.app,
            &fixture.ctx,
            "file.printOneCopy",
            json!({}),
        )?;
        assert_eq!(copy["copies"], 1);
        assert_eq!(copy["paper"], json!([595.28, 419.53]));
        assert_eq!(copy["scale"], 50.0);
        let second = fixture.event()?;
        assert_ne!(first["id"], second["id"]);
        assert_ne!(first["pdfPath"], second["pdfPath"]);
        assert_eq!(photocraft_ui_egui::actions::play(&mut fixture.app, 0)?, 1);
        let replay = fixture.event()?;
        assert_eq!(replay["kind"], "printRequest");
        assert!(fixture.requests.take_json().is_empty());
        assert!(
            fixture
                .app
                .session
                .active()
                .ok_or("document lost")?
                .is_dirty()
        );
        Ok(())
    }

    #[test]
    fn print_job_acceptance_blocking_and_monitoring_failure_do_not_invent_completion()
    -> Result<(), String> {
        let mut fixture = Fixture::new()?;
        let before = fixture.app.session.active().ok_or("no document")?.clone();
        fixture.app.run("file.print", json!({"send":true}))?;
        let event = fixture.event()?;
        let path = event["pdfPath"].as_str().ok_or("missing path")?;
        for phase in ["submitted", "blocked", "monitoringError"] {
            fixture.update(
                &event["id"],
                phase,
                false,
                if phase == "monitoringError" {
                    "listener unavailable"
                } else {
                    ""
                },
            )?;
            assert!(Path::new(path).exists());
            assert!(!fixture.app.ui.status.contains("completed"));
            if phase == "submitted" {
                assert_eq!(
                    fixture.app.ui.status,
                    "System printing request accepted; final job state is unconfirmed"
                );
            }
            assert_eq!(fixture.app.ui.status_error, phase == "monitoringError");
        }
        assert!(fixture.update(&event["id"], "submitted", true, "").is_err());
        assert!(
            fixture
                .update(&event["id"], "guessedTimeout", true, "")
                .is_err()
        );
        assert!(Path::new(path).exists());
        fixture.update(&event["id"], "succeeded", true, "")?;
        assert!(!Path::new(path).exists());
        assert_eq!(fixture.app.ui.status, "System print job completed");
        assert!(!fixture.app.ui.status_error);
        assert!(fixture.update(&event["id"], "succeeded", true, "").is_err());
        for phase in ["failed", "cancelled", "rejected"] {
            fixture.app.run("file.print", json!({}))?;
            let event = fixture.event()?;
            let path = event["pdfPath"].as_str().ok_or("missing path")?;
            fixture.update(
                &event["id"],
                phase,
                true,
                if phase == "rejected" {
                    "service unavailable"
                } else {
                    ""
                },
            )?;
            assert!(!Path::new(path).exists());
            assert_eq!(fixture.app.ui.status_error, phase != "cancelled");
        }
        let after = fixture.app.session.active().ok_or("document lost")?;
        assert_eq!(
            (after.path.clone(), after.revision, after.saved_revision),
            (before.path, before.revision, before.saved_revision)
        );
        assert!(after.is_dirty());
        Ok(())
    }

    #[test]
    fn print_to_pdf_uses_the_file_publisher_and_never_saves_the_working_document()
    -> Result<(), String> {
        let mut fixture = Fixture::new()?;
        let before = fixture.app.session.active().ok_or("no document")?.clone();
        let original_file = fixture.root.join("existing.pdf");
        std::fs::write(&original_file, b"existing publication")
            .map_err(|error| error.to_string())?;
        for result in ["cancel", "error", "success"] {
            let dialog = photocraft_ui_egui::menus::invoke(
                &mut fixture.app,
                &fixture.ctx,
                "file.print",
                json!({}),
            )?["dialog"]
                .as_u64()
                .ok_or("missing dialog")?;
            fixture
                .app
                .ui
                .dialog_mut(dialog)
                .ok_or("missing print dialog")?
                .fields
                .insert("output".into(), json!(original_file.to_string_lossy()));
            let rendered = photocraft_ui_egui::dialogs::confirm(&mut fixture.app, dialog)?;
            assert_eq!(rendered["pending"], true);
            assert!(fixture.event().is_err());
            fixture.requests.finish_frame(&mut fixture.app);
            let request: Value = serde_json::from_str(&fixture.requests.take_json())
                .map_err(|error| error.to_string())?;
            assert_eq!(request["intent"], "file.printToPdf");
            assert_eq!(request["chooseDestination"], true);
            assert_eq!(request["allowFormatChoice"], false);
            assert_eq!(request["allowedExtensions"], json!(["pdf"]));
            let id = request["id"].as_u64().ok_or("missing file request id")?;
            let bytes = std::fs::read(request["path"].as_str().ok_or("no initial stage")?)
                .map_err(|error| error.to_string())?;
            assert!(bytes.starts_with(b"%PDF"));
            let path = fixture
                .requests
                .prepare_save(&mut fixture.app, id, "actual.pdf")?;
            assert_eq!(
                std::fs::read(&path).map_err(|error| error.to_string())?,
                bytes
            );
            fixture.requests.complete(
                &mut fixture.app,
                id,
                result,
                "actual.pdf",
                if result == "error" {
                    "provider failed"
                } else {
                    ""
                },
            )?;
            assert!(!Path::new(&path).exists());
            let after = fixture.app.session.active().ok_or("document lost")?;
            assert_eq!(
                (after.path.clone(), after.revision, after.saved_revision),
                (before.path.clone(), before.revision, before.saved_revision)
            );
            assert!(after.is_dirty());
            assert!(fixture.app.ui.recent_files.is_empty());
            assert_eq!(
                std::fs::read(&original_file).map_err(|error| error.to_string())?,
                b"existing publication"
            );
        }
        Ok(())
    }

    #[test]
    fn pdf_export_keeps_recorded_output_but_repeat_print_and_one_copy_submit_to_the_system()
    -> Result<(), String> {
        let mut fixture = Fixture::new()?;
        let settings = json!({"output":"requested-output.pdf","paper":"a5","copies":3,"scale":66});
        photocraft_ui_egui::actions::start_recording(&mut fixture.app);
        fixture.app.run("file.print", settings.clone())?;
        photocraft_ui_egui::actions::stop_recording(&mut fixture.app);
        assert_eq!(
            fixture
                .app
                .ui
                .actions
                .list
                .first()
                .ok_or("missing action")?
                .steps,
            vec![("file.print".into(), settings.clone())]
        );
        assert_eq!(
            fixture.app.session.journal.last(),
            Some(&("file.print".into(), settings))
        );
        let remembered = fixture
            .app
            .session
            .file_menu
            .last_print
            .as_ref()
            .ok_or("no print settings")?;
        assert!(remembered.get("output").is_none());
        assert!(remembered.get("send").is_none());
        fixture.requests.finish_frame(&mut fixture.app);
        let request: Value = serde_json::from_str(&fixture.requests.take_json())
            .map_err(|error| error.to_string())?;
        assert_eq!(request["intent"], "file.printToPdf");
        let id = request["id"].as_u64().ok_or("missing PDF request")?;
        fixture
            .requests
            .prepare_save(&mut fixture.app, id, "published.pdf")?;
        fixture
            .requests
            .complete(&mut fixture.app, id, "success", "published.pdf", "")?;
        let dialog = photocraft_ui_egui::menus::invoke(
            &mut fixture.app,
            &fixture.ctx,
            "file.print",
            json!({}),
        )?["dialog"]
            .as_u64()
            .ok_or("missing next print dialog")?;
        assert_eq!(
            fixture
                .app
                .ui
                .dialog_mut(dialog)
                .ok_or("missing dialog")?
                .fields["output"],
            ""
        );
        let repeated = photocraft_ui_egui::dialogs::confirm(&mut fixture.app, dialog)?;
        assert_eq!(repeated["pending"], true);
        assert_eq!(repeated["sent"], false);
        assert_eq!(repeated["copies"], 3);
        assert_eq!(repeated["scale"], 66.0);
        let first = fixture.event()?;
        assert_eq!(first["kind"], "printRequest");
        let single = photocraft_ui_egui::menus::invoke(
            &mut fixture.app,
            &fixture.ctx,
            "file.printOneCopy",
            json!({}),
        )?;
        assert_eq!(single["pending"], true);
        assert_eq!(single["copies"], 1);
        let second = fixture.event()?;
        assert_eq!(second["kind"], "printRequest");
        assert_ne!(first["id"], second["id"]);
        assert!(fixture.requests.take_json().is_empty());
        assert!(
            fixture
                .app
                .session
                .active()
                .ok_or("document lost")?
                .is_dirty()
        );
        Ok(())
    }

    #[test]
    fn engine_scripts_keep_each_pending_print_bound_to_its_rendered_document() -> Result<(), String>
    {
        let mut fixture = Fixture::new()?;
        let before = fixture
            .app
            .session
            .active()
            .ok_or("original missing")?
            .clone();
        let script = fixture.app.session.execute("file.scripts.browse", json!({"steps":[
            ["file.print",{"paper":"a5"}],
            ["file.new",{"name":"second source","width":5,"height":3,"background":"#ff0000"}],
            ["file.print",{"copies":2}]
        ]})).map_err(|error|error.to_string())?;
        assert_eq!(script["ok"], true);
        assert_eq!(script["results"][0]["result"]["pending"], true);
        assert_eq!(script["results"][2]["result"]["pending"], true);
        assert_eq!(script["results"][0]["result"]["sent"], false);
        let active = fixture.app.session.active().ok_or("second missing")?.doc.id;
        let output: Value = serde_json::from_str(&fixture.platform.take_json())
            .map_err(|error| error.to_string())?;
        let events = output["events"].as_array().ok_or("missing events")?;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["documentId"], before.doc.id.0.to_string());
        assert_eq!(events[1]["documentId"], active.0.to_string());
        assert_eq!(events[1]["requestedCopies"], 2);
        assert_ne!(events[0]["id"], events[1]["id"]);
        let first = events[0].clone();
        let second = events[1].clone();
        let first_path = first["pdfPath"].as_str().ok_or("missing first path")?;
        let second_path = second["pdfPath"].as_str().ok_or("missing second path")?;
        assert!(
            String::from_utf8_lossy(&std::fs::read(first_path).map_err(|error| error.to_string())?)
                .contains("/Width 3 /Height 2")
        );
        assert!(
            String::from_utf8_lossy(
                &std::fs::read(second_path).map_err(|error| error.to_string())?
            )
            .contains("/Width 5 /Height 3")
        );
        fixture.update(&first["id"], "succeeded", true, "")?;
        assert!(!Path::new(first_path).exists());
        assert!(Path::new(second_path).exists());
        let pending: Value = serde_json::from_str(&fixture.platform.take_json())
            .map_err(|error| error.to_string())?;
        assert_eq!(
            pending["printJobs"],
            json!([{"id":second["id"],"documentId":active.0.to_string()}])
        );
        assert_eq!(
            fixture.app.session.active().ok_or("active lost")?.doc.id,
            active
        );
        let original = fixture
            .app
            .session
            .documents()
            .iter()
            .find(|state| state.doc.id == before.doc.id)
            .ok_or("original lost")?;
        assert_eq!(
            (
                original.revision,
                original.saved_revision,
                original.path.clone()
            ),
            (before.revision, before.saved_revision, before.path)
        );
        fixture.update(&second["id"], "cancelled", true, "")?;
        Ok(())
    }

    #[test]
    fn print_script_events_run_normally_and_nested_prints_do_not_recurse() -> Result<(), String> {
        let mut fixture = Fixture::new()?;
        fixture.app.run(
            "file.scripts.scriptEventsManager",
            json!({"add":{"event":"print","steps":[
                ["layer.new.layer",{"name":"actual print event"}], ["file.printOneCopy",{}]
            ]}}),
        )?;
        let before = fixture
            .app
            .session
            .active()
            .ok_or("missing document")?
            .clone();
        let result = fixture.app.run("file.print", json!({"copies":3}))?;
        assert_eq!(result["pending"], true);
        assert!(result["warnings"].is_null());
        assert!(fixture.app.session.prefs().script_events.enabled);
        assert!(!fixture.app.session.file_menu.firing);
        assert_eq!(fixture.app.session.file_menu.event_log.len(), 1);
        let event_log = fixture
            .app
            .session
            .file_menu
            .event_log
            .first()
            .ok_or("missing script event result")?;
        assert_eq!(event_log["event"], "print");
        assert_eq!(event_log["results"][1]["result"]["pending"], true);
        let after = fixture.app.session.active().ok_or("document missing")?;
        assert_eq!(after.revision, before.revision + 1);
        assert_eq!(after.saved_revision, before.saved_revision);
        assert_eq!(
            after
                .doc
                .layers
                .iter()
                .filter(|layer| layer.name == "actual print event")
                .count(),
            1
        );
        let output: Value = serde_json::from_str(&fixture.platform.take_json())
            .map_err(|error| error.to_string())?;
        let events = output["events"].as_array().ok_or("missing events")?;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["requestedCopies"], 3);
        assert_eq!(events[1]["requestedCopies"], 1);
        assert_eq!(events[0]["documentId"], events[1]["documentId"]);
        fixture.update(&events[0]["id"], "succeeded", true, "")?;
        fixture.update(
            &events[1]["id"],
            "monitoringError",
            false,
            "listener unavailable",
        )?;
        assert!(
            fixture
                .app
                .session
                .active()
                .ok_or("document missing")?
                .is_dirty()
        );
        assert!(Path::new(events[1]["pdfPath"].as_str().ok_or("nested path missing")?).exists());
        Ok(())
    }

    #[test]
    fn batch_scratch_sessions_inherit_the_spooler_without_claiming_the_active_document()
    -> Result<(), String> {
        let mut fixture = Fixture::new()?;
        let before = fixture
            .app
            .session
            .active()
            .ok_or("missing original")?
            .clone();
        let inputs = fixture.root.join("batch-input");
        std::fs::create_dir(&inputs).map_err(|error| error.to_string())?;
        let encode = fixture
            .app
            .services
            .encode_png
            .as_ref()
            .ok_or("PNG encoder missing")?;
        let mut paths = Vec::new();
        for (name, pixel) in [
            ("blue.png", [0, 0, 255, 255]),
            ("red.png", [255, 0, 0, 255]),
        ] {
            let path = inputs.join(name);
            std::fs::write(&path, encode(1, 1, &pixel)?).map_err(|error| error.to_string())?;
            paths.push(path.to_string_lossy().into_owned());
        }
        let output_directory = fixture.root.join("batch-output");
        let batch=fixture.app.session.execute("file.automate.batch",json!({"input":paths,"output":output_directory.to_string_lossy(),"format":"png","steps":[["file.print",{"paper":"a5"}]]})).map_err(|error|error.to_string())?;
        assert_eq!(batch["errors"], json!([]));
        assert_eq!(batch["files"].as_array().ok_or("no batch files")?.len(), 2);
        let output: Value = serde_json::from_str(&fixture.platform.take_json())
            .map_err(|error| error.to_string())?;
        let events = output["events"].as_array().ok_or("no events")?;
        assert_eq!(events.len(), 2);
        assert_ne!(events[0]["documentId"], before.doc.id.0.to_string());
        assert_ne!(events[1]["documentId"], before.doc.id.0.to_string());
        assert_ne!(events[0]["documentId"], events[1]["documentId"]);
        let pdf = |event: &Value| -> Result<Vec<u8>, String> {
            std::fs::read(event["pdfPath"].as_str().ok_or("missing path")?)
                .map_err(|error| error.to_string())
        };
        assert_ne!(pdf(&events[0])?, pdf(&events[1])?);
        fixture.update(&events[0]["id"], "failed", true, "")?;
        fixture.update(&events[1]["id"], "blocked", false, "")?;
        let active = fixture
            .app
            .session
            .active()
            .ok_or("active document missing")?;
        assert_eq!(active.doc, before.doc);
        assert_eq!(
            (active.revision, active.saved_revision, active.path.clone()),
            (before.revision, before.saved_revision, before.path)
        );
        assert!(active.is_dirty());
        Ok(())
    }

    #[test]
    fn failed_rendering_and_bounded_jobs_keep_original_preferences_and_documents()
    -> Result<(), String> {
        let mut fixture = Fixture::new()?;
        fixture
            .app
            .session
            .edit_prefs(|preferences| preferences.script_events.enabled = true);
        assert!(
            fixture
                .app
                .run("file.print", json!({"paper":"unknown"}))
                .is_err()
        );
        assert!(fixture.app.session.prefs().script_events.enabled);
        assert!(fixture.event().is_err());
        let result = fixture.app.run("file.print", json!({}))?;
        assert!(result["warnings"].is_null());
        assert!(
            !fixture
                .app
                .ui
                .status
                .contains("Script Events are unavailable")
        );
        assert!(fixture.app.session.prefs().script_events.enabled);
        fixture
            .app
            .session
            .edit_prefs(|preferences| preferences.script_events.enabled = false);
        for _ in 1..8 {
            fixture.app.run("file.print", json!({}))?;
        }
        assert!(fixture.app.run("file.print", json!({})).is_err());
        let printing = fixture.root.join("PhotoCraft/Documents/Printing");
        assert_eq!(
            std::fs::read_dir(printing)
                .map_err(|error| error.to_string())?
                .count(),
            8
        );
        assert!(
            fixture
                .app
                .session
                .active()
                .ok_or("document lost")?
                .is_dirty()
        );
        Ok(())
    }
}
