use crate::{
    audio::{self, Audio, Capture},
    auth::{Lock, Store, private_dir},
    config::{Config, PasteShortcut},
    desktop::{Activity, Desktop},
    transcribe::{self, Outcome},
};
use anyhow::{Context, Result, ensure};
use std::{
    io::{Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

struct Session {
    id: u64,
    capture: Option<Capture>,
    capture_error: Option<String>,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
    started: Instant,
    released: Option<Instant>,
    shortcut: PasteShortcut,
}
impl Session {
    fn fail_capture(&mut self, error: String) {
        self.capture.take();
        self.capture_error.get_or_insert(error);
        self.released.get_or_insert_with(Instant::now);
        // Keep the task and session ID alive until its recoverable text arrives.
    }
}

fn complete_session(
    session: &mut Option<Session>,
    id: u64,
    mut outcome: Outcome,
    last: &mut String,
) -> Option<(Session, Outcome)> {
    if !session.as_ref().is_some_and(|s| s.id == id) {
        return None;
    }
    let s = session.take().unwrap();
    if let Some(error) = &s.capture_error {
        // A full audio queue can prevent the error frame from reaching the task.
        // Even an already-queued successful outcome must not be pasted then.
        outcome.error.get_or_insert_with(|| error.clone());
    }
    if !outcome.text.is_empty() {
        *last = outcome.text.clone();
    }
    Some((s, outcome))
}

impl Drop for Session {
    fn drop(&mut self) {
        self.capture.take();
        self.cancel.cancel();
        self.task.abort();
    }
}
enum PasteStage {
    Modifiers,
    Snapshot,
    Delivered,
}
struct Paste {
    text: String,
    shortcut: PasteShortcut,
    stage: PasteStage,
    deadline: Instant,
    restore: Option<Instant>,
    released: Instant,
}

pub fn runtime_dir() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .context("XDG_RUNTIME_DIR is not set; run inside a desktop session")?;
    Ok(base.join("likh"))
}
pub fn copy_last(dir: &Path) -> Result<()> {
    let mut stream = UnixStream::connect(dir.join("control.sock"))
        .context("Likh is not running; start `likh run`")?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.write_all(b"copy-last")?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply)?;
    ensure!(reply == "ok", "{reply}");
    println!("Last transcript copied.");
    Ok(())
}
struct SocketGuard(PathBuf);
impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
pub fn run(
    config: Config,
    store: Store,
    runtime_dir: PathBuf,
    runtime: &tokio::runtime::Runtime,
) -> Result<()> {
    private_dir(&runtime_dir)?;
    let _lock = Lock::acquire(&runtime_dir.join("daemon.lock"), true)?;
    let socket = runtime_dir.join("control.sock");
    if socket.exists() {
        std::fs::remove_file(&socket)?;
    }
    let listener = UnixListener::bind(&socket)?;
    listener.set_nonblocking(true)?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    let _socket = SocketGuard(socket);
    let desktop = Desktop::open(true)?;
    let audio = Audio::new()?;
    // Validate and refresh once before claiming readiness; subsequent sessions recheck expiry.
    runtime.block_on(store.token())?;
    let shutdown = CancellationToken::new();
    let stop = shutdown.clone();
    runtime.spawn(async move {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler");
        tokio::select! {_ = tokio::signal::ctrl_c()=>(), _ = term.recv()=>()};
        stop.cancel();
    });
    let (done, results) = mpsc::channel::<(u64, Outcome)>();
    let mut session: Option<Session> = None;
    let mut paste: Option<Paste> = None;
    let mut next_id = 0u64;
    let mut last = String::new();
    let mut hide: Option<Instant> = None;
    eprintln!(
        "Likh {} ready. Hold Ctrl+Win to speak; release either to finish, both to paste. Escape cancels.",
        env!("CARGO_PKG_VERSION")
    );
    while !shutdown.is_cancelled() {
        if let Some(event) = desktop.next(if session.is_some() || paste.is_some() {
            10
        } else {
            100
        }) {
            match event.kind {
                1 => {
                    // A completed paste must not hold up the next recording.
                    if paste
                        .as_ref()
                        .is_some_and(|p| matches!(p.stage, PasteStage::Delivered))
                    {
                        if paste
                            .as_ref()
                            .is_some_and(|p| p.restore.is_some_and(|t| Instant::now() >= t))
                        {
                            desktop.restore();
                        }
                        paste = None;
                    }
                    if session.is_some() || paste.is_some() {
                        if !session.as_ref().is_some_and(|s| s.capture.is_some()) {
                            desktop.activity(Activity::Processing);
                        }
                    } else {
                        next_id += 1;
                        let class = desktop.track();
                        let shortcut = config.paste_shortcut(&class);
                        eprintln!("Paste target: WM_CLASS={class:?}, shortcut={shortcut}");
                        if !desktop.valid_focus() {
                            desktop.indicator("Focus a text input first");
                            hide = Some(Instant::now() + Duration::from_secs(3));
                            continue;
                        }
                        let (capture, frames) = audio.start(next_id, config.microphone.clone())?;
                        desktop.busy(true);
                        desktop.activity(Activity::Starting);
                        hide = None;
                        let cancel = CancellationToken::new();
                        let task_cancel = cancel.clone();
                        let cfg = config.clone();
                        let auth = store.clone();
                        let done = done.clone();
                        let id = next_id;
                        let task=runtime.spawn(async move {
                            let outcome=tokio::select! {
                                biased;
                                _=task_cancel.cancelled()=>return,
                                result=async {
                                    match auth.token().await {
                                        Ok(token)=>transcribe::transcribe(cfg,token,frames,task_cancel.clone()).await,
                                        Err(e)=>Outcome{text:String::new(),error:Some(e.to_string())},
                                    }
                                }=>result,
                            };
                            let _=done.send((id,outcome));
                        });
                        session = Some(Session {
                            id: next_id,
                            capture: Some(capture),
                            capture_error: None,
                            cancel,
                            task,
                            started: Instant::now(),
                            released: None,
                            shortcut,
                        });
                    }
                }
                2 => {
                    if let Some(s) = &mut session
                        && s.capture.take().is_some()
                    {
                        s.released = Some(Instant::now());
                        desktop.activity(Activity::Processing);
                    }
                }
                3 => {
                    // A paste already delivered to the target cannot be undone safely.
                    session.take();
                    paste.take();
                    desktop.busy(false);
                    desktop.indicator("Cancelled");
                    hide = Some(Instant::now() + Duration::from_secs(1));
                }
                4 => {
                    if let Some(p) = &mut paste
                        && matches!(p.stage, PasteStage::Delivered)
                        && desktop.target_requestor(event.window)
                    {
                        // Toolkits can request several formats in sequence. Keep
                        // the transcript available through that negotiation.
                        p.restore = Some(Instant::now() + Duration::from_secs(1));
                        p.deadline = Instant::now() + Duration::from_secs(2);
                    }
                }
                5 => {
                    if let Some(p) = &mut paste
                        && matches!(p.stage, PasteStage::Snapshot)
                        && let Err(e) = deliver(&desktop, p)
                    {
                        eprintln!("{e}");
                        desktop.indicator("Paste stopped - use likh copy-last");
                        paste = None;
                        desktop.busy(false);
                        hide = Some(Instant::now() + Duration::from_secs(4));
                    }
                }
                _ => (),
            }
        }
        // Snapshot completed tasks before draining audio events. Capture reports
        // errors before closing its frame channel, so even a successful outcome
        // queued during this iteration will have its capture error applied first.
        let completed: Vec<_> = results.try_iter().collect();
        while let Ok(event) = audio.events.try_recv() {
            match event {
                audio::Event::Ready(id) => {
                    if let Some(s) = &session
                        && s.id == id
                        && s.capture.is_some()
                    {
                        desktop.activity(Activity::Listening);
                        eprintln!("Microphone ready in {} ms", s.started.elapsed().as_millis());
                    }
                }
                audio::Event::Error(id, error) => {
                    if let Some(s) = &mut session
                        && s.id == id
                    {
                        eprintln!("{error}");
                        s.fail_capture(error);
                        desktop.indicator("Microphone error - recovering text");
                        hide = None;
                    }
                }
            }
        }
        if let Some(capture) = session.as_ref().and_then(|s| s.capture.as_ref()) {
            desktop.level(capture.take_level());
        }
        if let Some(s) = &mut session
            && s.capture.is_some()
            && s.started.elapsed() >= Duration::from_secs(config.max_recording_seconds)
        {
            s.capture.take();
            s.released = Some(Instant::now());
            desktop.indicator("Recording limit - release Ctrl+Win");
        }
        for (id, outcome) in completed {
            let Some((s, outcome)) = complete_session(&mut session, id, outcome, &mut last) else {
                continue;
            };
            if let Some(error) = outcome.error {
                eprintln!("{error}");
                desktop.indicator(if outcome.text.is_empty() {
                    "Transcription failed - see terminal"
                } else {
                    "Transcription failed - use likh copy-last"
                });
                desktop.busy(false);
                hide = Some(Instant::now() + Duration::from_secs(4));
            } else if outcome.text.trim().is_empty() {
                desktop.indicator("No speech detected");
                desktop.busy(false);
                hide = Some(Instant::now() + Duration::from_secs(2));
            } else if !desktop.valid_focus() {
                desktop.indicator("Focus changed - use likh copy-last");
                desktop.busy(false);
                hide = Some(Instant::now() + Duration::from_secs(4));
            } else {
                let mut text = outcome.text;
                text.push(' ');
                paste = Some(Paste {
                    text,
                    shortcut: s.shortcut,
                    stage: PasteStage::Modifiers,
                    deadline: Instant::now() + Duration::from_secs(5),
                    restore: None,
                    released: s.released.unwrap_or(s.started),
                });
                if desktop.modifiers_down() {
                    desktop.indicator("Release keys to paste");
                }
            }
        }
        let mut paste_finished = false;
        if let Some(p) = &mut paste {
            if let Some(when) = p.restore
                && Instant::now() >= when
            {
                desktop.restore();
                paste_finished = true;
            }
            if Instant::now() >= p.deadline {
                if !matches!(p.stage, PasteStage::Delivered) {
                    desktop.indicator("Paste timed out - use likh copy-last");
                    hide = Some(Instant::now() + Duration::from_secs(3));
                }
                paste_finished = true;
            } else if matches!(p.stage, PasteStage::Modifiers) && !desktop.modifiers_down() {
                p.stage = PasteStage::Snapshot;
                p.deadline = Instant::now() + Duration::from_secs(1);
                match desktop.snapshot() {
                    Ok(true) => {
                        if let Err(e) = deliver(&desktop, p) {
                            eprintln!("{e}");
                            paste_finished = true;
                        }
                    }
                    Ok(false) => (),
                    Err(e) => {
                        eprintln!("{e}");
                        paste_finished = true;
                    }
                }
            }
        }
        if paste_finished {
            paste = None;
            desktop.busy(false);
            if hide.is_none() {
                desktop.indicator("");
            }
        }
        if hide.is_some_and(|t| Instant::now() >= t) {
            desktop.indicator("");
            hide = None;
        }
        while let Ok((mut stream, _)) = listener.accept() {
            stream.set_read_timeout(Some(Duration::from_millis(50)))?;
            stream.set_write_timeout(Some(Duration::from_millis(50)))?;
            let mut command = [0; 32];
            let reply = match stream.read(&mut command) {
                Ok(n) if &command[..n] == b"copy-last" => {
                    if session.is_some() || paste.is_some() {
                        "Dictation is busy; retry after it finishes"
                    } else if last.is_empty() {
                        "No transcript available in this session"
                    } else if desktop.set_clipboard(&last).is_ok() {
                        "ok"
                    } else {
                        "Cannot set clipboard"
                    }
                }
                _ => "Unknown command",
            };
            let _ = stream.write_all(reply.as_bytes());
        }
    }
    session.take();
    desktop.busy(false);
    desktop.indicator("");
    Ok(())
}
fn deliver(desktop: &Desktop, p: &mut Paste) -> Result<()> {
    ensure!(desktop.valid_focus(), "Focus changed; use `likh copy-last`");
    ensure!(
        desktop.snapshot_current(),
        "Clipboard changed; use `likh copy-last`"
    );
    desktop.set_clipboard(&p.text)?;
    if let Err(e) = desktop.paste(p.shortcut) {
        desktop.restore();
        return Err(e);
    }
    eprintln!("Release to paste: {} ms", p.released.elapsed().as_millis());
    p.stage = PasteStage::Delivered;
    p.deadline = Instant::now() + Duration::from_secs(2);
    desktop.busy(false);
    desktop.indicator("");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn capture_errors_recover_delayed_and_already_queued_outcomes() {
        for already_queued in [false, true] {
            for error_frame_received in [false, true] {
                let (done, results) = mpsc::channel();
                let (finish, ready) = tokio::sync::oneshot::channel();
                let cancel = CancellationToken::new();
                let task = tokio::spawn(async move {
                    ready.await.unwrap();
                    done.send((
                        7,
                        Outcome {
                            text: "current partial transcript".into(),
                            error: error_frame_received.then(|| "microphone lost".into()),
                        },
                    ))
                    .unwrap();
                });
                let mut session = Some(Session {
                    id: 7,
                    capture: None,
                    capture_error: None,
                    cancel: cancel.clone(),
                    task,
                    started: Instant::now(),
                    released: None,
                    shortcut: PasteShortcut::CtrlV,
                });
                let mut finish = Some(finish);
                if already_queued {
                    finish.take().unwrap().send(()).unwrap();
                    (&mut session.as_mut().unwrap().task).await.unwrap();
                }
                let capture_error = if error_frame_received {
                    "microphone lost"
                } else {
                    "Audio upload could not keep up; recording stopped"
                };
                session.as_mut().unwrap().fail_capture(capture_error.into());
                assert!(!cancel.is_cancelled());
                if !already_queued {
                    finish.take().unwrap().send(()).unwrap();
                    (&mut session.as_mut().unwrap().task).await.unwrap();
                }
                let (id, outcome) = results.try_recv().unwrap();
                let mut last = "previous transcript".to_owned();
                let (finished, outcome) =
                    complete_session(&mut session, id, outcome, &mut last).unwrap();
                assert!(session.is_none());
                assert_eq!(last, "current partial transcript");
                assert_eq!(outcome.error.as_deref(), Some(capture_error));
                drop(finished);
                assert!(cancel.is_cancelled());
            }
        }
    }

    #[tokio::test]
    async fn cancelled_session_discards_late_outcome() {
        let cancel = CancellationToken::new();
        let mut session = Some(Session {
            id: 7,
            capture: None,
            capture_error: None,
            cancel: cancel.clone(),
            task: tokio::spawn(std::future::pending()),
            started: Instant::now(),
            released: None,
            shortcut: PasteShortcut::CtrlV,
        });
        session
            .as_mut()
            .unwrap()
            .fail_capture("microphone lost".into());
        session.take();
        assert!(cancel.is_cancelled());
        let mut last = "previous transcript".to_owned();
        assert!(
            complete_session(
                &mut session,
                7,
                Outcome {
                    text: "discarded transcript".into(),
                    error: None,
                },
                &mut last,
            )
            .is_none()
        );
        assert_eq!(last, "previous transcript");
    }
}
