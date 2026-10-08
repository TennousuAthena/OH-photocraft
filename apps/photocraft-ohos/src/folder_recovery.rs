//! Durable, content-checked publication recovery. No command or encoder is invoked here.
use super::{
    Job, MAX_BYTES, Owner, Stage, command_label, count, publication_name, read_small,
    validate_handle,
};
use crate::folder_requests::{absolute_path, inspect, owned_job, plain_directory};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Record {
    version: u32,
    pub allocation: u64,
    pub command: String,
    pub target: String,
    pub handle: String,
    pub owner: PathBuf,
    pub tree_name: String,
    pub failed: usize,
    pub partial: bool,
    #[serde(default)]
    pub legacy: bool,
    entries: Vec<Entry>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    path: String,
    kind: String,
    bytes: u64,
    digest: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Journal {
    version: u32,
    id: u64,
    target: String,
    phase: String,
    owner_root: String,
    stage_root: String,
    publication_name: String,
    destination_uri: String,
    partial_possible: bool,
    file_count: u64,
    total_bytes: u64,
}

impl Record {
    pub fn capture(job: &Job) -> Result<Self, String> {
        let stage = job
            .stage
            .as_ref()
            .ok_or("Folder output has no retained material")?;
        let result = job
            .result
            .as_ref()
            .ok_or("Folder output has no processing result")?;
        Ok(Self {
            version: 1,
            allocation: job.allocation,
            command: job.command.clone(),
            target: job.target.clone(),
            handle: job.handle.clone(),
            owner: stage.owner.clone(),
            tree_name: tree_name(&stage.tree)?,
            failed: count(result, "errors"),
            partial: false,
            legacy: false,
            entries: entries(&stage.tree)?,
        })
    }

    pub fn summary(&self) -> photocraft_ui_egui::FolderRecovery {
        photocraft_ui_egui::FolderRecovery {
            id: self.allocation,
            label: command_label(&self.command).into(),
            files: self
                .entries
                .iter()
                .filter(|entry| entry.kind == "file")
                .count(),
            failed: self.failed,
            partial: self.partial,
            unknown_failures: self.legacy,
        }
    }

    pub fn store(&self) -> Result<(), String> {
        plain_directory(&self.owner)?;
        let path = self.owner.join("recovery.json");
        if std::fs::symlink_metadata(&path)
            .is_ok_and(|meta| !meta.is_file() || meta.file_type().is_symlink())
        {
            return Err("Folder recovery record has invalid ownership".into());
        }
        let bytes = serde_json::to_vec(self).map_err(|_| "Cannot encode folder recovery record")?;
        if bytes.len() > 1_048_576 {
            return Err("Folder recovery record is too large".into());
        }
        photocraft_format::atomic_write(&path, &bytes)
            .map_err(|_| "Cannot retain folder recovery record".to_string())
    }

    pub fn stage(&self) -> Stage {
        Stage {
            owner: self.owner.clone(),
            tree: self.owner.join(&self.tree_name),
            journal: self.owner.join("publication.json"),
        }
    }

    pub fn validate(&mut self, root: &Path) -> Result<(), String> {
        if self.version != 1
            || !photocraft_ui_egui::folder_ui::output_command(&self.command)
            || self.target.is_empty()
            || self.target.len() > 512
            || self.target.contains('\0')
            || self.failed > 500
            || self.entries.len() > 4096
            || self.allocation > 9_007_199_254_740_991
        {
            return Err("Folder recovery record is invalid".into());
        }
        if !self.handle.is_empty() || !self.legacy {
            validate_handle(&self.handle)?;
        }
        owned_job(
            root,
            self.owner
                .to_str()
                .ok_or("Folder recovery ownership is invalid")?,
            self.allocation,
        )?;
        let marker: Owner = serde_json::from_slice(&read_small(&self.owner.join("owner.json"))?)
            .map_err(|_| "Folder recovery ownership is invalid")?;
        if marker.version != 1
            || marker.creation_id != self.allocation
            || marker.target != self.target
            || marker.owner_root != self.owner.to_string_lossy()
        {
            return Err("Folder recovery ownership does not match the retained task".into());
        }
        let journal: Journal =
            serde_json::from_slice(&read_small(&self.owner.join("publication.json"))?)
                .map_err(|_| "Folder publication journal is invalid")?;
        if journal.version != 1
            || journal.id > 9_007_199_254_740_991
            || journal.target != self.target
            || journal.owner_root != self.owner.to_string_lossy()
            || journal.destination_uri.len() > 8192
            || journal.destination_uri.contains('\0')
            || journal.file_count > 500
            || journal.total_bytes > MAX_BYTES
            || !matches!(
                journal.phase.as_str(),
                "prepared" | "renamed" | "copying" | "complete" | "failed" | "cancelled"
            )
        {
            return Err("Folder publication journal does not match the retained task".into());
        }
        absolute_path(&journal.stage_root)?;
        let previous = Path::new(&journal.stage_root);
        if previous.parent() != Some(self.owner.as_path()) {
            return Err("Folder publication tree is outside its task".into());
        }
        tree_name(previous)?;
        // Resolve only the two explicit journal paths. Never infer a root from a directory listing.
        let actual = if journal.publication_name.is_empty() {
            previous.to_path_buf()
        } else {
            if !publication_name(&journal.publication_name) {
                return Err("Folder publication name is invalid".into());
            }
            let renamed = self.owner.join(&journal.publication_name);
            if previous == renamed {
                renamed
            } else {
                let old_exists = std::fs::symlink_metadata(previous).is_ok();
                let new_exists = std::fs::symlink_metadata(&renamed).is_ok();
                match (old_exists, new_exists) {
                    (true, false) => previous.to_path_buf(),
                    (false, true) => renamed,
                    _ => {
                        return Err(
                            "Retained output location needs inspection; files were preserved"
                                .into(),
                        );
                    }
                }
            }
        };
        plain_directory(&actual)?;
        if entries(&actual)? != self.entries {
            return Err(
                "Retained output changed; recovery refused to publish different bytes".into(),
            );
        }
        self.tree_name = tree_name(&actual)?;
        self.partial |=
            journal.partial_possible || matches!(journal.phase.as_str(), "copying" | "complete");
        Ok(())
    }

    pub fn job(&self) -> Job {
        let stage = self.stage();
        let files: Vec<Value> = self
            .entries
            .iter()
            .filter(|entry| entry.kind == "file")
            .map(|entry| json!(stage.tree.join(&entry.path)))
            .collect();
        // Preserve the failed-input count, without persisting private input paths in messages.
        let errors: Vec<Value> = (0..self.failed)
            .map(|_| json!({"error":"Input processing failed in the original task"}))
            .collect();
        Job {
            command: self.command.clone(),
            allocation: self.allocation,
            attempt: None,
            target: self.target.clone(),
            handle: self.handle.clone(),
            params: json!({}),
            stage: Some(stage),
            result: Some(json!({"files":files,"errors":errors})),
            publishing: false,
            cancelled: false,
            cancel_sent: false,
            progress: "Publishing retained folder output…".into(),
            recovery: Some(self.clone()),
            engine_job: None,
        }
    }
}

pub(super) fn discover(root: &Path) -> (Vec<Record>, usize) {
    let mut records = Vec::new();
    let mut invalid = 0;
    let Ok(directories) = std::fs::read_dir(root) else {
        return (records, 1);
    };
    for directory in directories.take(129) {
        if records.len() + invalid >= 128 {
            invalid += 1;
            break;
        }
        let Ok(directory) = directory else {
            invalid += 1;
            continue;
        };
        let path = directory.path().join("recovery.json");
        let record: Result<Record, String> = (|| {
            plain_directory(&directory.path())?;
            if std::fs::symlink_metadata(&path).is_err() {
                return legacy(root, &directory.path());
            }
            let mut record: Record = serde_json::from_slice(&read_record(&path)?)
                .map_err(|_| "Folder recovery record is damaged")?;
            if record.owner != directory.path() {
                return Err("Folder recovery owner is invalid".into());
            }
            record.validate(root)?;
            Ok(record)
        })();
        match record {
            Ok(record) => records.push(record),
            Err(_) => invalid += 1,
        }
    }
    (records, invalid)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    version: u32,
    entries: Vec<ManifestEntry>,
    file_count: u64,
    total_bytes: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ManifestEntry {
    relative_path: String,
    kind: String,
    bytes: u64,
}

fn legacy(root: &Path, owner: &Path) -> Result<Record, String> {
    let marker: Owner = serde_json::from_slice(&read_small(&owner.join("owner.json"))?)
        .map_err(|_| "Folder recovery ownership is invalid")?;
    owned_job(
        root,
        owner
            .to_str()
            .ok_or("Folder recovery ownership is invalid")?,
        marker.creation_id,
    )?;
    let journal: Journal = serde_json::from_slice(&read_small(&owner.join("publication.json"))?)
        .map_err(|_| "Folder publication journal is invalid")?;
    let manifest: Manifest = serde_json::from_slice(&read_record(&owner.join("manifest.json"))?)
        .map_err(|_| "Original folder output manifest is unavailable")?;
    if manifest.version != 1
        || manifest.entries.len() > 4096
        || manifest.file_count == 0
        || manifest.file_count > 500
        || manifest.total_bytes > MAX_BYTES
    {
        return Err("Original folder output manifest is invalid".into());
    }
    let tree =
        if !journal.publication_name.is_empty() && owner.join(&journal.publication_name).exists() {
            owner.join(&journal.publication_name)
        } else {
            PathBuf::from(&journal.stage_root)
        };
    if tree.parent() != Some(owner) {
        return Err("Original folder output has invalid ownership".into());
    }
    let command = [
        "file.automate.batch",
        "file.automate.lensCorrection",
        "file.scripts.imageProcessor",
    ]
    .into_iter()
    .find(|command| marker.target.contains(command))
    .unwrap_or("file.scripts.imageProcessor");
    let mut record = Record {
        version: 1,
        allocation: marker.creation_id,
        command: command.into(),
        target: marker.target,
        handle: String::new(),
        owner: owner.to_path_buf(),
        tree_name: tree_name(&tree)?,
        failed: 0,
        partial: true,
        legacy: true,
        entries: entries(&tree)?,
    };
    let mut expected: Vec<_> = manifest
        .entries
        .into_iter()
        .map(|entry| (entry.relative_path, entry.kind, entry.bytes))
        .collect();
    expected.sort();
    let mut actual: Vec<_> = record
        .entries
        .iter()
        .map(|entry| (entry.path.clone(), entry.kind.clone(), entry.bytes))
        .collect();
    actual.sort();
    if actual != expected
        || actual.iter().filter(|(_, kind, _)| kind == "file").count() as u64 != manifest.file_count
        || actual.iter().map(|(_, _, bytes)| bytes).sum::<u64>() != manifest.total_bytes
    {
        return Err("Original retained output no longer matches its recorded manifest".into());
    }
    // Older packages recorded tree layout and sizes, but no content hashes or output
    // handle. Capture the retained bytes now; explicit reauthorization is required.
    record.validate(root)?;
    record.store()?;
    Ok(record)
}

fn read_record(path: &Path) -> Result<Vec<u8>, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|_| "Cannot read folder recovery record")?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 1_048_576 {
        return Err("Folder recovery record is invalid".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| "Cannot read folder recovery record")?
        .take(1_048_577)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read folder recovery record")?;
    if bytes.len() > 1_048_576 {
        return Err("Folder recovery record is too large".into());
    }
    Ok(bytes)
}

fn tree_name(tree: &Path) -> Result<String, String> {
    let name = tree
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("Folder output name is invalid")?;
    if name != "contents" && !publication_name(name) {
        return Err("Folder output name is invalid".into());
    }
    Ok(name.into())
}

fn entries(tree: &Path) -> Result<Vec<Entry>, String> {
    inspect(tree)?
        .into_iter()
        .map(|(path, (kind, bytes))| {
            let digest = if kind == "file" {
                let mut file = std::fs::File::open(tree.join(&path))
                    .map_err(|_| "Cannot verify retained output")?;
                let mut hasher = blake3::Hasher::new();
                let mut buffer = [0_u8; 65536];
                let mut total = 0_u64;
                loop {
                    let read = file
                        .read(&mut buffer)
                        .map_err(|_| "Cannot verify retained output")?;
                    if read == 0 {
                        break;
                    }
                    total = total
                        .checked_add(read as u64)
                        .filter(|total| *total <= bytes)
                        .ok_or("Retained output changed during verification")?;
                    hasher.update(buffer.get(..read).ok_or("Cannot verify retained output")?);
                }
                if total != bytes {
                    return Err("Retained output changed during verification".into());
                }
                hasher.finalize().to_hex().to_string()
            } else {
                String::new()
            };
            Ok(Entry {
                path,
                kind,
                bytes,
                digest,
            })
        })
        .collect()
}
