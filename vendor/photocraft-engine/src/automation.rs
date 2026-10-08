//! Resumable platform command sequences. No UI, URI or image algorithm lives here.

use std::collections::VecDeque;
use std::sync::Arc;

use photocraft_doc::{DocId, Document, LayerId};
use serde_json::{Value, json};

use crate::automate_cmds::ScriptBinding;
use crate::file_cmds::{FileProcess, ProcessInput};
use crate::jobs::{JobCtx, JobEvent, JobId, JobOutcome, Started};
use crate::source_cmds::{SourceCommand, SourceResult};
use crate::{EngineError, Result, Session};

#[cfg(test)]
mod tests;

const MAX_DEPTH: usize = 32;
const MAX_STEPS: usize = 10_000;
const TICK_STEPS: usize = 8;

/// Session and step identities remain distinct even when persisted DocIds overlap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scope {
    pub run: u64,
    pub session: u64,
    pub step: u64,
}

#[derive(Default)]
pub struct Automation {
    pub enabled: bool,
    pub defer_batch_publication: bool,
    pub(crate) permit: Option<Scope>,
    runs: VecDeque<Run>,
    events: VecDeque<(String, ScriptBinding)>,
    encoded: VecDeque<Encoded>,
}

pub struct Encoded {
    pub job: JobId,
    pub command: String,
    pub result: Value,
}

struct Run {
    id: JobId,
    context: JobCtx,
    command: String,
    params: Value,
    action: bool,
    frames: Vec<Frame>,
    batch: Option<Batch>,
    flow: Flow,
    waiting: Option<Waiting>,
    ready: Option<Result<Value>>,
    steps: u64,
    result: Option<Value>,
    publication: bool,
    events: VecDeque<(String, ScriptBinding)>,
}

struct Batch {
    process: FileProcess,
    steps: Vec<(String, Value)>,
    current: Option<ProcessInput>,
    generation: u64,
}

struct Frame {
    steps: Vec<(String, Value)>,
    next: usize,
    results: Vec<Value>,
    failed: bool,
    returns: ReturnTo,
}

enum ReturnTo {
    Root,
    Script(String, Value),
    Event(String, String),
}

#[derive(Clone, Copy)]
enum Waiting {
    Source(Scope),
    Job(JobId, Scope),
}

#[derive(Default)]
struct Flow {
    document: Option<DocId>,
    guard: Option<Guard>,
}

struct Guard {
    document: Arc<Document>,
    revision: u64,
    path: Option<String>,
    layer: Option<LayerId>,
    selected: Vec<LayerId>,
    channels: crate::channel_cmds::ChannelView,
}

impl Flow {
    fn capture(session: &Session) -> Self {
        Self::for_document(session, session.active().map(|state| state.doc.id))
    }

    fn for_document(session: &Session, document: Option<DocId>) -> Self {
        let guard = document.and_then(|id| session.docs.iter().find(|state| state.doc.id == id)).map(|state| Guard {
            document: state.doc.clone(),
            revision: state.revision,
            path: state.path.clone(),
            layer: state.active_layer,
            selected: state.selected_layers.clone(),
            channels: state.channel_view.clone(),
        });
        Self { document, guard }
    }

    fn activate(&self, session: &mut Session) -> Result<()> {
        if let Some(guard) = &self.guard {
            let index = session
                .docs
                .iter()
                .position(|state| state.doc.id == guard.document.id)
                .ok_or_else(|| EngineError::Other("the automation document was closed".into()))?;
            let state = session.docs.get(index).ok_or(EngineError::NoDocument)?;
            if !Arc::ptr_eq(&state.doc, &guard.document)
                || state.revision != guard.revision
                || state.path != guard.path
                || state.active_layer != guard.layer
                || state.selected_layers != guard.selected
                || state.channel_view != guard.channels
            {
                return Err(EngineError::Other("the automation document or layer selection changed while a step was pending".into()));
            }
            session.set_active(index);
        } else {
            session.active = None;
        }
        Ok(())
    }
}

impl Frame {
    fn new(steps: Vec<(String, Value)>, returns: ReturnTo) -> Result<Self> {
        if steps.len() > MAX_STEPS {
            return Err(EngineError::Other("the automation step limit was exceeded".into()));
        }
        Ok(Self { steps, next: 0, results: Vec::new(), failed: false, returns })
    }

    fn finish(&mut self, result: Result<Value>) {
        let Some((id, _)) = self.steps.get(self.next) else { return };
        match result {
            Ok(value) => {
                self.failed |= value.get("ok").and_then(Value::as_bool) == Some(false);
                self.results.push(json!({"command":id,"result":value}));
            }
            Err(error) => {
                self.failed = true;
                self.results.push(json!({"command":id,"error":error.to_string()}));
            }
        }
        self.next = self.next.saturating_add(1);
    }

    fn result(&self) -> Value {
        let mut result = json!({"steps":self.steps.len(),"ok":!self.failed,"results":self.results});
        if self.failed {
            result["error"] = json!(step_error(&result, 0));
        }
        result
    }
}

pub(crate) fn supports(id: &str) -> bool {
    matches!(id, "file.scripts.browse" | "file.automate.batch")
}

impl Session {
    pub fn start_action(&mut self, name: &str, steps: Vec<(String, Value)>) -> Result<JobId> {
        if !self.automation.enabled {
            return Err(EngineError::Other("asynchronous actions are not enabled for this session".into()));
        }
        self.enqueue_sequence("action.play", json!({"name":name}), name, steps, None, true, None)
    }

    pub fn cancel_actions(&mut self) {
        for id in self.jobs().into_iter().filter(|job| job.command == "action.play").map(|job| job.id).collect::<Vec<_>>() {
            self.cancel_job(id);
        }
    }

    pub(crate) fn begin_automation(&mut self, command: &str, params: Value) -> Result<JobId> {
        if command == "file.scripts.browse" {
            let steps = crate::automate_cmds::script_steps(&params)?;
            self.enqueue_sequence(command, params, "Running script", steps, None, false, None)
        } else {
            let raw = params
                .get("steps")
                .or_else(|| params.get("action"))
                .ok_or_else(|| EngineError::BadParams { cmd: command.into(), msg: "missing steps".into() })?;
            let steps = crate::file_cmds::parse_steps(raw)?;
            for (id, _) in &steps {
                if crate::commands::find(id).is_none() {
                    return Err(EngineError::UnknownCommand(id.clone()));
                }
            }
            let inputs = crate::file_cmds::batch_inputs(&params, command)?;
            if inputs.len() > 500 {
                return Err(EngineError::Other("too many batch inputs".into()));
            }
            let output = crate::file_cmds::directory_output(self, &params, command)?;
            let format = params.get("format").and_then(Value::as_str).unwrap_or("same").into();
            let quality = crate::file_cmds::f64_param(&params, "quality");
            let batch = Batch { process: FileProcess::new(inputs, output, format, quality, String::new()), steps, current: None, generation: 0 };
            self.enqueue_sequence(command, params, "Processing batch", Vec::new(), Some(batch), false, None)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn enqueue_sequence(
        &mut self,
        command: &str,
        params: Value,
        label: &str,
        steps: Vec<(String, Value)>,
        batch: Option<Batch>,
        action: bool,
        event: Option<(String, String)>,
    ) -> Result<JobId> {
        let frame = if batch.is_none() { Some(Frame::new(steps, event.map_or(ReturnTo::Root, |(event, name)| ReturnTo::Event(event, name)))?) } else { None };
        let document = if batch.is_some() { None } else { self.active().map(|state| state.doc.id) };
        let (id, context) = self.external_job(command, label, document)?;
        self.automation.runs.push_back(Run {
            id,
            context,
            command: command.into(),
            params,
            action,
            frames: frame.into_iter().collect(),
            batch,
            flow: Flow::capture(self),
            waiting: None,
            ready: None,
            steps: 0,
            result: None,
            publication: false,
            events: VecDeque::new(),
        });
        Ok(id)
    }

    pub(crate) fn queue_automation_event(&mut self, event: &str, bindings: Vec<ScriptBinding>) -> usize {
        let mut accepted = 0;
        for binding in bindings {
            if let Some(scope) = self.automation.permit {
                let queue = self.automation.runs.iter_mut().find(|run| run.id.0 == scope.run).map_or(&mut self.automation.events, |run| &mut run.events);
                if queue.len() < 32 {
                    queue.push_back((event.into(), binding));
                    accepted += 1;
                } else {
                    self.file_menu.event_log.push(json!({"event":event,"binding":binding.name,"error":"too many pending script events"}));
                }
            } else {
                match crate::automate_cmds::binding_steps(&binding).and_then(|steps| {
                    self.enqueue_sequence("scriptEvent", Value::Null, "Running script event", steps, None, false, Some((event.into(), binding.name.clone())))
                }) {
                    Ok(_) => accepted += 1,
                    Err(error) => self.file_menu.event_log.push(json!({"event":event,"binding":binding.name,"error":error.to_string()})),
                }
            }
        }
        trim_events(self);
        accepted
    }

    /// A source response applies only to the precise main/scratch scope that asked for it.
    pub fn validate_source_owner(&self, request: &SourceCommand) -> Result<()> {
        if let Some(scope) = request.scope {
            let run =
                self.automation.runs.iter().find(|run| run.id.0 == scope.run).ok_or_else(|| EngineError::Other("the automation request has ended".into()))?;
            if run.context.cancelled() {
                return Err(EngineError::Cancelled);
            }
            if run.ready.is_some() {
                return Err(EngineError::Other("the automation source response was already completed".into()));
            }
            if !matches!(run.waiting,Some(Waiting::Source(expected)) if expected==scope) {
                return Err(EngineError::Other("the source response belongs to another automation step".into()));
            }
            if scope.session != 0 {
                let batch = run
                    .batch
                    .as_ref()
                    .filter(|batch| batch.generation == scope.session)
                    .ok_or_else(|| EngineError::Other("the batch input has ended".into()))?;
                let scratch = batch.current.as_ref().ok_or(EngineError::NoDocument)?;
                return validate_document(&scratch.session, request);
            }
            if let Some(guard) = &run.flow.guard {
                let state = self.docs.iter().find(|state| state.doc.id == guard.document.id).ok_or(EngineError::NoDocument)?;
                if state.active_layer != guard.layer || state.selected_layers != guard.selected || state.channel_view != guard.channels {
                    return Err(EngineError::Other("the automation layer selection changed while the source was pending".into()));
                }
            }
        }
        validate_document(self, request)
    }

    pub(crate) fn complete_automation_source(&mut self, request: &SourceCommand, response: SourceResult) -> Result<Value> {
        self.validate_source_owner(request)?;
        let scope = request.scope.ok_or_else(|| EngineError::Other("missing automation scope".into()))?;
        let mut state = std::mem::take(&mut self.automation);
        self.automation.enabled = state.enabled;
        self.automation.defer_batch_publication = state.defer_batch_publication;
        self.automation.permit = Some(scope);
        let result = if let Some(run) = state.runs.iter_mut().find(|run| run.id.0 == scope.run) {
            if scope.session == 0 {
                let result = self.complete_source_direct(request, response);
                let document = result
                    .as_ref()
                    .ok()
                    .and_then(|value| value["document"].as_u64())
                    .and_then(|index| usize::try_from(index).ok())
                    .and_then(|index| self.docs.get(index))
                    .map(|state| state.doc.id)
                    .or(Some(request.document.id));
                run.flow = Flow::for_document(self, document);
                result
            } else {
                match run.batch.as_mut().and_then(|batch| batch.current.as_mut()) {
                    Some(scratch) => {
                        scratch.session.automation.permit = Some(scope);
                        let result = scratch.session.complete_source_direct(request, response);
                        scratch.session.automation.permit = None;
                        result
                    }
                    None => Err(EngineError::NoDocument),
                }
            }
        } else {
            Err(EngineError::Cancelled)
        };
        if let Some(run) = state.runs.iter_mut().find(|run| run.id.0 == scope.run) {
            run.ready = Some(clone_result(&result));
            run.events.extend(std::mem::take(&mut self.automation.events));
        }
        state.events.extend(std::mem::take(&mut self.automation.events));
        self.automation = state;
        result
    }

    /// Terminal cancellation/error must also release a waiting continuation.
    pub fn source_outcome(&mut self, request: &SourceCommand, error: Option<&str>, cancelled: bool) {
        let Some(scope) = request.scope else { return };
        let Some(run) = self.automation.runs.iter_mut().find(|run| run.id.0 == scope.run) else { return };
        if !matches!(run.waiting,Some(Waiting::Source(expected)) if expected==scope) {
            return;
        }
        if cancelled {
            run.ready = Some(Err(EngineError::Cancelled));
        } else if let Some(error) = error {
            run.ready = Some(Err(EngineError::Other(error.into())));
        } else if run.ready.is_none() {
            run.ready = Some(Ok(json!({"published":true})));
        }
    }

    pub(crate) fn tick_automation(&mut self, events: &[JobEvent]) {
        if !self.automation.enabled {
            return;
        }
        let mut state = std::mem::take(&mut self.automation);
        self.automation.enabled = true;
        self.automation.defer_batch_publication = state.defer_batch_publication;
        if let Some(mut run) = state.runs.pop_front() {
            let complete = self.advance_sequence(&mut run, events);
            state.events.extend(std::mem::take(&mut self.automation.events));
            if !complete {
                state.runs.push_front(run);
            }
        }
        state.runs.extend(std::mem::take(&mut self.automation.runs));
        state.encoded.extend(std::mem::take(&mut self.automation.encoded));
        self.automation = state;
        trim_events(self);
    }

    fn advance_sequence(&mut self, run: &mut Run, events: &[JobEvent]) -> bool {
        if run.publication {
            return false;
        }
        if let Some(Waiting::Job(id, scope)) = run.waiting {
            let mut scratch_events = Vec::new();
            if scope.session != 0
                && let Some(input) = run.batch.as_mut().and_then(|batch| batch.current.as_mut())
            {
                scratch_events = input.session.poll_jobs();
            }
            let owner_events = if scope.session == 0 { events } else { &scratch_events };
            if let Some(event) = owner_events.iter().find(|event| event.id == id) {
                run.ready = Some(match &event.outcome {
                    JobOutcome::Done(value) => Ok(value.clone()),
                    JobOutcome::Failed(error) => Err(EngineError::Other(error.clone())),
                    JobOutcome::Cancelled => Err(EngineError::Cancelled),
                });
                if scope.session == 0 {
                    run.flow = Flow::for_document(self, run.flow.document);
                }
            }
        }
        if run.context.cancelled() {
            if let Some(Waiting::Source(_)) = run.waiting
                && run.ready.is_none()
            {
                run.context.progress(0.0, "Cancelling; waiting for the file operation to settle");
                return false;
            }
            if let Some(Waiting::Job(id, scope)) = run.waiting {
                if scope.session == 0 {
                    self.cancel_job(id);
                } else if let Some(input) = run.batch.as_mut().and_then(|batch| batch.current.as_mut()) {
                    input.session.cancel_job(id);
                }
            }
            self.end_external_job(run.id, JobOutcome::Cancelled, run.batch.as_ref().map(|batch| batch.process.result()));
            return true;
        }
        if run.waiting.is_some() {
            let Some(result) = run.ready.take() else { return false };
            run.waiting = None;
            if matches!(result, Err(EngineError::Cancelled)) {
                run.context.cancel();
                return self.advance_sequence(run, events);
            }
            if let Some(frame) = run.frames.last_mut() {
                frame.finish(result);
            }
        }
        self.push_sequence_events(run);
        for _ in 0..TICK_STEPS {
            if run.frames.is_empty() {
                if let Some(batch) = &mut run.batch {
                    if let Some(input) = batch.current.take() {
                        let failed = run.result.take().filter(|value| value["ok"] == false);
                        let result = if let Some(value) = failed { Err(EngineError::Other(step_error(&value, 0))) } else { batch.process.encode(&input) };
                        batch.process.finish(input.path, result);
                    }
                    loop {
                        match batch.process.next_input() {
                            Some(Err((path, error))) => batch.process.finish(path, Err(error)),
                            Some(Ok(mut input)) => {
                                input.session.print_spool = self.print_spool.clone();
                                input.session.source_file = self.source_file.clone();
                                batch.generation = batch.generation.saturating_add(1);
                                batch.current = Some(input);
                                match Frame::new(batch.steps.clone(), ReturnTo::Root) {
                                    Ok(frame) => run.frames.push(frame),
                                    Err(error) => {
                                        self.end_external_job(run.id, JobOutcome::Failed(error.to_string()), Some(batch.process.result()));
                                        return true;
                                    }
                                }
                                break;
                            }
                            None => {
                                let result = batch.process.result();
                                if self.automation.defer_batch_publication {
                                    run.publication = true;
                                    self.automation.encoded.push_back(Encoded { job: run.id, command: run.command.clone(), result: result.clone() });
                                    run.result = Some(result);
                                    run.context.progress(0.99, "Waiting for folder publication confirmation");
                                    return false;
                                }
                                self.finish_sequence(run, JobOutcome::Done(result));
                                return true;
                            }
                        }
                    }
                } else {
                    let result = run.result.take().unwrap_or(json!({"ok":true,"steps":0,"results":[]}));
                    if run.action && result["ok"] == false {
                        self.finish_sequence(run, JobOutcome::Failed(step_error(&result, 0)));
                    } else {
                        self.finish_sequence(run, JobOutcome::Done(result));
                    }
                    return true;
                }
            }
            if run.frames.last().is_some_and(|frame| frame.failed || frame.next >= frame.steps.len()) {
                let Some(frame) = run.frames.pop() else { continue };
                let result = frame.result();
                match frame.returns {
                    ReturnTo::Root => run.result = Some(result),
                    ReturnTo::Script(id, params) => {
                        if run.batch.is_none()
                            && let Some(spec) = crate::commands::find(&id)
                        {
                            self.sequence_bookkeeping(run, &id, params, spec.journal);
                        }
                        if let Some(parent) = run.frames.last_mut() {
                            parent.finish(Ok(result));
                        }
                    }
                    ReturnTo::Event(event, binding) => {
                        self.file_menu.event_log.push(json!({"event":event,"binding":binding,"results":result["results"]}));
                        if run.frames.is_empty() {
                            run.result = Some(result);
                        }
                    }
                }
                continue;
            }
            let Some((id, params)) = run.frames.last().and_then(|frame| frame.steps.get(frame.next)).cloned() else { continue };
            if id == "file.scripts.browse" {
                let frame = crate::automate_cmds::script_steps(&params).and_then(|steps| Frame::new(steps, ReturnTo::Script(id.clone(), params)));
                if run.frames.len() >= MAX_DEPTH {
                    if let Some(parent) = run.frames.last_mut() {
                        parent.finish(Err(EngineError::Other("nested script depth limit exceeded".into())));
                    }
                } else {
                    match frame {
                        Ok(frame) => run.frames.push(frame),
                        Err(error) => {
                            if let Some(parent) = run.frames.last_mut() {
                                parent.finish(Err(error));
                            }
                        }
                    }
                }
                continue;
            }
            run.steps = run.steps.saturating_add(1);
            if run.steps > MAX_STEPS as u64 {
                self.finish_sequence(run, JobOutcome::Failed("automation step limit exceeded".into()));
                return true;
            }
            let scope = Scope { run: run.id.0, session: run.batch.as_ref().map_or(0, |batch| batch.generation), step: run.steps };
            let event = run.frames.iter().any(|frame| matches!(frame.returns, ReturnTo::Event(_, _)));
            let result = if let Some(input) = run.batch.as_mut().and_then(|batch| batch.current.as_mut()) {
                input.session.automation.permit = Some(scope);
                let result = input.session.start(&id, params);
                input.session.automation.permit = None;
                result
            } else {
                let previous = self.active().map(|state| state.doc.id);
                let previous_flow = run.flow.document;
                self.automation.permit = Some(scope);
                let firing = self.file_menu.firing;
                self.file_menu.firing |= event;
                let result = match run.flow.activate(self) {
                    Ok(()) => {
                        let result = self.start(&id, params);
                        run.flow = Flow::capture(self);
                        result
                    }
                    Err(error) => Err(error),
                };
                self.file_menu.firing = firing;
                self.automation.permit = None;
                if previous != previous_flow
                    && let Some(index) = previous.and_then(|id| self.docs.iter().position(|state| state.doc.id == id))
                {
                    self.set_active(index);
                }
                result
            };
            run.context.progress(0.0, &format!("Step {}: {}", run.steps, id));
            match result {
                Ok(Started::Job(id)) => run.waiting = Some(Waiting::Job(id, scope)),
                Ok(Started::Done(value)) if crate::source_cmds::supports(&id) && value["pending"] == true => run.waiting = Some(Waiting::Source(scope)),
                Ok(Started::Done(value)) => {
                    if let Some(frame) = run.frames.last_mut() {
                        frame.finish(Ok(value));
                    }
                }
                Err(error) => {
                    if let Some(frame) = run.frames.last_mut() {
                        frame.finish(Err(error));
                    }
                }
            }
            run.events.extend(std::mem::take(&mut self.automation.events));
            if run.waiting.is_some() {
                return false;
            }
            self.push_sequence_events(run);
        }
        false
    }

    fn push_sequence_events(&mut self, run: &mut Run) {
        while let Some((event, binding)) = run.events.pop_back() {
            match crate::automate_cmds::binding_steps(&binding).and_then(|steps| Frame::new(steps, ReturnTo::Event(event.clone(), binding.name.clone()))) {
                Ok(frame) if run.frames.len() < MAX_DEPTH => run.frames.push(frame),
                _ => self
                    .file_menu
                    .event_log
                    .push(json!({"event":event,"binding":binding.name,"error":"script event cannot be resumed within the depth/parse limit"})),
            }
        }
    }

    fn sequence_bookkeeping(&mut self, run: &Run, command: &str, params: Value, journal: bool) {
        let previous = self.active().map(|state| state.doc.id);
        let permit = self.automation.permit;
        if run.batch.is_some() || run.flow.activate(self).is_ok() {
            self.automation.permit = Some(Scope { run: run.id.0, session: 0, step: run.steps });
            self.after_command(command, params, journal);
        }
        self.automation.permit = permit;
        if let Some(index) = previous.and_then(|id| self.docs.iter().position(|state| state.doc.id == id)) {
            self.set_active(index);
        }
    }

    fn finish_sequence(&mut self, run: &Run, outcome: JobOutcome) {
        if matches!(outcome, JobOutcome::Done(_))
            && let Some(spec) = crate::commands::find(&run.command)
        {
            self.sequence_bookkeeping(run, &run.command, run.params.clone(), spec.journal);
        }
        self.end_external_job(run.id, outcome, run.batch.as_ref().map(|batch| batch.process.result()));
    }

    pub fn take_automation_outputs(&mut self) -> Vec<Encoded> {
        self.automation.encoded.drain(..).collect()
    }

    pub fn complete_automation_publication(&mut self, id: JobId, result: Result<Value>) -> bool {
        let Some(index) = self.automation.runs.iter().position(|run| run.id == id && run.publication) else { return false };
        let Some(run) = self.automation.runs.remove(index) else { return false };
        let outcome = match result {
            Ok(value) => JobOutcome::Done(value),
            Err(EngineError::Cancelled) => JobOutcome::Cancelled,
            Err(error) => JobOutcome::Failed(error.to_string()),
        };
        self.finish_sequence(&run, outcome);
        true
    }
}

fn validate_document(session: &Session, request: &SourceCommand) -> Result<()> {
    let state =
        session.docs.iter().find(|state| state.doc.id == request.document.id).ok_or_else(|| EngineError::Other("the original document was closed".into()))?;
    if state.revision != request.revision || state.path != request.path || !Arc::ptr_eq(&state.doc, &request.document) {
        return Err(EngineError::Other("the original document changed while the source was pending".into()));
    }
    Ok(())
}

fn clone_result(result: &Result<Value>) -> Result<Value> {
    match result {
        Ok(value) => Ok(value.clone()),
        Err(EngineError::Cancelled) => Err(EngineError::Cancelled),
        Err(error) => Err(EngineError::Other(error.to_string())),
    }
}

fn step_error(value: &Value, depth: usize) -> String {
    if depth < MAX_DEPTH
        && let Some(results) = value["results"].as_array()
    {
        for (index, result) in results.iter().enumerate() {
            if let Some(error) = result["error"].as_str() {
                return format!("Step {} ({}) failed: {error}", index + 1, result["command"].as_str().unwrap_or("command"));
            }
            if result["result"]["ok"] == false {
                return step_error(&result["result"], depth + 1);
            }
        }
    }
    "the automation step failed".into()
}

fn trim_events(session: &mut Session) {
    if session.file_menu.event_log.len() > 32 {
        let extra = session.file_menu.event_log.len() - 32;
        session.file_menu.event_log.drain(..extra);
    }
}
