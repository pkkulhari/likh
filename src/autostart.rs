use crate::{AutostartCommand, config};
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

const TEMPLATE: &str = include_str!("../packaging/likh.desktop");
const DISABLED: &str = "[Desktop Entry]\nType=Application\nName=Likh\nHidden=true\n";

pub fn manage(
    command: Option<AutostartCommand>,
    config_dir: Option<PathBuf>,
    runtime_dir: Option<PathBuf>,
) -> Result<()> {
    let path = config::config_home()?.join("autostart/likh.desktop");
    match command.unwrap_or(AutostartCommand::Enable) {
        AutostartCommand::Enable => {
            let executable = std::env::current_exe().context("Locate the Likh executable")?;
            let config_dir = config_dir.map(Ok).unwrap_or_else(config::config_dir)?;
            let entry = desktop_entry(&executable, &config_dir, runtime_dir.as_deref())?;
            write_entry(&path, &entry)?;
            println!("Autostart enabled: {}", path.display());
            println!("Likh will start at your next desktop login. Run `likh run -d` to start now.");
        }
        AutostartCommand::Disable => {
            // Hidden=true also overrides a system-wide autostart entry.
            write_entry(&path, DISABLED)?;
            println!("Autostart disabled. A running Likh instance is unchanged.");
        }
        AutostartCommand::Status => {
            let entry = match fs::read_to_string(&path) {
                Ok(entry) => entry,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    println!("Autostart is not configured at {}", path.display());
                    return Ok(());
                }
                Err(error) => return Err(error).context("Read autostart entry"),
            };
            println!(
                "Autostart {}: {}",
                if enabled(&entry) {
                    "enabled"
                } else {
                    "disabled"
                },
                path.display()
            );
        }
    }
    Ok(())
}

fn desktop_entry(
    executable: &Path,
    config_dir: &Path,
    runtime_dir: Option<&Path>,
) -> Result<String> {
    ensure!(
        !executable.as_os_str().as_encoded_bytes().contains(&b'='),
        "An autostart executable path cannot contain '='"
    );
    // GIO checks the executable's existence before expanding %% field escapes.
    ensure!(
        !executable.as_os_str().as_encoded_bytes().contains(&b'%'),
        "Install Likh in a path without '%' before enabling autostart (desktop launcher limitation)"
    );
    let mut exec = format!(
        "Exec={} --config-dir {}",
        quote_path(executable)?,
        quote_path(config_dir)?
    );
    if let Some(dir) = runtime_dir {
        exec.push_str(&format!(" --runtime-dir {}", quote_path(dir)?));
    }
    exec.push_str(" run -d");
    Ok(TEMPLATE.replace("Exec=likh run", &exec))
}

fn quote_path(path: &Path) -> Result<String> {
    let path = std::path::absolute(path)?;
    let text = path
        .to_str()
        .context("Autostart paths must be valid UTF-8")?;
    ensure!(
        !text.chars().any(char::is_control),
        "Autostart paths cannot contain control characters"
    );
    // Desktop Entry string escaping is applied before Exec argument unquoting.
    let mut quoted = String::from("\"");
    for ch in text.chars() {
        match ch {
            '\\' => quoted.push_str("\\\\\\\\"),
            '"' | '`' | '$' => {
                quoted.push_str("\\\\");
                quoted.push(ch);
            }
            '%' => quoted.push_str("%%"),
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    Ok(quoted)
}

fn write_entry(path: &Path, contents: &str) -> Result<()> {
    let dir = path
        .parent()
        .context("Autostart entry has no parent directory")?;
    fs::create_dir_all(dir).context("Create autostart directory")?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    file.write_all(contents.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(path).context("Save autostart entry")?;
    Ok(())
}

fn enabled(entry: &str) -> bool {
    let mut in_entry = false;
    let mut has_exec = false;
    for line in entry.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if in_entry && let Some((key, value)) = line.split_once('=') {
            match (key.trim(), value.trim()) {
                ("Hidden", "true") | ("X-GNOME-Autostart-enabled", "false") => return false,
                ("Exec", value) => has_exec = !value.is_empty(),
                _ => (),
            }
        }
    }
    has_exec
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_desktop_paths_without_shell_or_field_expansion() {
        assert_eq!(
            quote_path(Path::new("/tmp/a b\"c$d`e\\f%g")).unwrap(),
            r#""/tmp/a b\\"c\\$d\\`e\\\\f%%g""#
        );
        assert!(quote_path(Path::new("/tmp/a\nExec=bad")).is_err());
        assert!(desktop_entry(Path::new("/tmp/a=b"), Path::new("/tmp/c"), None).is_err());
        assert!(desktop_entry(Path::new("/tmp/a%b"), Path::new("/tmp/c"), None).is_err());
    }

    #[test]
    fn preserves_overrides_but_resolves_default_runtime_at_login() {
        let entry =
            desktop_entry(Path::new("/opt/likh"), Path::new("/tmp/config space"), None).unwrap();
        assert!(entry.contains("Exec=\"/opt/likh\" --config-dir \"/tmp/config space\" run -d\n"));
        assert!(!entry.contains("--runtime-dir"));
        let entry = desktop_entry(
            Path::new("/opt/likh"),
            Path::new("cfg"),
            Some(Path::new("run")),
        )
        .unwrap();
        assert!(entry.contains(&format!(
            "--runtime-dir {} run -d",
            quote_path(Path::new("run")).unwrap()
        )));
        assert!(enabled(&entry));
        assert!(!enabled(DISABLED));
        assert!(!enabled(&entry.replace("enabled=true", "enabled=false")));
    }
}
