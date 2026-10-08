//! Authorized directory copies update a captured original form, never execute it.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use photocraft_ui_egui::{PhotocraftApp, Services, folder_ui, i18n};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::file_requests::FileRequests;
use crate::platform::Platform;

const MAX_BYTES: u64 = 1_073_741_824;
const MAX_MANIFEST: u64 = 1_048_576;

#[derive(Clone)]
pub struct FolderRequests {
    root: PathBuf,
    platform: Platform,
    files: FileRequests,
    pending: Rc<RefCell<Option<Pending>>>,
}

struct Pending {
    id: u64,
    target: String,
    form: folder_ui::Request,
    cancelled: bool,
    cancel_sent: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Manifest {
    version: u32,
    entries: Vec<Entry>,
    file_count: usize,
    total_bytes: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Entry {
    relative_path: String,
    kind: String,
    bytes: u64,
}

impl FolderRequests {
    pub fn new(files: &Path, platform: &Platform, requests: &FileRequests) -> Result<Self, String> {
        if !files.is_absolute() {
            return Err("Folder imports require application storage".into());
        }
        let mut root = files.to_path_buf();
        plain_directory(&root)?;
        for name in ["PhotoCraft", "Documents", "folder-imports"] {
            root.push(name);
            if !root.exists() {
                std::fs::create_dir(&root).map_err(|_| "Cannot prepare folder import storage")?;
            }
            plain_directory(&root)?;
        }
        Ok(Self {
            root: root
                .canonicalize()
                .map_err(|_| "Cannot prepare folder import storage")?,
            platform: platform.clone(),
            files: requests.clone(),
            pending: Rc::new(RefCell::new(None)),
        })
    }

    pub fn attach(&self, services: &mut Services) {
        let folders = self.clone();
        services.browse_folder = Some(Box::new(move |form| folders.begin(form)));
    }

    fn begin(&self, form: folder_ui::Request) -> Result<(), String> {
        if self.pending.borrow().is_some() || self.files.is_pending() {
            return Err("Finish the current file or folder selection first".into());
        }
        if folder_ui::field(&form.command) != Some(form.field.as_str()) {
            return Err("This form does not accept a folder".into());
        }
        let id = crate::platform::next_id();
        let target = format!(
            "form:{}:{}:{}:{}:{:?}",
            form.dialog, form.generation, form.command, form.field, form.document
        );
        self.platform.acquire_folder(id)?;
        if let Err(error) = self.platform.send_event(json!({"kind":"folderImport","id":id,"target":target,"intent":form.command,
            "scope":"topLevel","copyLayout":"sourceBasenameChild","limits":{"maxFiles":500,"maxBytes":MAX_BYTES}})) {
            self.platform.release_folder(id);
            return Err(error);
        }
        *self.pending.borrow_mut() = Some(Pending {
            id,
            target,
            form,
            cancelled: false,
            cancel_sent: false,
        });
        Ok(())
    }

    pub fn finish_frame(&self, app: &mut PhotocraftApp) {
        let mut state = self.pending.borrow_mut();
        let Some(pending) = state.as_mut() else {
            return;
        };
        let cancel = app
            .ui
            .dialogs
            .iter()
            .find(|dialog| dialog.id == pending.form.dialog)
            .is_some_and(|dialog| dialog.fields.get("__folder_cancel") == Some(&json!(true)));
        if cancel || !target_valid(app, &pending.form) {
            pending.cancelled = true;
        }
        if pending.cancelled && !pending.cancel_sent {
            match self
                .platform
                .send_event(json!({"kind":"folderCancel","id":pending.id,"target":pending.target}))
            {
                Ok(()) => {
                    pending.cancel_sent = true;
                    release_form(app, &pending.form);
                    if cancel {
                        app.ui.status = i18n::t("Folder import cancellation requested").into();
                        app.ui.status_error = false;
                    }
                }
                Err(error) => {
                    app.ui.status = error;
                    app.ui.status_error = true;
                }
            }
        }
    }

    pub fn apply(&self, packet: Value, app: &mut PhotocraftApp) -> Result<(), String> {
        let id = packet["id"]
            .as_u64()
            .filter(|id| *id <= 9_007_199_254_740_991)
            .ok_or("Invalid folder request ID")?;
        let target = packet["target"].as_str().ok_or("Missing folder target")?;
        let mut state = self.pending.borrow_mut();
        let Some(pending) = state
            .as_ref()
            .filter(|pending| pending.id == id && pending.target == target)
        else {
            // A duplicate must not delete a previously accepted tree still used by a form/action.
            return Ok(());
        };
        if packet["kind"] == "folderProgress" {
            if !pending.cancelled && target_valid(app, &pending.form) {
                let done = bounded(&packet, "processedBytes", MAX_BYTES)?;
                let total = bounded(&packet, "totalBytes", MAX_BYTES)?;
                bounded(&packet, "filesDone", 500)?;
                if let Some(dialog) = app.ui.dialog_mut(pending.form.dialog) {
                    dialog.fields.insert(
                        "__folder_progress".into(),
                        json!(i18n::fmt(
                            i18n::t("Importing folder: {done} / {total} MiB"),
                            &[
                                ("done", &(done / 1_048_576).to_string()),
                                ("total", &(total / 1_048_576).to_string()),
                            ],
                        )),
                    );
                }
            }
            return Ok(());
        }
        if packet["kind"] != "folderComplete" {
            return Err("Invalid folder response".into());
        }
        let result = packet["result"]
            .as_str()
            .filter(|result| matches!(*result, "success" | "cancel" | "error"))
            .ok_or("Invalid folder completion result")?;
        let pending = state.take().ok_or("Folder request is no longer pending")?;
        drop(state);
        self.platform.release_folder(id);
        let applicable = !pending.cancelled && target_valid(app, &pending.form);
        release_form(app, &pending.form);
        let owner_text = packet["ownerRoot"]
            .as_str()
            .ok_or("Missing folder ownership")?;
        let owner = if owner_text.is_empty() {
            None
        } else {
            Some(self.owner(owner_text, id)?)
        };
        if !applicable || result != "success" {
            if let Some(owner) = owner {
                remove_owner(&owner)?;
            }
            if applicable {
                app.ui.status = if result == "cancel" {
                    i18n::t("Folder selection cancelled").into()
                } else {
                    friendly_error(&packet)
                };
                app.ui.status_error = result == "error";
            }
            return Ok(());
        }
        let owner = owner.ok_or("Folder import did not provide owned storage")?;
        let files = self.validate(&owner, &packet);
        let files = match files {
            Ok(files) => files,
            Err(error) => {
                remove_owner(&owner)?;
                return Err(error);
            }
        };
        let count = files.len();
        let dialog = app
            .ui
            .dialog_mut(pending.form.dialog)
            .ok_or("The input form is closed")?;
        dialog.fields.insert(pending.form.field, json!(files));
        dialog.fields.insert("__folder_ready".into(), json!(true));
        app.ui.status = i18n::fmt(
            i18n::t("Selected {count} image files"),
            &[("count", &count.to_string())],
        );
        app.ui.status_error = false;
        Ok(())
    }

    fn owner(&self, text: &str, id: u64) -> Result<PathBuf, String> {
        owned_job(&self.root, text, id)
    }

    fn validate(&self, owner: &Path, packet: &Value) -> Result<Vec<String>, String> {
        if packet["copyLayout"] != "sourceBasenameChild" {
            return Err("Unsupported folder copy layout".into());
        }
        let container = owner.join("contents");
        plain_directory(&container)?;
        absolute_path(
            packet["resolvedRoot"]
                .as_str()
                .ok_or("Missing imported folder")?,
        )?;
        let root = PathBuf::from(
            packet["resolvedRoot"]
                .as_str()
                .ok_or("Missing imported folder")?,
        );
        if root.parent() != Some(container.as_path()) || root.file_name().is_none() {
            return Err("Imported folder layout does not match the copy contract".into());
        }
        plain_directory(&root)?;
        if root
            .canonicalize()
            .map_err(|_| "Cannot verify imported folder")?
            != root
        {
            return Err("Imported folder path is invalid".into());
        }
        let children = std::fs::read_dir(&container)
            .map_err(|_| "Cannot verify folder layout")?
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(|_| "Cannot verify folder layout")?;
        if children.len() != 1 || children.first().is_none_or(|child| child.path() != root) {
            return Err("Imported folder layout does not match the copy contract".into());
        }
        let manifest_path = owner.join("manifest.json");
        if packet["manifestPath"].as_str() != manifest_path.to_str()
            || packet["bindingId"].as_str() != owner.file_name().and_then(|name| name.to_str())
        {
            return Err("Folder manifest ownership is invalid".into());
        }
        let metadata =
            std::fs::symlink_metadata(&manifest_path).map_err(|_| "Cannot read folder manifest")?;
        if !metadata.is_file() || metadata.len() > MAX_MANIFEST {
            return Err("Folder manifest is invalid or too large".into());
        }
        let mut bytes = Vec::new();
        std::fs::File::open(&manifest_path)
            .map_err(|_| "Cannot read folder manifest")?
            .take(MAX_MANIFEST + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "Cannot read folder manifest")?;
        if bytes.len() > MAX_MANIFEST as usize {
            return Err("Folder manifest is too large".into());
        }
        let manifest: Manifest =
            serde_json::from_slice(&bytes).map_err(|_| "Folder manifest is malformed")?;
        if manifest.version != 1
            || manifest.entries.len() > 4096
            || manifest.file_count > 500
            || manifest.total_bytes > MAX_BYTES
        {
            return Err("Folder manifest exceeds supported limits".into());
        }
        let mut expected = BTreeMap::new();
        for entry in manifest.entries {
            relative_path(&entry.relative_path)?;
            if !matches!(entry.kind.as_str(), "file" | "directory")
                || (entry.kind == "directory" && entry.bytes != 0)
                || expected
                    .insert(entry.relative_path, (entry.kind, entry.bytes))
                    .is_some()
            {
                return Err("Folder manifest contains invalid or duplicate entries".into());
            }
        }
        let actual = inspect(&root)?;
        let count = actual.values().filter(|(kind, _)| kind == "file").count();
        let total = actual
            .values()
            .try_fold(0_u64, |sum, (_, bytes)| sum.checked_add(*bytes))
            .ok_or("Folder size exceeds supported limits")?;
        if actual != expected || count != manifest.file_count || total != manifest.total_bytes {
            return Err("Folder manifest does not match the copied files".into());
        }
        let files = photocraft_engine::file_cmds::list_images(
            root.to_str().ok_or("Imported folder name is invalid")?,
        )
        .map_err(|_| "Cannot read the imported image files")?;
        if files.is_empty() {
            return Err("The selected folder has no supported images at its top level".into());
        }
        Ok(files)
    }
}

pub(crate) fn owned_job(root: &Path, text: &str, id: u64) -> Result<PathBuf, String> {
    absolute_path(text)?;
    let owner = PathBuf::from(text);
    let relative = owner
        .strip_prefix(root)
        .map_err(|_| "Folder storage ownership is invalid")?;
    let mut parts = relative.components();
    let name = match (parts.next(), parts.next()) {
        (Some(Component::Normal(name)), None) => name.to_str(),
        _ => None,
    }
    .ok_or("Folder storage ownership is invalid")?;
    let random = name
        .strip_prefix(&format!("{id}-"))
        .ok_or("Folder storage does not belong to this request")?;
    if random.len() != 6 || !random.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err("Folder storage ownership is invalid".into());
    }
    plain_directory(root)?;
    plain_directory(&owner)?;
    if owner
        .canonicalize()
        .map_err(|_| "Cannot verify folder storage")?
        != owner
    {
        return Err("Folder storage ownership is invalid".into());
    }
    Ok(owner)
}

pub(crate) fn target_valid(app: &PhotocraftApp, form: &folder_ui::Request) -> bool {
    app.session.active().map(|doc| doc.doc.id.0) == form.document
        && app.ui.dialogs.iter().any(|dialog| {
            dialog.id == form.dialog
                && dialog.fields.get("__command").and_then(Value::as_str)
                    == Some(form.command.as_str())
                && dialog
                    .fields
                    .get(folder_ui::generation_key(&form.field))
                    .and_then(Value::as_u64)
                    == Some(form.generation)
                && dialog.fields.get(&form.field) == Some(&form.original)
        })
}

pub(crate) fn release_form(app: &mut PhotocraftApp, form: &folder_ui::Request) {
    if let Some(dialog) = app.ui.dialog_mut(form.dialog)
        && dialog
            .fields
            .get(folder_ui::generation_key(&form.field))
            .and_then(Value::as_u64)
            == Some(form.generation)
    {
        dialog
            .fields
            .insert(folder_ui::pending_key(&form.field).into(), json!(false));
        dialog.fields.remove(folder_ui::cancel_key(&form.field));
        dialog.fields.remove(folder_ui::progress_key(&form.field));
    }
}

fn bounded(packet: &Value, field: &str, max: u64) -> Result<u64, String> {
    packet[field]
        .as_u64()
        .filter(|value| *value <= max)
        .ok_or_else(|| "Folder progress exceeds supported limits".into())
}

pub(crate) fn plain_directory(path: &Path) -> Result<(), String> {
    if std::fs::symlink_metadata(path)
        .map_err(|_| "Cannot inspect folder storage")?
        .file_type()
        .is_dir()
    {
        Ok(())
    } else {
        Err("Folder storage contains a link or invalid directory".into())
    }
}

pub(crate) fn absolute_path(path: &str) -> Result<(), String> {
    if !path.starts_with('/')
        || path.contains(['\\', '\0'])
        || path
            .split('/')
            .skip(1)
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        Err("Folder storage path is invalid".into())
    } else {
        Ok(())
    }
}

fn relative_path(path: &str) -> Result<(), String> {
    let parts: Vec<_> = path.split('/').collect();
    if path.is_empty()
        || path.len() > 4096
        || path.contains(['\\', '\0'])
        || parts.len() > 32
        || parts
            .iter()
            .any(|part| part.is_empty() || matches!(*part, "." | ".."))
    {
        Err("Folder manifest contains an unsafe path".into())
    } else {
        Ok(())
    }
}

pub(crate) fn inspect(root: &Path) -> Result<BTreeMap<String, (String, u64)>, String> {
    let mut result = BTreeMap::new();
    let mut directories = vec![root.to_path_buf()];
    let mut files = 0_usize;
    let mut total = 0_u64;
    while let Some(directory) = directories.pop() {
        plain_directory(&directory)?;
        for entry in std::fs::read_dir(directory).map_err(|_| "Cannot inspect the copied folder")? {
            let path = entry
                .map_err(|_| "Cannot inspect the copied folder")?
                .path();
            let relative = path
                .strip_prefix(root)
                .ok()
                .and_then(Path::to_str)
                .ok_or("Copied folder name is invalid")?;
            relative_path(relative)?;
            let metadata =
                std::fs::symlink_metadata(&path).map_err(|_| "Cannot inspect the copied folder")?;
            let (kind, bytes) = if metadata.file_type().is_dir() {
                directories.push(path.clone());
                ("directory", 0)
            } else if metadata.file_type().is_file() {
                files += 1;
                total = total
                    .checked_add(metadata.len())
                    .ok_or("Folder size exceeds supported limits")?;
                ("file", metadata.len())
            } else {
                return Err("Copied folder contains a link or special file".into());
            };
            result.insert(relative.to_string(), (kind.to_string(), bytes));
            if result.len() > 4096 || files > 500 || total > MAX_BYTES {
                return Err("Copied folder exceeds supported limits".into());
            }
        }
    }
    Ok(result)
}

pub(crate) fn remove_owner(owner: &Path) -> Result<(), String> {
    // Rust's remove_dir_all does not follow symlinks. Only a verified exclusive
    // job is removed, after the SDK copy has settled and handed it over.
    std::fs::remove_dir_all(owner).map_err(|_| "Folder import draft could not be cleaned up".into())
}

fn friendly_error(packet: &Value) -> String {
    let message = packet["error"]
        .as_str()
        .filter(|message| {
            !message.is_empty() && message.len() <= 256 && !message.contains(['/', '\\'])
        })
        .unwrap_or_else(|| i18n::t("Folder import failed; choose the folder again or retry"));
    i18n::t(message).to_string()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use eframe::App;
    use photocraft_engine::Session;

    pub(crate) struct Fixture {
        pub root: PathBuf,
        pub app: PhotocraftApp,
        pub folders: FolderRequests,
        pub platform: Platform,
        pub ctx: egui::Context,
    }

    impl Fixture {
        pub fn new() -> Result<Self, String> {
            let root = std::env::temp_dir()
                .join(format!("photocraft-folder-{}", crate::platform::next_id()));
            let files = FileRequests::new(&root)?;
            let platform = Platform::new(&root.join("cache"))?;
            let folders = FolderRequests::new(&root, &platform, &files)?;
            let mut hooks = crate::services::services_with_files(&root, &files);
            folders.attach(&mut hooks);
            let mut app = PhotocraftApp::new(Session::new(), hooks);
            app.run(
                "file.new",
                json!({"width":2,"height":2,"name":"Original dirty.psd"}),
            )?;
            app.run("layer.new.layer", json!({}))?;
            let ctx = egui::Context::default();
            PhotocraftApp::setup_context(&ctx, photocraft_ui_egui::theme::ThemeKind::Pro);
            Ok(Self {
                root,
                app,
                folders,
                platform,
                ctx,
            })
        }

        pub fn browse(&mut self, command: &str) -> Result<(u64, Value), String> {
            let dialog = photocraft_ui_egui::menus::invoke(
                &mut self.app,
                &self.ctx,
                command,
                json!({}),
            )?["dialog"]
                .as_u64()
                .ok_or("missing original form")?;
            folder_ui::request(&mut self.app, dialog)?;
            let event = serde_json::from_str::<Value>(&self.platform.take_json())
                .map_err(|error| error.to_string())?["events"]
                .as_array()
                .and_then(|events| events.iter().find(|event| event["kind"] == "folderImport"))
                .cloned()
                .ok_or("missing folder request")?;
            assert_eq!(event["intent"], command);
            assert_eq!(event["scope"], "topLevel");
            assert_eq!(event["copyLayout"], "sourceBasenameChild");
            Ok((dialog, event))
        }

        pub fn copied(&self, request: &Value) -> Result<Value, String> {
            let id = request["id"].as_u64().ok_or("missing request id")?;
            let owner = self.folders.root.join(format!("{id}-ABC123"));
            let actual = owner.join("contents/Fixture input");
            std::fs::create_dir_all(actual.join("nested")).map_err(|error| error.to_string())?;
            std::fs::create_dir(actual.join("empty")).map_err(|error| error.to_string())?;
            let mut entries = vec![
                json!({"relativePath":"nested","kind":"directory","bytes":0}),
                json!({"relativePath":"empty","kind":"directory","bytes":0}),
            ];
            let mut total = 0;
            for (name, rgba) in [
                ("b.PNG", [255, 0, 0, 255]),
                ("a.png", [0, 0, 255, 255]),
                ("中.png", [0, 255, 0, 255]),
                ("nested/ignored.png", [255, 0, 255, 255]),
            ] {
                let bytes = self
                    .app
                    .services
                    .encode_png
                    .as_ref()
                    .ok_or("missing PNG encoder")?(1, 1, &rgba)?;
                std::fs::write(actual.join(name), &bytes).map_err(|error| error.to_string())?;
                total += bytes.len();
                entries.push(json!({"relativePath":name,"kind":"file","bytes":bytes.len()}));
            }
            std::fs::write(actual.join("marker.txt"), b"keep")
                .map_err(|error| error.to_string())?;
            entries.push(json!({"relativePath":"marker.txt","kind":"file","bytes":4}));
            let manifest = owner.join("manifest.json");
            std::fs::write(
                &manifest,
                json!({"version":1,"entries":entries,"fileCount":5,"totalBytes":total+4})
                    .to_string(),
            )
            .map_err(|error| error.to_string())?;
            Ok(
                json!({"kind":"folderComplete","id":id,"target":request["target"],"result":"success",
                "copyLayout":"sourceBasenameChild","ownerRoot":owner,"resolvedRoot":actual,"manifestPath":manifest,
                "bindingId":owner.file_name().and_then(|name| name.to_str()),"error":""}),
            )
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn original_load_stack_confirms_sorted_top_level_pngs_only_after_folder_selection()
    -> Result<(), String> {
        let mut fixture = Fixture::new()?;
        let original = fixture
            .app
            .session
            .active()
            .ok_or("original missing")?
            .clone();
        let journal = fixture.app.session.journal.clone();
        let (dialog, request) = fixture.browse("file.scripts.loadFilesIntoStack")?;
        assert!(photocraft_ui_egui::dialogs::confirm(&mut fixture.app, dialog).is_err());
        assert!(fixture.app.ui.dialogs.iter().any(|form| form.id == dialog));
        let response = fixture.copied(&request)?;
        fixture.folders.apply(response.clone(), &mut fixture.app)?;
        assert_eq!(fixture.app.session.documents().len(), 1);
        assert_eq!(fixture.app.session.journal, journal);
        let after = fixture.app.session.active().ok_or("original missing")?;
        assert_eq!(
            (
                after.doc.clone(),
                after.path.clone(),
                after.revision,
                after.saved_revision
            ),
            (
                original.doc,
                original.path,
                original.revision,
                original.saved_revision
            )
        );
        let files = fixture
            .app
            .ui
            .dialog_mut(dialog)
            .ok_or("original form missing")?
            .fields["paths"]
            .as_array()
            .ok_or("files missing")?
            .clone();
        let names: Vec<_> = files
            .iter()
            .filter_map(Value::as_str)
            .filter_map(|path| Path::new(path).file_name())
            .filter_map(|name| name.to_str())
            .collect();
        assert_eq!(names, ["a.png", "b.PNG", "中.png"]);
        let result = photocraft_ui_egui::dialogs::confirm(&mut fixture.app, dialog)?;
        assert_eq!(result["layers"], 3);
        let document = &fixture.app.session.active().ok_or("stack missing")?.doc;
        assert_eq!(
            document
                .layers
                .iter()
                .map(|layer| layer.name.as_str())
                .collect::<Vec<_>>(),
            names
        );
        assert_eq!(
            document.layers[0]
                .surface()
                .ok_or("first raster missing")?
                .rgba(0, 0),
            [0.0, 0.0, 1.0, 1.0]
        );
        assert_eq!(
            document.layers[1]
                .surface()
                .ok_or("second raster missing")?
                .rgba(0, 0),
            [1.0, 0.0, 0.0, 1.0]
        );
        fixture.folders.apply(response.clone(), &mut fixture.app)?;
        assert!(
            Path::new(response["ownerRoot"].as_str().ok_or("owner missing")?).exists(),
            "duplicate completion cannot delete accepted inputs"
        );
        Ok(())
    }

    #[test]
    fn original_contact_sheet_and_statistics_keep_options_and_create_actual_results()
    -> Result<(), String> {
        for command in ["file.automate.contactSheetII", "file.scripts.statistics"] {
            let mut fixture = Fixture::new()?;
            let (dialog, request) = fixture.browse(command)?;
            let fields = &mut fixture
                .app
                .ui
                .dialog_mut(dialog)
                .ok_or("form missing")?
                .fields;
            if command == "file.automate.contactSheetII" {
                for (key, value) in [
                    ("units", json!("pixels")),
                    ("width", json!(48)),
                    ("height", json!(16)),
                    ("columns", json!(3)),
                    ("rows", json!(1)),
                    ("caption", json!(false)),
                ] {
                    fields.insert(key.into(), value);
                }
            } else {
                fields.insert("mode".into(), json!("mean"));
            }
            let options = fields.clone();
            let response = fixture.copied(&request)?;
            fixture.folders.apply(response, &mut fixture.app)?;
            let current = &fixture
                .app
                .ui
                .dialog_mut(dialog)
                .ok_or("form missing")?
                .fields;
            for (key, value) in options
                .iter()
                .filter(|(key, _)| !key.starts_with("__") && *key != "input")
            {
                assert_eq!(current.get(key), Some(value));
            }
            assert_eq!(fixture.app.session.documents().len(), 1);
            let result = photocraft_ui_egui::dialogs::confirm(&mut fixture.app, dialog)?;
            assert_eq!(result["images"], 3);
            assert_eq!(fixture.app.session.documents().len(), 2);
            let document = &fixture.app.session.active().ok_or("result missing")?.doc;
            if command == "file.automate.contactSheetII" {
                assert_eq!(result["pages"], 1);
                assert_eq!((document.size.width, document.size.height), (48, 16));
                assert_eq!(document.layers.len(), 4);
            } else {
                assert_eq!(result["mode"], "mean");
                assert_eq!(document.layers.len(), 1);
                let photocraft_doc::LayerContent::Smart(smart) = &document.layers[0].content else {
                    return Err("statistics did not create a smart object".into());
                };
                assert_eq!(smart.stack_mode, Some(photocraft_doc::StackMode::Mean));
            }
        }
        Ok(())
    }

    #[test]
    fn cancel_closed_changed_field_generation_and_document_never_execute_late_results()
    -> Result<(), String> {
        for change in ["cancel", "closed", "field", "generation", "document"] {
            let mut fixture = Fixture::new()?;
            let (dialog, request) = fixture.browse("file.scripts.loadFilesIntoStack")?;
            let response = fixture.copied(&request)?;
            match change {
                "closed" => {
                    fixture.app.ui.close_dialog(dialog);
                }
                "document" => {
                    fixture
                        .app
                        .run("file.new", json!({"width":1,"height":1,"name":"second"}))?;
                }
                other => {
                    let fields = &mut fixture
                        .app
                        .ui
                        .dialog_mut(dialog)
                        .ok_or("form missing")?
                        .fields;
                    fields.insert(
                        match other {
                            "field" => "paths",
                            "generation" => "__folder_generation",
                            _ => "__folder_cancel",
                        }
                        .into(),
                        match other {
                            "field" => json!("user replacement"),
                            "generation" => json!(900),
                            _ => json!(true),
                        },
                    );
                }
            }
            let documents = fixture.app.session.documents().len();
            let journal = fixture.app.session.journal.clone();
            fixture.folders.finish_frame(&mut fixture.app);
            let output: Value = serde_json::from_str(&fixture.platform.take_json())
                .map_err(|error| error.to_string())?;
            assert!(
                output["events"]
                    .as_array()
                    .is_some_and(|events| events
                        .iter()
                        .any(|event| event["kind"] == "folderCancel"
                            && event["id"] == request["id"]
                            && event["target"] == request["target"]))
            );
            fixture.folders.apply(response.clone(), &mut fixture.app)?;
            assert_eq!(fixture.app.session.documents().len(), documents);
            assert_eq!(fixture.app.session.journal, journal);
            assert!(!Path::new(response["ownerRoot"].as_str().ok_or("owner missing")?).exists());
        }
        Ok(())
    }

    #[test]
    fn malformed_manifests_and_layouts_never_change_the_original_input_or_execute()
    -> Result<(), String> {
        for corruption in [
            "size",
            "duplicate",
            "traversal",
            "oversized",
            "layout",
            "binding",
            "mode",
        ] {
            let mut fixture = Fixture::new()?;
            let (dialog, request) = fixture.browse("file.scripts.loadFilesIntoStack")?;
            let original = fixture
                .app
                .ui
                .dialog_mut(dialog)
                .ok_or("form missing")?
                .fields["paths"]
                .clone();
            let mut response = fixture.copied(&request)?;
            let path = response["manifestPath"]
                .as_str()
                .ok_or("manifest missing")?
                .to_string();
            let mut manifest: Value =
                serde_json::from_slice(&std::fs::read(&path).map_err(|error| error.to_string())?)
                    .map_err(|error| error.to_string())?;
            match corruption {
                "size" => manifest["totalBytes"] = json!(0),
                "duplicate" => {
                    let duplicate = manifest["entries"][0].clone();
                    manifest["entries"]
                        .as_array_mut()
                        .ok_or("entries missing")?
                        .push(duplicate);
                }
                "traversal" => manifest["entries"][0]["relativePath"] = json!("../escape"),
                "layout" => response["resolvedRoot"] = response["ownerRoot"].clone(),
                "binding" => response["bindingId"] = json!("another-binding"),
                "mode" => response["copyLayout"] = json!("guess-single-child"),
                _ => {}
            }
            std::fs::write(
                &path,
                if corruption == "oversized" {
                    vec![b' '; MAX_MANIFEST as usize + 1]
                } else {
                    manifest.to_string().into_bytes()
                },
            )
            .map_err(|error| error.to_string())?;
            assert!(
                fixture
                    .folders
                    .apply(response.clone(), &mut fixture.app)
                    .is_err()
            );
            assert_eq!(fixture.app.session.documents().len(), 1);
            assert_eq!(
                fixture
                    .app
                    .ui
                    .dialog_mut(dialog)
                    .ok_or("form missing")?
                    .fields["paths"],
                original
            );
            assert!(photocraft_ui_egui::dialogs::confirm(&mut fixture.app, dialog).is_err());
            assert!(!Path::new(response["ownerRoot"].as_str().ok_or("owner missing")?).exists());
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn copied_links_and_forged_owner_roots_cannot_delete_external_files() -> Result<(), String> {
        for forgery in [false, true] {
            let mut fixture = Fixture::new()?;
            let (_dialog, request) = fixture.browse("file.scripts.loadFilesIntoStack")?;
            let mut response = fixture.copied(&request)?;
            let outside = fixture.root.join("outside");
            std::fs::create_dir(&outside).map_err(|error| error.to_string())?;
            std::fs::write(outside.join("sentinel"), b"untouched")
                .map_err(|error| error.to_string())?;
            if forgery {
                response["ownerRoot"] = json!(outside);
            } else {
                let actual =
                    PathBuf::from(response["resolvedRoot"].as_str().ok_or("root missing")?);
                std::os::unix::fs::symlink(&outside, actual.join("link"))
                    .map_err(|error| error.to_string())?;
            }
            assert!(fixture.folders.apply(response, &mut fixture.app).is_err());
            assert_eq!(
                std::fs::read(outside.join("sentinel")).map_err(|error| error.to_string())?,
                b"untouched"
            );
            assert_eq!(fixture.app.session.documents().len(), 1);
        }
        Ok(())
    }

    #[test]
    fn empty_or_nested_only_folders_are_visible_errors_without_new_documents() -> Result<(), String>
    {
        for empty in [false, true] {
            let mut fixture = Fixture::new()?;
            let (dialog, request) = fixture.browse("file.scripts.loadFilesIntoStack")?;
            let response = fixture.copied(&request)?;
            let root = Path::new(response["resolvedRoot"].as_str().ok_or("root missing")?);
            let manifest = Path::new(
                response["manifestPath"]
                    .as_str()
                    .ok_or("manifest missing")?,
            );
            let mut contents: Value = serde_json::from_slice(
                &std::fs::read(manifest).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
            for name in ["a.png", "b.PNG", "中.png"] {
                std::fs::remove_file(root.join(name)).map_err(|error| error.to_string())?;
            }
            let entries = contents["entries"]
                .as_array_mut()
                .ok_or("entries missing")?;
            entries.retain(|entry| {
                !matches!(
                    entry["relativePath"].as_str(),
                    Some("a.png" | "b.PNG" | "中.png")
                )
            });
            if empty {
                for name in ["nested", "empty"] {
                    std::fs::remove_dir_all(root.join(name)).map_err(|error| error.to_string())?;
                }
                std::fs::remove_file(root.join("marker.txt")).map_err(|error| error.to_string())?;
                entries.clear();
            }
            let count = entries
                .iter()
                .filter(|entry| entry["kind"] == "file")
                .count();
            let bytes: u64 = entries
                .iter()
                .filter_map(|entry| entry["bytes"].as_u64())
                .sum();
            contents["fileCount"] = json!(count);
            contents["totalBytes"] = json!(bytes);
            std::fs::write(manifest, contents.to_string()).map_err(|error| error.to_string())?;
            let error = fixture
                .folders
                .apply(response, &mut fixture.app)
                .err()
                .ok_or("folder should be rejected")?;
            assert!(error.contains("no supported images at its top level"));
            assert_eq!(fixture.app.session.documents().len(), 1);
            assert!(
                fixture
                    .app
                    .session
                    .active()
                    .ok_or("original missing")?
                    .is_dirty()
            );
            assert!(photocraft_ui_egui::dialogs::confirm(&mut fixture.app, dialog).is_err());
        }
        Ok(())
    }

    #[test]
    fn original_form_renders_browse_without_sandbox_paths_and_cancel_preserves_old_selection()
    -> Result<(), String> {
        let mut fixture = Fixture::new()?;
        let (dialog, request) = fixture.browse("file.scripts.loadFilesIntoStack")?;
        let response = fixture.copied(&request)?;
        fixture.folders.apply(response, &mut fixture.app)?;
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 800.0),
            )),
            ..Default::default()
        };
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
        let mut visible = String::new();
        for _ in 0..4 {
            let mut logic_done = false;
            let output = fixture.ctx.run_ui(input.clone(), |ui| {
                if !logic_done {
                    fixture.app.logic(ui.ctx(), &mut frame);
                    logic_done = true;
                }
                fixture.app.ui(ui, &mut frame);
            });
            visible.clear();
            for clipped in &output.shapes {
                text(&clipped.shape, &mut visible);
            }
            output.drop_without_applying_deltas();
        }
        assert!(visible.contains("Browse"), "{visible}");
        assert!(visible.contains("3 image files"), "{visible}");
        assert!(!visible.contains("folder-imports"));
        let original = fixture
            .app
            .ui
            .dialog_mut(dialog)
            .ok_or("form missing")?
            .fields["paths"]
            .clone();
        folder_ui::request(&mut fixture.app, dialog)?;
        let request: Value = serde_json::from_str::<Value>(&fixture.platform.take_json())
            .map_err(|error| error.to_string())?["events"][0]
            .clone();
        fixture.folders.apply(json!({"kind":"folderComplete","id":request["id"],"target":request["target"],"result":"cancel","ownerRoot":"","error":""}), &mut fixture.app)?;
        assert_eq!(
            fixture
                .app
                .ui
                .dialog_mut(dialog)
                .ok_or("form missing")?
                .fields["paths"],
            original
        );
        assert!(folder_ui::ready(
            &fixture.app,
            &fixture
                .app
                .ui
                .dialogs
                .iter()
                .find(|form| form.id == dialog)
                .ok_or("form missing")?
                .fields
        ));
        Ok(())
    }
}
