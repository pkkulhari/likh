use std::{
    fs,
    os::{fd::AsRawFd, unix::fs::PermissionsExt},
    path::Path,
    process::{Command, Output},
};

fn cli(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_likh"))
        .args(args)
        .current_dir(home)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("xdg"))
        .env("XDG_RUNTIME_DIR", home.join("runtime"))
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
        .output()
        .unwrap()
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn autostart_enable_status_disable_and_reenable_are_isolated_and_repeatable() {
    let home = tempfile::tempdir().unwrap();
    let entry = home.path().join("xdg/autostart/likh.desktop");
    let args = [
        "--config-dir",
        "my config",
        "--runtime-dir",
        "my runtime",
        "autostart",
    ];
    success(cli(home.path(), &args));
    let contents = fs::read_to_string(&entry).unwrap();
    assert!(contents.contains(&format!("Exec=\"{}\"", env!("CARGO_BIN_EXE_likh"))));
    assert!(contents.contains(&format!(
        "--config-dir \"{}/my config\"",
        home.path().display()
    )));
    assert!(contents.contains(&format!(
        "--runtime-dir \"{}/my runtime\" run -d",
        home.path().display()
    )));
    assert!(!home.path().join("my runtime").exists());
    success(cli(home.path(), &args));
    assert_eq!(fs::read_to_string(&entry).unwrap(), contents);
    assert!(success(cli(home.path(), &["autostart", "status"])).contains("enabled"));

    // Managing login startup must also work when the app configuration is broken.
    fs::create_dir_all(home.path().join("xdg/likh")).unwrap();
    fs::write(home.path().join("xdg/likh/config.toml"), "invalid config").unwrap();
    for _ in 0..2 {
        success(cli(home.path(), &["autostart", "disable"]));
        assert!(success(cli(home.path(), &["autostart", "status"])).contains("disabled"));
        assert!(fs::read_to_string(&entry).unwrap().contains("Hidden=true"));
    }
    success(cli(home.path(), &["autostart", "enable"]));
    assert!(success(cli(home.path(), &["autostart", "status"])).contains("enabled"));
    assert!(
        !fs::read_to_string(&entry)
            .unwrap()
            .contains("--runtime-dir")
    );
}

#[test]
fn autostart_uses_home_when_xdg_config_home_is_unset_or_empty() {
    let home = tempfile::tempdir().unwrap();
    for value in [None, Some(""), Some("relative")] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_likh"));
        command
            .args(["autostart", "enable"])
            .env("HOME", home.path());
        if let Some(value) = value {
            command.env("XDG_CONFIG_HOME", value);
        } else {
            command.env_remove("XDG_CONFIG_HOME");
        }
        success(command.output().unwrap());
        assert!(home.path().join(".config/autostart/likh.desktop").is_file());
    }
}

#[test]
fn daemon_reports_failed_startup_and_keeps_private_diagnostics() {
    let home = tempfile::tempdir().unwrap();
    for flag in ["-d", "--daemon"] {
        let output = cli(
            home.path(),
            &["run", flag, "--runtime-dir", "private runtime"],
        );
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("Background startup failed"));
        let runtime = home.path().join("private runtime");
        let log = runtime.join("daemon.log");
        assert!(fs::read_to_string(&log).unwrap().contains("likh:"));
        assert_eq!(
            fs::metadata(&runtime).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&log).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(!runtime.join("control.sock").exists());
    }
}

#[test]
fn background_launch_respects_the_existing_instance_lock() {
    let home = tempfile::tempdir().unwrap();
    let runtime = home.path().join("runtime/likh");
    fs::create_dir_all(&runtime).unwrap();
    let lock = fs::File::create(runtime.join("daemon.lock")).unwrap();
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let output = cli(home.path(), &["run", "-d"]);
    assert!(!output.status.success());
    assert!(
        fs::read_to_string(runtime.join("daemon.log"))
            .unwrap()
            .contains("Another Likh process")
    );
    assert!(!runtime.join("control.sock").exists());
}

#[test]
fn help_documents_foreground_daemon_and_autostart() {
    let home = tempfile::tempdir().unwrap();
    let help = success(cli(home.path(), &["run", "--help"]));
    assert!(help.contains("-d, --daemon"));
    assert!(!help.contains("daemon-child"));
    let help = success(cli(home.path(), &["autostart", "--help"]));
    for action in ["enable", "disable", "status"] {
        assert!(help.contains(action));
    }
    assert!(
        !cli(home.path(), &["run", "-d", "--daemon-child"])
            .status
            .success()
    );
}
