use super::*;
use std::sync::Mutex;

fn folder(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("photocraft-async-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn image(name: &str, color: &str) -> (Session, Vec<u8>) {
    let mut session = Session::new();
    session.execute("file.new", json!({"width":4,"height":3,"background":color})).unwrap();
    let path = folder(name).join("source.png");
    crate::file_cmds::save_doc(&session.active().unwrap().doc, path.to_str().unwrap(), None).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    session.active_mut().unwrap().path = Some(path.to_string_lossy().into_owned());
    (session, bytes)
}

fn sources(session: &mut Session) -> Arc<Mutex<VecDeque<SourceCommand>>> {
    let queue = Arc::new(Mutex::new(VecDeque::new()));
    let output = queue.clone();
    session.source_file = Some(Arc::new(move |request| {
        output.lock().unwrap().push_back(request);
        Ok(json!({"pending":true}))
    }));
    session.automation.enabled = true;
    queue
}

fn complete(session: &mut Session, request: &SourceCommand, bytes: Vec<u8>) {
    session
        .complete_source(request, SourceResult::Read { path: request.path.clone().unwrap_or_else(|| "fresh.png".into()), name: "fresh.png".into(), bytes })
        .unwrap();
    session.source_outcome(request, None, false);
}

fn drain(session: &mut Session, job: JobId) -> JobInfo {
    for _ in 0..200 {
        session.poll_jobs();
        if let Some(info) = session.jobs_with_recent().into_iter().find(|info| info.id == job && info.state != "running") {
            return info;
        }
        std::thread::yield_now();
    }
    panic!("continuation did not finish");
}

use crate::jobs::JobInfo;

#[test]
fn nested_source_script_pauses_then_edits_original_document_and_preserves_tab() {
    let (mut session, _) = image("nested", "#000000");
    let (_, white) = image("nested-fresh", "#ffffff");
    let owner = session.active().unwrap().doc.id;
    let queue = sources(&mut session);
    let job = session
        .start(
            "file.scripts.browse",
            json!({"steps":[["file.scripts.browse",{"steps":[["file.revert",{}],["image.adjustments.invert",{}]]}],["edit.undo",{}]]}),
        )
        .unwrap();
    let Started::Job(job) = job else { panic!("script did not become a continuation") };
    session.poll_jobs();
    let request = queue.lock().unwrap().pop_front().unwrap();
    assert_eq!(request.scope.unwrap().run, job.0);
    assert!(session.journal.iter().all(|(id, _)| id != "file.revert"));
    session.execute("file.new", json!({"width":2,"height":2})).unwrap();
    let other = session.active().unwrap().doc.id;
    complete(&mut session, &request, white);
    assert!(session.complete_source(&request, SourceResult::Read { path: "unused".into(), name: "x".into(), bytes: vec![] }).is_err());
    let result = drain(&mut session, job);
    assert_eq!(result.state, "done");
    assert_eq!(result.result.unwrap()["ok"], true);
    assert_eq!(session.active().unwrap().doc.id, other);
    let doc = &session.docs.iter().find(|state| state.doc.id == owner).unwrap().doc;
    assert!(photocraft_compose::flatten(doc).px[0][0] > 0.99, "undo ran on owner after fresh revert + invert");
}

#[test]
fn cancelled_source_action_never_runs_following_write() {
    let (mut session, _) = image("cancel", "#000000");
    let queue = sources(&mut session);
    let output = folder("cancel-output").join("must-not-exist.png");
    let control = output.with_file_name("actual-copy.png");
    session.execute("file.saveACopy", json!({"path":control})).unwrap();
    assert!(std::fs::read(&control).unwrap().starts_with(b"\x89PNG\r\n\x1a\n"));
    let job = session.start_action("Revert then write", vec![("file.revert".into(), json!({})), ("file.saveACopy".into(), json!({"path":output}))]).unwrap();
    session.poll_jobs();
    let request = queue.lock().unwrap().pop_front().unwrap();
    session.source_outcome(&request, None, true);
    assert_eq!(drain(&mut session, job).state, "cancelled");
    assert!(!output.exists());
    assert_eq!(photocraft_compose::flatten(&session.active().unwrap().doc).px[0][0], 0.0);
}

#[test]
fn source_failure_or_owner_change_stops_later_action_steps() {
    for changed in [false, true] {
        let (mut session, _) = image(if changed { "changed" } else { "failure" }, "#000000");
        let queue = sources(&mut session);
        let job = session.start_action("Fresh then invert", vec![("file.revert".into(), json!({})), ("image.adjustments.invert".into(), json!({}))]).unwrap();
        session.poll_jobs();
        let request = queue.lock().unwrap().pop_front().unwrap();
        if changed {
            session.active_mut().unwrap().revision += 1;
            assert!(session.validate_source_owner(&request).is_err());
        }
        session.source_outcome(&request, Some("external read failed"), false);
        let info = drain(&mut session, job);
        assert_eq!(info.state, "failed");
        assert!(info.error.unwrap().contains("external read failed"));
        assert_eq!(photocraft_compose::flatten(&session.active().unwrap().doc).px[0][0], 0.0);
    }
}

#[test]
fn batch_scratch_source_identity_and_publication_are_separate_from_working_document() {
    let (mut session, _) = image("batch-working", "#ff0000");
    let working = session.active().unwrap().doc.clone();
    let queue = sources(&mut session);
    session.automation.defer_batch_publication = true;
    let (first, _) = image("batch-first", "#000000");
    let (mut second, fresh) = image("batch-second", "#ffffff");
    let second_path = std::path::Path::new(second.active().unwrap().path.as_ref().unwrap()).with_file_name("two.png");
    std::fs::write(&second_path, &fresh).unwrap();
    second.active_mut().unwrap().path = Some(second_path.to_string_lossy().into_owned());
    let inputs =
        vec![first.active().unwrap().path.clone().unwrap(), second.active().unwrap().path.clone().unwrap(), first.active().unwrap().path.clone().unwrap()];
    let output = folder("batch-output");
    let Started::Job(job) = session
        .start("file.automate.batch", json!({"input":inputs,"output":output,"format":"png","steps":[["file.revert",{}],["image.adjustments.invert",{}]]}))
        .unwrap()
    else {
        panic!("batch was synchronous")
    };
    let mut previous = None;
    for input in 0..2 {
        for _ in 0..30 {
            session.poll_jobs();
            if !queue.lock().unwrap().is_empty() {
                break;
            }
        }
        let request = queue.lock().unwrap().pop_front().unwrap();
        let scope = request.scope.unwrap();
        assert_eq!(scope.run, job.0);
        assert_ne!(scope.session, 0);
        if let Some(old) = &previous {
            assert!(session.validate_source_owner(old).is_err());
        }
        assert!(session.docs.iter().all(|state| !Arc::ptr_eq(&state.doc, &request.document)));
        complete(&mut session, &request, fresh.clone());
        previous = Some(request);
        if input == 0 {
            assert!(session.is_external_job(job));
        }
    }
    for _ in 0..40 {
        session.poll_jobs();
    }
    let encoded = session.take_automation_outputs();
    assert_eq!(encoded.len(), 1);
    assert_eq!(encoded[0].result["errors"].as_array().unwrap().len(), 1, "same filename conflict remains an original per-file error");
    assert_eq!(encoded[0].result["files"].as_array().unwrap().len(), 2);
    assert!(session.is_external_job(job), "encoding cannot imply publication success");
    assert!(Arc::ptr_eq(&session.active().unwrap().doc, &working));
    let bytes = std::fs::read(encoded[0].result["files"][0].as_str().unwrap()).unwrap();
    let saved = crate::file_cmds::import("saved.png", &bytes).unwrap();
    assert!(photocraft_compose::flatten(&saved).px[0][0] < 0.01);
    assert!(session.complete_automation_publication(job, Err(EngineError::Other("provider publish failed".into()))));
    let info = session.jobs_with_recent().into_iter().find(|info| info.id == job).unwrap();
    assert_eq!(info.state, "failed");
    assert!(!session.complete_automation_publication(job, Ok(json!({"published":true}))));
    assert!(output.join("source.png").exists(), "failed publication retains encoded bytes");
}

#[test]
fn source_journal_keeps_caller_parameters_for_cross_document_action_replay() {
    let (mut session, fresh) = image("record-source", "#ffffff");
    session.execute("layer.smartObjects.convertToSmartObject", json!({})).unwrap();
    let first = session.active().unwrap().active_layer.unwrap();
    let queue = sources(&mut session);
    assert_eq!(session.execute("layer.smartObjects.editContents", json!({})).unwrap()["pending"], true);
    let original = queue.lock().unwrap().pop_front().unwrap();
    complete(&mut session, &original, fresh.clone());
    let recorded = session.journal.last().unwrap().clone();
    assert_eq!(recorded.0, "layer.smartObjects.editContents");
    assert_eq!(recorded.1, json!({}), "stable completion target must not be recorded into an Action");
    session.execute("file.new", json!({"width":4,"height":3})).unwrap();
    session.execute("layer.smartObjects.convertToSmartObject", json!({})).unwrap();
    let second = session.active().unwrap().active_layer.unwrap();
    assert_ne!(first, second);
    let job = session.start_action("Replay source", vec![recorded]).unwrap();
    session.poll_jobs();
    let replay = queue.lock().unwrap().pop_front().unwrap();
    assert_eq!(replay.layer, Some(second));
    complete(&mut session, &replay, fresh);
    assert_eq!(drain(&mut session, job).state, "done");
}

#[test]
fn edit_contents_continues_on_its_child_without_switching_the_users_other_tab() {
    let (mut session, fresh) = image("edit-child", "#ffffff");
    session.execute("layer.smartObjects.convertToSmartObject", json!({})).unwrap();
    let parent = session.active().unwrap().doc.clone();
    let queue = sources(&mut session);
    let job = session
        .start_action("Edit smart child", vec![("layer.smartObjects.editContents".into(), json!({})), ("image.adjustments.invert".into(), json!({}))])
        .unwrap();
    session.poll_jobs();
    let request = queue.lock().unwrap().pop_front().unwrap();
    session.execute("file.new", json!({"width":1,"height":1})).unwrap();
    let other = session.active().unwrap().doc.id;
    complete(&mut session, &request, fresh);
    assert_eq!(drain(&mut session, job).state, "done");
    assert_eq!(session.active().unwrap().doc.id, other);
    assert!(Arc::ptr_eq(&session.docs.iter().find(|state| state.doc.id == parent.id).unwrap().doc, &parent));
    let child = session.docs.iter().find(|state| state.doc.id != parent.id && state.doc.id != other).unwrap();
    assert!(photocraft_compose::flatten(&child.doc).px[0][0] < 0.01);
    assert!(child.is_dirty());
}

#[test]
fn print_script_event_can_suspend_for_source_and_has_execution_only_recursion_guard() {
    let (mut session, fresh) = image("print-event", "#000000");
    session
        .execute(
            "file.scripts.scriptEventsManager",
            json!({"enabled":true,"add":{"event":"print","name":"fresh print hook","steps":[["file.revert",{}],["file.printOneCopy",{}]]}}),
        )
        .unwrap();
    let queue = sources(&mut session);
    let prints = Arc::new(Mutex::new(Vec::new()));
    let trace = prints.clone();
    session.print_spool = Some(Arc::new(move |spool| {
        assert!(spool.pdf.starts_with(b"%PDF"));
        trace.lock().unwrap().push(spool.document);
        Ok(crate::print_cmds::PrintSpoolResult {
            pdf: "private-spool.pdf".into(),
            request: 44,
            status: "system request accepted; final job state unconfirmed".into(),
        })
    }));
    let job = session.start_action("Physical print", vec![("file.print".into(), json!({}))]).unwrap();
    for _ in 0..5 {
        session.poll_jobs();
    }
    let request = queue.lock().unwrap().pop_front().unwrap();
    assert_eq!(prints.lock().unwrap().len(), 1);
    // Waiting does not globally suppress unrelated Script Events.
    assert!(!session.file_menu.firing);
    complete(&mut session, &request, fresh);
    assert_eq!(drain(&mut session, job).state, "done");
    assert_eq!(prints.lock().unwrap().len(), 2, "event Print One Copy must not recursively invoke its own binding");
    assert!(queue.lock().unwrap().is_empty());
    assert_eq!(session.file_menu.event_log.len(), 1);
}

#[test]
fn equal_root_and_scratch_job_numbers_do_not_cross_complete_a_batch_step() {
    let (mut session, _) = image("job-number-working", "#ff0000");
    let (input, _) = image("job-number-input", "#000000");
    session.automation.enabled = true;
    let output = folder("job-number-output");
    let batch = Batch {
        process: FileProcess::new(
            vec![input.active().unwrap().path.clone().unwrap()],
            output.to_string_lossy().into_owned(),
            "png".into(),
            None,
            String::new(),
        ),
        steps: vec![("document.pixel".into(), json!({"x":0,"y":0}))],
        current: None,
        generation: 1,
    };
    let id = session.enqueue_sequence("file.automate.batch", json!({}), "Batch", Vec::new(), Some(batch), false, None).unwrap();
    let run = session.automation.runs.front_mut().unwrap();
    let batch = run.batch.as_mut().unwrap();
    let mut current = batch.process.next_input().unwrap().unwrap();
    let (release, wait) = std::sync::mpsc::channel();
    let Started::Job(child) = current
        .session
        .start_job(
            "test.wait",
            json!({}),
            "Scratch",
            false,
            move |_| {
                wait.recv().map_err(|error| EngineError::Other(error.to_string()))?;
                Ok(())
            },
            |_, ()| Ok(json!({"realScratch":true})),
        )
        .unwrap()
    else {
        panic!("host job did not start")
    };
    assert_eq!(id, child, "distinct Sessions legitimately share local job numbers");
    batch.current = Some(current);
    run.frames.push(Frame::new(batch.steps.clone(), ReturnTo::Root).unwrap());
    run.waiting = Some(Waiting::Job(child, Scope { run: id.0, session: 1, step: 1 }));
    let unrelated = JobEvent {
        id: child,
        command: "other root job".into(),
        label: "Other".into(),
        document: None,
        outcome: JobOutcome::Failed("must not complete scratch".into()),
    };
    session.tick_automation(&[unrelated]);
    assert!(session.is_external_job(id));
    let run = session.automation.runs.front().unwrap();
    assert!(run.ready.is_none());
    assert!(matches!(run.waiting,Some(Waiting::Job(_,scope)) if scope.session==1));
    release.send(()).unwrap();
    for _ in 0..200 {
        session.poll_jobs();
        if !session.is_external_job(id) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let job = session.jobs_with_recent().into_iter().find(|job| job.id == id).unwrap();
    assert_eq!(job.state, "done");
    assert_eq!(job.result.unwrap()["errors"], json!([]));
}
