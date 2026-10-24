use crate::auth::private_dir;
use anyhow::{Context, Result, ensure};
use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::OwnedFd,
        unix::{
            fs::{OpenOptionsExt, PermissionsExt},
            net::UnixStream,
            process::CommandExt,
        },
    },
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

const READY: &[u8; 6] = b"ready\n";

pub fn start(config_dir: &Path, runtime_dir: &Path) -> Result<()> {
    let executable = std::env::current_exe().context("Locate the Likh executable")?;
    let config_dir = std::path::absolute(config_dir)?;
    let runtime_dir = std::path::absolute(runtime_dir)?;
    private_dir(&runtime_dir)?;
    let log_path = runtime_dir.join("daemon.log");
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&log_path)
        .with_context(|| format!("Open {}", log_path.display()))?;
    ensure!(
        log.metadata()?.is_file(),
        "Daemon log must be a regular file"
    );
    log.set_permissions(fs::Permissions::from_mode(0o600))?;
    let mut command = Command::new(executable);
    command
        .arg("--config-dir")
        .arg(config_dir)
        .arg("--runtime-dir")
        .arg(runtime_dir)
        .args(["run", "--daemon-child"])
        .current_dir("/")
        .stderr(log);
    let pid = launch(&mut command, Duration::from_secs(60))
        .with_context(|| format!("Background startup failed; see {}", log_path.display()))?;
    println!("Likh is running in the background (PID {pid}).");
    println!("Log: {}", log_path.display());
    println!("Stop with `kill -TERM {pid}`.");
    Ok(())
}

fn launch(command: &mut Command, timeout: Duration) -> Result<u32> {
    let (mut parent, child) = UnixStream::pair()?;
    parent.set_read_timeout(Some(timeout))?;
    command.stdin(Stdio::null()).stdout(OwnedFd::from(child));
    // SAFETY: setsid is async-signal-safe; the hook does not allocate or take locks.
    // Re-exec creates the Tokio runtime and audio thread only after detaching.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().context("Spawn background Likh")?;
    // Command retains its copy of the socket. Close it so a failed child gives EOF.
    command.stdout(Stdio::null());
    let mut reply = [0; READY.len()];
    let result = parent
        .read_exact(&mut reply)
        .context("Likh exited or timed out before becoming ready")
        .and_then(|()| {
            ensure!(&reply == READY, "Invalid background startup response");
            Ok(())
        });
    if let Err(error) = result {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    Ok(child.id())
}

pub fn notify_ready() -> Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(READY)?;
    stdout.flush()?;
    // After the handshake, all future output belongs in the log, not the socket.
    // SAFETY: both descriptors are open standard streams inherited at exec.
    ensure!(
        unsafe { libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) } != -1,
        "Redirect daemon output: {}",
        io::Error::last_os_error()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_child_has_its_own_session_and_no_terminal_input() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf 'ready\\n'; exec sleep 30"]);
        command.stderr(Stdio::null());
        let pid = launch(&mut command, Duration::from_secs(3)).unwrap() as libc::pid_t;
        let session = unsafe { libc::getsid(pid) };
        let group = unsafe { libc::getpgid(pid) };
        let stdin = fs::read_link(format!("/proc/{pid}/fd/0"));
        // Reap our test child even if an assertion below fails.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
            libc::waitpid(pid, std::ptr::null_mut(), 0);
        }
        assert_eq!(session, pid);
        assert_eq!(group, pid);
        assert_eq!(stdin.unwrap(), Path::new("/dev/null"));
    }

    #[test]
    fn failed_or_stuck_child_is_reaped() {
        for script in ["exit 7", "exec sleep 30", "printf 'wrong\\n'"] {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", script]);
            assert!(launch(&mut command, Duration::from_millis(100)).is_err());
        }
    }
}
