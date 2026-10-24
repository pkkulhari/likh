use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub language: String,
    pub microphone: Option<String>,
    pub endpoint: String,
    pub cli_version: String,
    pub max_recording_seconds: u64,
    pub connect_timeout_seconds: u64,
    pub finalize_timeout_seconds: u64,
    pub paste_overrides: BTreeMap<String, PasteShortcut>,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum PasteShortcut {
    CtrlV,
    CtrlShiftV,
    ShiftInsert,
}

impl std::fmt::Display for PasteShortcut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::CtrlV => "ctrl-v",
            Self::CtrlShiftV => "ctrl-shift-v",
            Self::ShiftInsert => "shift-insert",
        })
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            language: "en".into(),
            microphone: None,
            endpoint: "wss://api.anthropic.com/api/ws/speech_to_text/voice_stream".into(),
            cli_version: "2.1.309".into(),
            max_recording_seconds: 120,
            connect_timeout_seconds: 20,
            finalize_timeout_seconds: 5,
            paste_overrides: BTreeMap::new(),
        }
    }
}
impl Config {
    pub fn load(dir: &std::path::Path) -> Result<Self> {
        let path = dir.join("config.toml");
        let config: Self = if path.exists() {
            toml::from_str(&std::fs::read_to_string(path)?).context("Invalid config.toml")?
        } else {
            Self::default()
        };
        ensure!(
            !config.language.is_empty() && config.language.len() <= 32,
            "Invalid language code"
        );
        ensure!(
            (1..=120).contains(&config.max_recording_seconds),
            "max_recording_seconds must be 1–120"
        );
        ensure!(
            (1..=30).contains(&config.connect_timeout_seconds),
            "connect_timeout_seconds must be 1–30"
        );
        ensure!(
            (1..=30).contains(&config.finalize_timeout_seconds),
            "finalize_timeout_seconds must be 1–30"
        );
        let url = url::Url::parse(&config.endpoint)?;
        // Never send OAuth credentials over an unencrypted or arbitrary connection.
        ensure!(
            url.scheme() == "wss"
                && matches!(url.host_str(), Some("api.anthropic.com" | "claude.ai"))
                && url.username().is_empty()
                && url.password().is_none(),
            "Endpoint must use wss on api.anthropic.com or claude.ai"
        );
        Ok(config)
    }
    pub fn paste_shortcut(&self, class: &str) -> PasteShortcut {
        let class = class.to_lowercase();
        if let Some(shortcut) = self.paste_overrides.get(&class) {
            return *shortcut;
        }
        // Zed forwards Ctrl+V into its terminal (Codex treats it as image paste).
        // Shift+Insert invokes text paste in BOTH Zed's editor and terminal.
        // https://github.com/zed-industries/zed/blob/main/assets/keymaps/default-linux.json
        if [
            "zed",
            "zed-preview",
            "zed-nightly",
            "zed-dev",
            "dev.zed.zed",
            "dev.zed.zed-preview",
            "dev.zed.zed-nightly",
            "dev.zed.zed-dev",
        ]
        .contains(&class.as_str())
        {
            return PasteShortcut::ShiftInsert;
        }
        if [
            "gnome-terminal",
            "gnome-terminal-server",
            "org.gnome.terminal",
            "org.gnome.console",
            "org.gnome.ptyxis",
            "ptyxis",
            "kgx",
            "konsole",
            "xfce4-terminal",
            "alacritty",
            "kitty",
            "org.wezfurlong.wezterm",
            "wezterm",
            "com.mitchellh.ghostty",
            "ghostty",
            "tilix",
            "terminator",
            "guake",
            "yakuake",
            "sakura",
            "terminology",
            "hyper",
            "tabby",
            "xterm",
            "uxterm",
        ]
        .contains(&class.as_str())
        {
            PasteShortcut::CtrlShiftV
        } else {
            PasteShortcut::CtrlV
        }
    }
}
pub fn config_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path).join("likh"));
    }
    Ok(PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".config/likh"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_and_override_shortcuts() {
        let mut c = Config::default();
        assert_eq!(
            c.paste_shortcut("Gnome-terminal"),
            PasteShortcut::CtrlShiftV
        );
        assert_eq!(c.paste_shortcut("code"), PasteShortcut::CtrlV);
        c.paste_overrides
            .insert("code".into(), PasteShortcut::ShiftInsert);
        assert_eq!(c.paste_shortcut("Code"), PasteShortcut::ShiftInsert);
    }
    #[test]
    fn terminal_aliases_use_terminal_paste_not_raw_ctrl_v() {
        let c = Config::default();
        for class in [
            "org.gnome.Console",
            "org.gnome.Terminal",
            "org.gnome.Ptyxis",
            "Ptyxis",
            "Ghostty",
            "WezTerm",
            "Tabby",
        ] {
            assert_eq!(
                c.paste_shortcut(class),
                PasteShortcut::CtrlShiftV,
                "{class}"
            );
        }
        for class in ["firefox", "Google-chrome", "Code", "Cursor"] {
            assert_eq!(c.paste_shortcut(class), PasteShortcut::CtrlV, "{class}");
        }
    }
    #[test]
    fn zed_editor_and_terminal_share_text_paste_with_override_precedence() {
        let mut c = Config::default();
        for class in [
            "Zed",
            "zed-preview",
            "zed-nightly",
            "zed-dev",
            "dev.zed.Zed",
            "dev.zed.Zed-Preview",
            "dev.zed.Zed-Nightly",
            "dev.zed.Zed-Dev",
        ] {
            assert_eq!(
                c.paste_shortcut(class),
                PasteShortcut::ShiftInsert,
                "{class}"
            );
        }
        c.paste_overrides
            .insert("dev.zed.zed".into(), PasteShortcut::CtrlShiftV);
        assert_eq!(c.paste_shortcut("dev.zed.Zed"), PasteShortcut::CtrlShiftV);
    }
    #[test]
    fn reject_unsafe_endpoint_and_invalid_duration() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Config::load(dir.path()).unwrap().language, "en");
        for content in [
            "endpoint = 'ws://api.anthropic.com/voice'",
            "endpoint = 'wss://example.com/voice'",
            "max_recording_seconds = 121",
            "finalize_timeout_seconds = 0",
            "typo = true",
        ] {
            std::fs::write(dir.path().join("config.toml"), content).unwrap();
            assert!(Config::load(dir.path()).is_err(), "{content}");
        }
    }
}
