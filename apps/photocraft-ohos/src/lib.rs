#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The unsafe boundary is isolated here; all app and GPU work belongs to one render thread.
#[cfg(feature = "device-tests")]
mod device_tests;
#[cfg(test)]
mod document_views;
mod file_requests;
mod folder_requests;
mod fonts;
mod logger;
mod platform;
mod printing;
mod processor;
mod recent;
mod runner;
mod services;
mod source_files;

use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, OnceLock, mpsc};

use runner::Runner;

enum Command {
    Density(f32),
    SystemLanguage(String),
    Surface(usize, u32, u32),
    Resize(u32, u32),
    Destroy,
    Frame(u64),
    Pointer(f32, f32, i32, i32, f32, f32, f32),
    Key(i32, bool, bool, bool, bool, String),
    Scroll(f32, f32),
    Zoom(f32),
    Open(String),
    Save(String),
    TakeFileRequest(mpsc::Sender<String>),
    PrepareFileSave(u64, String, mpsc::Sender<Result<String, String>>),
    CompleteFileRequest(u64, String, String, String, String),
    FileError(String),
    PlatformInput(platform::Input),
    #[cfg(feature = "device-tests")]
    TestControl(photocraft_ui_egui::ControlRequest),
}

struct Request {
    command: Command,
    reply: Option<mpsc::Sender<Result<(), String>>>,
}

struct Worker {
    sender: mpsc::SyncSender<Request>,
    files: String,
    cache: String,
    #[cfg(feature = "device-tests")]
    device_mailbox: Option<device_tests::SharedMailbox>,
}

struct WorkerReady {
    #[cfg(feature = "device-tests")]
    device_mailbox: Option<device_tests::SharedMailbox>,
}

static WORKER: OnceLock<Mutex<Option<Worker>>> = OnceLock::new();
static ERROR: OnceLock<Mutex<CString>> = OnceLock::new();

fn set_error(message: &str) {
    let message = message.replace('\0', " ");
    if let Ok(value) = CString::new(message) {
        *ERROR
            .get_or_init(|| Mutex::new(CString::default()))
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = value;
    }
}

fn guarded(action: impl FnOnce() -> Result<(), String>) -> bool {
    guarded_with_policy(action, true)
}

fn event(action: impl FnOnce() -> Result<(), String>) -> bool {
    guarded_with_policy(action, false)
}

fn guarded_with_policy(action: impl FnOnce() -> Result<(), String>, clear: bool) -> bool {
    let result = catch_unwind(AssertUnwindSafe(action))
        .unwrap_or_else(|_| Err("PhotoCraft recovered from a native operation panic".into()));
    match result {
        Ok(()) => {
            if clear {
                set_error("");
            }
            true
        }
        Err(error) => {
            set_error(&error);
            false
        }
    }
}

fn request(command: Command) -> Result<(), String> {
    let sender = WORKER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|worker| worker.sender.clone())
        .ok_or("PhotoCraft has not been initialized")?;
    let (tx, rx) = mpsc::channel();
    sender
        .send(Request {
            command,
            reply: Some(tx),
        })
        .map_err(|_| "PhotoCraft render thread stopped")?;
    // Surface destruction must wait until the GPU resources are dropped. No timeout may allow
    // the native window to be released while the renderer still uses it.
    rx.recv().map_err(|_| "PhotoCraft render thread stopped")?
}

/// # Safety
/// `value` is null (which is rejected), or a valid NUL-terminated byte string for
/// the duration of this call. Invalid UTF-8 is rejected.
unsafe fn read_string(value: *const c_char) -> Result<String, String> {
    if value.is_null() {
        return Err("null string passed to PhotoCraft".into());
    }
    // SAFETY: The C++ bridge keeps the NAPI string storage alive through this synchronous call.
    unsafe { CStr::from_ptr(value) }
        .to_str()
        .map(str::to_string)
        .map_err(|e| e.to_string())
}

/// # Safety
/// `language_tag` is null (which is rejected), or points to a valid NUL-terminated
/// UTF-8 system locale tag for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn craft_set_system_language(language_tag: *const c_char) -> bool {
    guarded(|| {
        // SAFETY: The bridge keeps its language tag alive through this synchronous call.
        let tag = unsafe { read_string(language_tag)? };
        let worker = WORKER
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if worker.is_none() {
            // The worker lock also serializes this with Runner construction during initialize.
            photocraft_ui_egui::i18n::set_system_language(&tag);
            return Ok(());
        }
        drop(worker);
        request(Command::SystemLanguage(tag))
    })
}

/// # Safety
/// Both paths point to valid NUL-terminated UTF-8 strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn craft_initialize(
    files: *const c_char,
    cache: *const c_char,
    density: f32,
) -> bool {
    guarded(|| {
        // SAFETY: See this entry point's contract and read_string.
        let (files, cache) = unsafe { (read_string(files)?, read_string(cache)?) };
        if !density.is_finite() || density <= 0.0 || density > 16.0 {
            return Err("invalid display density".into());
        }
        let mut worker = WORKER
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = worker.as_ref() {
            validate_reinitialization(&existing.files, &existing.cache, &files, &cache)?;
            drop(worker);
            return request(Command::Density(density));
        }
        let (worker_files, worker_cache) = (files.clone(), cache.clone());
        let (tx, rx) = mpsc::sync_channel::<Request>(64);
        let (ready_tx, ready_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("photocraft-render".into())
            .spawn(move || {
                let init = catch_unwind(AssertUnwindSafe(|| Runner::new(&files, &cache, density)));
                let mut app = match init {
                    Ok(Ok(app)) => app,
                    Ok(Err(error)) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                    Err(_) => {
                        let _ = ready_tx.send(Err("PhotoCraft initialization failed".into()));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(WorkerReady {
                    #[cfg(feature = "device-tests")]
                    device_mailbox: app.device_mailbox(),
                }));
                for request in rx {
                    let result = catch_unwind(AssertUnwindSafe(|| app.handle(request.command)))
                        .unwrap_or_else(|_| {
                            Err("PhotoCraft recovered from a render operation panic".into())
                        });
                    if let Some(reply) = request.reply {
                        let _ = reply.send(result);
                    } else if let Err(error) = result {
                        app.report_error(&error);
                        set_error(&error);
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let ready = ready_rx
            .recv()
            .map_err(|_| "PhotoCraft initialization thread stopped")??;
        #[cfg(not(feature = "device-tests"))]
        let _ = ready;
        *worker = Some(Worker {
            sender: tx,
            files: worker_files,
            cache: worker_cache,
            #[cfg(feature = "device-tests")]
            device_mailbox: ready.device_mailbox,
        });
        Ok(())
    })
}

/// # Safety
/// `window` is a retained OHNativeWindow, alive until craft_surface_destroyed returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn craft_surface_created(window: *mut c_void, width: u32, height: u32) {
    guarded(|| request(Command::Surface(window as usize, width, height)));
}

#[unsafe(no_mangle)]
pub extern "C" fn craft_surface_changed(width: u32, height: u32) {
    event(|| request(Command::Resize(width, height)));
}

#[unsafe(no_mangle)]
pub extern "C" fn craft_surface_destroyed() {
    event(|| request(Command::Destroy));
}

#[unsafe(no_mangle)]
pub extern "C" fn craft_frame(timestamp: u64, _target_timestamp: u64) {
    event(|| request(Command::Frame(timestamp)));
}

#[unsafe(no_mangle)]
pub extern "C" fn craft_pointer(
    x: f32,
    y: f32,
    action: i32,
    button: i32,
    pressure: f32,
    tilt_x: f32,
    tilt_y: f32,
) {
    event(|| {
        request(Command::Pointer(
            x, y, action, button, pressure, tilt_x, tilt_y,
        ))
    });
}

/// # Safety
/// `text` points to a valid NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn craft_key(
    code: i32,
    pressed: bool,
    ctrl: bool,
    shift: bool,
    alt: bool,
    text: *const c_char,
) {
    event(|| {
        // SAFETY: The C++ bridge keeps text alive until this call returns.
        request(Command::Key(code, pressed, ctrl, shift, alt, unsafe {
            read_string(text)?
        }))
    });
}

/// The per-update pinch multiplier, after native cumulative-scale conversion.
#[unsafe(no_mangle)]
pub extern "C" fn craft_zoom(factor: f32) {
    event(|| {
        if !factor.is_finite() || factor <= 0.0 {
            return Err("invalid pinch zoom factor".into());
        }
        request(Command::Zoom(factor))
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn craft_scroll(dx: f32, dy: f32) {
    event(|| request(Command::Scroll(dx, dy)));
}

/// # Safety
/// `path` points to a valid NUL-terminated UTF-8 sandbox path.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn craft_open_document(path: *const c_char) -> bool {
    // SAFETY: The C++ bridge owns path throughout this call.
    guarded(|| request(Command::Open(unsafe { read_string(path)? })))
}

/// # Safety
/// `path` points to a valid NUL-terminated UTF-8 sandbox path.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn craft_save_document(path: *const c_char) -> bool {
    // SAFETY: The C++ bridge owns path throughout this call.
    guarded(|| request(Command::Save(unsafe { read_string(path)? })))
}

thread_local! { static FILE_REQUEST_COPY: std::cell::RefCell<CString> = std::cell::RefCell::new(CString::default()); }
thread_local! { static PREPARED_SAVE_COPY: std::cell::RefCell<CString> = std::cell::RefCell::new(CString::default()); }

/// Re-encode the captured save snapshot for the chosen filename. The pointer
/// lasts until the next prepare call on this thread; an empty string is failure.
/// # Safety
/// `destination_name` is valid NUL-terminated UTF-8 throughout this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn craft_prepare_file_save(
    id: u64,
    destination_name: *const c_char,
) -> *const c_char {
    let mut path = String::new();
    guarded(|| {
        // SAFETY: The native bridge retains its UTF-8 filename for this RPC.
        let name = unsafe { read_string(destination_name)? };
        let (tx, rx) = mpsc::channel();
        request(Command::PrepareFileSave(id, name, tx))?;
        path = rx
            .recv()
            .map_err(|_| "PhotoCraft render thread stopped")??;
        Ok(())
    });
    PREPARED_SAVE_COPY.with(|copy| {
        *copy.borrow_mut() = CString::new(path).unwrap_or_default();
        copy.borrow().as_ptr()
    })
}

/// The pointer stays valid on the calling thread until the next call to this function.
#[unsafe(no_mangle)]
pub extern "C" fn craft_take_file_request() -> *const c_char {
    let (tx, rx) = mpsc::channel();
    let mut json = String::new();
    event(|| {
        request(Command::TakeFileRequest(tx))?;
        json = rx.recv().map_err(|_| "PhotoCraft render thread stopped")?;
        Ok(())
    });
    FILE_REQUEST_COPY.with(|copy| {
        *copy.borrow_mut() = CString::new(json).unwrap_or_default();
        copy.borrow().as_ptr()
    })
}

/// # Safety
/// All strings are NUL-terminated UTF-8 and remain alive throughout this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn craft_complete_file_request(
    id: u64,
    result: *const c_char,
    resolved_path: *const c_char,
    display_name: *const c_char,
    error: *const c_char,
) -> bool {
    guarded(|| {
        // SAFETY: The native bridge owns these strings through the synchronous RPC.
        let (result, resolved_path, display_name, error) = unsafe {
            (
                read_string(result)?,
                read_string(resolved_path)?,
                read_string(display_name)?,
                read_string(error)?,
            )
        };
        request(Command::CompleteFileRequest(
            id,
            result,
            resolved_path,
            display_name,
            error,
        ))
    })
}

/// # Safety
/// `message` is a NUL-terminated UTF-8 string, alive throughout this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn craft_report_file_error(message: *const c_char) {
    event(|| {
        // SAFETY: The native bridge owns the string until this synchronous RPC finishes.
        request(Command::FileError(unsafe { read_string(message)? }))
    });
}

thread_local! { static ERROR_COPY: std::cell::RefCell<CString> = std::cell::RefCell::new(CString::default()); }

/// The pointer stays valid on the calling thread until its next craft_last_error call.
#[unsafe(no_mangle)]
pub extern "C" fn craft_last_error() -> *const c_char {
    ERROR_COPY.with(|copy| {
        *copy.borrow_mut() = ERROR
            .get_or_init(|| Mutex::new(CString::default()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        copy.borrow().as_ptr()
    })
}

thread_local! { static PLATFORM_OUTPUT_COPY: std::cell::RefCell<CString> = std::cell::RefCell::new(CString::default()); }

/// Pulls a latest snapshot and bounded output queue without contacting the render worker.
/// The pointer remains valid until the next call on this calling thread.
#[unsafe(no_mangle)]
pub extern "C" fn craft_take_platform_output() -> *const c_char {
    let json = platform::output_json();
    PLATFORM_OUTPUT_COPY.with(|copy| {
        *copy.borrow_mut() = CString::new(json).unwrap_or_default();
        copy.borrow().as_ptr()
    })
}

/// # Safety
/// `packet` is a valid NUL-terminated UTF-8 JSON string. A true return transfers
/// ownership of a validated clipboard PNG to the asynchronous worker.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn craft_platform_input(packet: *const c_char) -> bool {
    event(|| {
        // SAFETY: The bridge retains its JSON storage for this call; Input owns it afterward.
        let input = platform::Input::parse(&unsafe { read_string(packet)? })?;
        let sender = WORKER
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|worker| worker.sender.clone())
            .ok_or("PhotoCraft has not been initialized")?;
        sender
            .try_send(Request {
                command: Command::PlatformInput(input),
                reply: None,
            })
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => "PhotoCraft input queue is full",
                mpsc::TrySendError::Disconnected(_) => "PhotoCraft render thread stopped",
            })?;
        Ok(())
    })
}

fn validate_reinitialization(
    previous_files: &str,
    previous_cache: &str,
    files: &str,
    cache: &str,
) -> Result<(), String> {
    if previous_files != files || previous_cache != cache {
        return Err(
            "PhotoCraft storage roots cannot change in a running process; start a new session"
                .into(),
        );
    }
    Ok(())
}

#[cfg(feature = "device-tests")]
fn test_worker() -> Result<(mpsc::SyncSender<Request>, device_tests::SharedMailbox), String> {
    let worker = WORKER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let worker = worker
        .as_ref()
        .ok_or("PhotoCraft has not been initialized")?;
    Ok((
        worker.sender.clone(),
        worker
            .device_mailbox
            .clone()
            .ok_or("device tests require an isolated PhotoCraftTestRuns session")?,
    ))
}

#[cfg(feature = "device-tests")]
thread_local! { static TEST_OUTPUT_COPY: std::cell::RefCell<CString> = std::cell::RefCell::new(CString::default()); }

#[cfg(feature = "device-tests")]
fn test_string(action: impl FnOnce() -> Result<String, String>) -> *const c_char {
    let mut output = String::new();
    guarded(|| {
        output = action()?;
        Ok(())
    });
    TEST_OUTPUT_COPY.with(|copy| {
        *copy.borrow_mut() = CString::new(output).unwrap_or_default();
        copy.borrow().as_ptr()
    })
}

/// Feature-only link/version guard. Normal archives contain no device-test ABI.
#[cfg(feature = "device-tests")]
#[unsafe(no_mangle)]
pub extern "C" fn craft_device_tests_abi_v1() -> u32 {
    1
}

/// # Safety
/// Both strings remain valid NUL-terminated UTF-8 through this call.
#[cfg(feature = "device-tests")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn craft_test_submit(
    run_id: *const c_char,
    request_json: *const c_char,
) -> *const c_char {
    test_string(|| {
        // SAFETY: The NAPI bridge owns both strings during this nonblocking submission.
        let (run_id, text) = unsafe { (read_string(run_id)?, read_string(request_json)?) };
        let (sender, mailbox) = test_worker()?;
        device_tests::submit(&mailbox, &run_id, &text, |control| {
            sender
                .try_send(Request {
                    command: Command::TestControl(control),
                    reply: None,
                })
                .map_err(|error| match error {
                    mpsc::TrySendError::Full(_) => {
                        "PhotoCraft device test worker queue is full".into()
                    }
                    mpsc::TrySendError::Disconnected(_) => {
                        "PhotoCraft render thread stopped".into()
                    }
                })
        })
    })
}

/// # Safety
/// Both strings remain valid NUL-terminated UTF-8 through this call.
#[cfg(feature = "device-tests")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn craft_test_poll(
    run_id: *const c_char,
    ticket: *const c_char,
) -> *const c_char {
    test_string(|| {
        // SAFETY: The NAPI bridge owns both strings while copying the latest response.
        let (run_id, ticket) = unsafe { (read_string(run_id)?, read_string(ticket)?) };
        let (_, mailbox) = test_worker()?;
        device_tests::poll(&mailbox, &run_id, &ticket)
    })
}

/// # Safety
/// `run_id` remains valid NUL-terminated UTF-8 through this call.
#[cfg(feature = "device-tests")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn craft_test_snapshot(run_id: *const c_char) -> *const c_char {
    test_string(|| {
        // SAFETY: The NAPI bridge owns this string during the independent mailbox read.
        let run_id = unsafe { read_string(run_id)? };
        let (_, mailbox) = test_worker()?;
        device_tests::snapshot(&mailbox, &run_id)
    })
}

#[cfg(test)]
mod initialization_tests {
    #[test]
    fn a_live_worker_cannot_switch_production_or_test_storage_roots() {
        assert!(
            super::validate_reinitialization("/a/files", "/a/cache", "/a/files", "/a/cache")
                .is_ok()
        );
        assert!(
            super::validate_reinitialization("/a/files", "/a/cache", "/b/files", "/a/cache")
                .is_err()
        );
        assert!(
            super::validate_reinitialization("/a/files", "/a/cache", "/a/files", "/b/cache")
                .is_err()
        );
    }
}
