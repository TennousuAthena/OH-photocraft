//! Optional publication of an immutable multi-file export. No platform URI enters L5.

use crate::{EngineError, Result, Session};
use photocraft_doc::Document;
use serde_json::{Value, json};
use std::sync::Arc;

pub type TreeOutputFn = Arc<dyn Fn(Request) -> Result<Value> + Send + Sync>;

#[derive(Clone)]
pub struct Request {
    pub command: String,
    pub params: Value,
    pub document: Arc<Document>,
    pub revision: u64,
    pub path: Option<String>,
    /// Automatic assets are an independent export after a successful project save.
    pub automatic: bool,
}

pub fn supports(command: &str) -> bool {
    matches!(command, "file.export.saveForWebLegacy" | "file.generate.imageAssets" | "file.package")
}

pub fn capture(session: &Session, index: usize, command: &str, params: Value, automatic: bool) -> Result<Request> {
    if session.automation.permit.is_some() {
        return Err(EngineError::Other(format!("`{command}` requires asynchronous directory authorization; this script step cannot continue yet")));
    }
    let state = session.documents().get(index).ok_or(EngineError::NoDocument)?;
    Ok(Request { command: command.into(), params, document: state.doc.clone(), revision: state.revision, path: state.path.clone(), automatic })
}

/// Run the original encoder in an isolated snapshot. Only destination parameters
/// change; the caller's document, dirty marker and command parameters stay intact.
pub fn encode(request: &Request, directory: &str) -> Result<Value> {
    let mut session = Session::new();
    session.add_document((*request.document).clone(), request.path.clone());
    let mut params = request.params.clone();
    params["dir"] = json!(directory);
    if request.command == "file.export.saveForWebLegacy" {
        params.as_object_mut().map(|fields| fields.remove("path"));
    }
    if request.command == "file.generate.imageAssets" {
        params["on"] = json!(true);
    }
    if request.command == "file.package" {
        params["portableLinks"] = json!(true);
    }
    session.execute(&request.command, params)
}

impl Session {
    /// Publication is the command's completion. Admission never records a
    /// successful export, and switching tabs cannot journal against another doc.
    pub fn complete_tree_output(&mut self, request: &Request) {
        if request.automatic {
            return;
        }
        let previous = self.active().map(|state| state.doc.id);
        if let Some(index) = self.documents().iter().position(|state| state.doc.id == request.document.id) {
            self.set_active(index);
            if let Some(spec) = crate::commands::find(&request.command) {
                self.after_command(&request.command, request.params.clone(), spec.journal);
            }
        }
        if let Some(index) = previous.and_then(|id| self.documents().iter().position(|state| state.doc.id == id)) {
            self.set_active(index);
        }
    }
}
