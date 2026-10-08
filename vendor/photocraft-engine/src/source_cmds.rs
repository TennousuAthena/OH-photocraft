//! Optional asynchronous file-source boundary; no platform UI or URI enters L5.

use crate::{EngineError, Result, Session};
use photocraft_doc::{Document, LayerId};
use serde_json::{Value, json};
use std::sync::Arc;

pub type SourceFileFn = Arc<dyn Fn(SourceCommand) -> Result<Value> + Send + Sync>;

/// Immutable owner/version, captured before a system file operation starts.
#[derive(Clone)]
pub struct SourceCommand {
    pub command: String,
    pub params: Value,
    /// Original caller parameters, without completion-only stable layer binding.
    pub recorded_params: Value,
    pub document: Arc<Document>,
    pub revision: u64,
    pub path: Option<String>,
    pub layer: Option<LayerId>,
    pub scope: Option<crate::automation::Scope>,
}

pub enum SourceResult {
    Read { path: String, name: String, bytes: Vec<u8> },
    Refresh { paths: Vec<(LayerId, String)>, failed: Vec<LayerId> },
    Published(String),
}

pub fn supports(id: &str) -> bool {
    matches!(
        id,
        "file.revert"
            | "layer.smartObjects.replaceContents"
            | "layer.smartObjects.relinkToFile"
            | "layer.smartObjects.updateModifiedContent"
            | "layer.smartObjects.updateAllModifiedContent"
            | "layer.smartObjects.convertToEmbedded"
            | "layer.smartObjects.editContents"
            | "layer.smartObjects.exportContents"
            | "layer.smartObjects.convertToLinked"
    )
}

/// Synchronous scripts and scratch batches cannot wait for a system picker or
/// authorization. Fail the step instead of using an old cache or advancing to
/// later writes. Sessions without a platform source service keep their behavior.
pub fn check_synchronous(session: &Session, id: &str) -> Result<()> {
    if session.source_file.is_some() && supports(id) {
        return Err(EngineError::Other(format!("`{id}` requires an asynchronous source operation; run it separately before replaying this action")));
    }
    Ok(())
}

pub(crate) fn capture(session: &Session, id: &str, params: &Value) -> Result<SourceCommand> {
    let layer =
        if matches!(id, "file.revert" | "layer.smartObjects.updateAllModifiedContent") { None } else { Some(crate::commands::layer_param(session, params)?) };
    let state = session.active().ok_or(EngineError::NoDocument)?;
    let recorded_params = params.clone();
    let mut params = if params.is_object() { params.clone() } else { json!({}) };
    if let Some(layer) = layer {
        params["layer"] = json!(layer.0);
    }
    Ok(SourceCommand {
        command: id.into(),
        params,
        recorded_params,
        document: state.doc.clone(),
        revision: state.revision,
        path: state.path.clone(),
        layer,
        scope: session.automation.permit,
    })
}

impl Session {
    /// Apply a completed source request to its original document/version. The
    /// original handlers own decoding/history; pending admission never journals
    /// an edit that has not happened. Invalid responses cannot fall back to disk.
    pub fn complete_source(&mut self, request: &SourceCommand, response: SourceResult) -> Result<Value> {
        if request.scope.is_some() {
            return self.complete_automation_source(request, response);
        }
        self.complete_source_direct(request, response)
    }

    pub(crate) fn complete_source_direct(&mut self, request: &SourceCommand, response: SourceResult) -> Result<Value> {
        let target = self
            .docs
            .iter()
            .position(|state| state.doc.id == request.document.id)
            .ok_or_else(|| EngineError::Other("the original document was closed".into()))?;
        if self
            .docs
            .get(target)
            .is_none_or(|state| state.revision != request.revision || state.path != request.path || !Arc::ptr_eq(&state.doc, &request.document))
        {
            return Err(EngineError::Other("the original document changed while the file operation was pending".into()));
        }
        let previous = self.active().map(|state| state.doc.id);
        self.set_active(target);
        let result = if let Some(why) = crate::commands::find(&request.command).and_then(|spec| self.job_conflict(&request.command, spec.journal)) {
            Err(EngineError::Disabled(request.command.clone(), why))
        } else {
            self.apply_source(request, response)
        };
        let all_failed = result.as_ref().is_ok_and(|value| {
            value.get("updated").and_then(Value::as_array).is_some_and(Vec::is_empty)
                && value.get("failed").and_then(Value::as_array).is_some_and(|failed| !failed.is_empty())
        });
        if result.is_ok()
            && !all_failed
            && let Some(spec) = crate::commands::find(&request.command)
        {
            self.after_command(&request.command, request.recorded_params.clone(), spec.journal);
        }
        // Edit Contents deliberately activates its new child when its initiating
        // document is still active; tab switches during a picker are preserved.
        if (request.command != "layer.smartObjects.editContents" || previous != Some(request.document.id) || result.is_err())
            && let Some(index) = previous.and_then(|id| self.docs.iter().position(|state| state.doc.id == id))
        {
            self.set_active(index);
        }
        result
    }

    fn apply_source(&mut self, request: &SourceCommand, response: SourceResult) -> Result<Value> {
        match (request.command.as_str(), response) {
            ("file.revert", SourceResult::Read { name, bytes, .. }) => crate::file_cmds::revert_bytes(self, &name, &bytes),
            ("layer.smartObjects.editContents", SourceResult::Read { name, bytes, .. }) => {
                crate::smart_cmds::edit_contents_bytes(self, &request.params, &name, &bytes)
            }
            ("layer.smartObjects.replaceContents" | "layer.smartObjects.relinkToFile", SourceResult::Read { path, .. }) => {
                let mut params = request.params.clone();
                params["path"] = json!(path);
                let spec = crate::commands::find(&request.command).ok_or_else(|| EngineError::UnknownCommand(request.command.clone()))?;
                (spec.run)(self, &params)
            }
            ("layer.smartObjects.updateModifiedContent", SourceResult::Refresh { paths, failed }) => {
                crate::smart_cmds::update_from_files(self, &request.params, &paths, &failed, false)
            }
            ("layer.smartObjects.updateAllModifiedContent", SourceResult::Refresh { paths, failed }) => {
                crate::smart_cmds::update_from_files(self, &request.params, &paths, &failed, true)
            }
            ("layer.smartObjects.convertToEmbedded", SourceResult::Read { name, bytes, .. }) => {
                crate::smart_cmds::embed_fresh(self, &request.params, name, bytes)
            }
            ("layer.smartObjects.exportContents", SourceResult::Published(path)) => Ok(json!({"path":path})),
            ("layer.smartObjects.convertToLinked", SourceResult::Published(path)) => crate::smart_cmds::link_published(self, &request.params, &path),
            _ => Err(EngineError::Other("source completion does not match its command".into())),
        }
    }
}
