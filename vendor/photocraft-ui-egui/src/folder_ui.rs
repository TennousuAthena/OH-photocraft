//! Optional asynchronous folder selection in the original command forms.

use crate::PhotocraftApp;
use serde_json::{Map, Value, json};

#[derive(Clone)]
pub struct Request {
    pub dialog: u64,
    pub generation: u64,
    pub command: String,
    pub field: String,
    pub document: Option<u64>,
    pub original: Value,
}

pub type BrowseFn = Box<dyn FnMut(Request) -> Result<(), String>>;

pub fn recovery_button(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let retained = app.services.folder_recoveries.as_ref().map(|list| list()).unwrap_or_default();
    if retained.is_empty() {
        return;
    }
    let button = ui.button(tl!("Recover folder outputs…"));
    egui::Popup::menu(&button).show(|ui| {
        ui.set_max_width(430.0);
        ui.label(tl!("Retained folder outputs"));
        ui.label(tl!("Retry publishes the retained files without processing them again."));
        for record in retained {
            ui.separator();
            ui.label(if record.unknown_failures {
                crate::i18n::fmt(crate::i18n::t("{label}: {files} retained files"), &[("label", &record.label), ("files", &record.files.to_string())])
            } else {
                crate::i18n::fmt(crate::i18n::t("{label}: {files} files, {failed} inputs failed"), &[("label", &record.label), ("files", &record.files.to_string()), ("failed", &record.failed.to_string())])
            });
            if record.unknown_failures {
                ui.label(tl!("Original input failure details are unavailable."));
            }
            if record.partial {
                ui.label(tl!("External output may already exist. Retrying creates another copy."));
            }
            ui.horizontal(|ui| {
                for (label, choose) in [(tl!("Retry"), false), (tl!("Choose another folder…"), true)] {
                    if ui.button(label).clicked() {
                        let result = app
                            .services
                            .recover_folder
                            .as_mut()
                            .ok_or_else(|| "Folder recovery is unavailable".to_string())
                            .and_then(|recover| recover(record.id, choose));
                        match result {
                            Ok(()) => {
                                app.ui.status = crate::i18n::t("Preparing retained folder output…").into();
                                app.ui.status_error = false;
                                ui.close();
                            }
                            Err(error) => {
                                app.ui.status = error;
                                app.ui.status_error = true;
                            }
                        }
                    }
                }
            });
        }
    });
}

pub fn field(command: &str) -> Option<&'static str> {
    match command {
        "file.scripts.loadFilesIntoStack" => Some("paths"),
        "file.automate.contactSheetII" | "file.scripts.statistics" | "file.scripts.imageProcessor" | "file.automate.batch" | "file.automate.lensCorrection" => {
            Some("input")
        }
        _ => None,
    }
}

pub fn output_command(command: &str) -> bool {
    matches!(command, "file.scripts.imageProcessor" | "file.automate.batch" | "file.automate.lensCorrection")
}

pub fn applies(app: &PhotocraftApp, fields: &Map<String, Value>, key: &str) -> bool {
    (app.services.browse_folder.is_some() && fields.get("__command").and_then(Value::as_str).and_then(field) == Some(key))
        || (app.services.browse_folder_destination.is_some() && key == "output" && fields.get("__command").and_then(Value::as_str).is_some_and(output_command))
}

pub fn generation_key(key: &str) -> &'static str {
    if key == "output" { "__folder_output_generation" } else { "__folder_generation" }
}
pub fn pending_key(key: &str) -> &'static str {
    if key == "output" { "__folder_output_pending" } else { "__folder_pending" }
}
pub fn cancel_key(key: &str) -> &'static str {
    if key == "output" { "__folder_output_cancel" } else { "__folder_cancel" }
}
pub fn progress_key(key: &str) -> &'static str {
    if key == "output" { "__folder_output_progress" } else { "__folder_progress" }
}
pub fn ready_key(key: &str) -> &'static str {
    if key == "output" { "__folder_output_ready" } else { "__folder_ready" }
}

pub fn ready(app: &PhotocraftApp, fields: &Map<String, Value>) -> bool {
    let Some(key) = fields.get("__command").and_then(Value::as_str).and_then(field) else {
        return true;
    };
    let input_ready = !applies(app, fields, key)
        || (fields.get("__folder_pending") != Some(&json!(true))
            && fields.get("__folder_ready") == Some(&json!(true))
            && fields.get(key).and_then(Value::as_array).is_some_and(|files| !files.is_empty()));
    let output_ready = !applies(app, fields, "output")
        || (fields.get("__folder_output_pending") != Some(&json!(true))
            && fields.get("__folder_output_ready") == Some(&json!(true))
            && fields.get("output").and_then(Value::as_str).is_some_and(|handle| !handle.is_empty()));
    input_ready && output_ready
}

fn request_fields(app: &mut PhotocraftApp, dialog: u64, fields: &mut Map<String, Value>, key: &str) -> Result<(), String> {
    let command = fields.get("__command").and_then(Value::as_str).ok_or("This form has no command")?.to_string();
    if !applies(app, fields, key) {
        return Err("This form does not accept a folder".into());
    }
    let generation = fields.get(generation_key(key)).and_then(Value::as_u64).unwrap_or(0).checked_add(1).ok_or("Folder selection generation exhausted")?;
    let request = Request {
        dialog,
        generation,
        command,
        field: key.into(),
        document: app.session.active().map(|doc| doc.doc.id.0),
        original: fields.get(key).cloned().ok_or("The input field is missing")?,
    };
    let hook = if key == "output" { app.services.browse_folder_destination.as_mut() } else { app.services.browse_folder.as_mut() };
    hook.ok_or("Folder selection is unavailable")?(request)?;
    fields.insert(generation_key(key).into(), json!(generation));
    fields.insert(pending_key(key).into(), json!(true));
    fields.remove(cancel_key(key));
    fields.remove(progress_key(key));
    Ok(())
}

/// The same operation used by the form's Browse button and integration tests.
pub fn request(app: &mut PhotocraftApp, dialog: u64) -> Result<(), String> {
    let mut fields = app.ui.dialogs.iter().find(|d| d.id == dialog).ok_or("The input form is closed")?.fields.clone();
    let key = fields.get("__command").and_then(Value::as_str).and_then(field).ok_or("This form does not accept a folder")?;
    request_fields(app, dialog, &mut fields, key)?;
    app.ui.dialog_mut(dialog).ok_or("The input form is closed")?.fields = fields;
    Ok(())
}

pub fn request_destination(app: &mut PhotocraftApp, dialog: u64) -> Result<(), String> {
    let mut fields = app.ui.dialogs.iter().find(|d| d.id == dialog).ok_or("The input form is closed")?.fields.clone();
    request_fields(app, dialog, &mut fields, "output")?;
    app.ui.dialog_mut(dialog).ok_or("The input form is closed")?.fields = fields;
    Ok(())
}

pub fn row(app: &mut PhotocraftApp, ui: &mut egui::Ui, dialog: u64, fields: &mut Map<String, Value>, key: &str) {
    let pending = fields.get(pending_key(key)) == Some(&json!(true));
    let count = fields.get(key).and_then(Value::as_array).map_or(0, Vec::len);
    ui.horizontal(|ui| {
        if pending {
            if let Some(progress) = fields.get(progress_key(key)).and_then(Value::as_str) {
                ui.label(progress);
            } else {
                ui.label(tl!("Selecting folder…"));
            }
            if crate::widgets::secondary_button(ui, tl!("Cancel"), 72.0).clicked() {
                fields.insert(cancel_key(key).into(), json!(true));
            }
        } else {
            ui.label(if key == "output" && fields.get(ready_key(key)) == Some(&json!(true)) {
                tl!("Selected output folder").to_string()
            } else if count == 0 {
                tl!("Choose a folder").to_string()
            } else {
                crate::i18n::fmt(crate::i18n::t("{count} image files"), &[("count", &count.to_string())])
            });
            if crate::widgets::secondary_button(ui, tl!("Browse…"), 84.0).clicked()
                && let Err(error) = request_fields(app, dialog, fields, key)
            {
                app.ui.status = error;
                app.ui.status_error = true;
            }
        }
    });
}

pub fn has_progress(app: &mut PhotocraftApp) -> bool {
    app.services.folder_progress.as_mut().is_some_and(|progress| progress(false).is_some())
}

pub fn status_progress(app: &mut PhotocraftApp, ui: &mut egui::Ui) -> bool {
    let Some(label) = app.services.folder_progress.as_mut().and_then(|progress| progress(false)) else {
        return false;
    };
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
    if crate::widgets::secondary_button(ui, tl!("Cancel"), 72.0).clicked()
        && let Some(progress) = app.services.folder_progress.as_mut()
    {
        let _ = progress(true);
    }
    ui.label(label);
    true
}
