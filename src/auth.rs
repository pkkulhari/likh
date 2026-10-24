use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const CLIENT: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const CALLBACK: &str = "https://platform.claude.com/oauth/code/callback";
const SCOPES: &str = "user:profile user:inference user:sessions:claude_code user:mcp_servers";

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub fn private_dir(dir: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
pub struct Lock(File);
impl Lock {
    pub fn acquire(path: &Path, nonblocking: bool) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)?;
        // SAFETY: flock operates on an owned, valid file descriptor.
        let result = unsafe {
            libc::flock(
                file.as_raw_fd(),
                libc::LOCK_EX | if nonblocking { libc::LOCK_NB } else { 0 },
            )
        };
        ensure!(result == 0, "Another Likh process holds {}", path.display());
        Ok(Self(file))
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct Credentials {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: u64,
    pub scope: String,
}
impl Credentials {
    fn from_response(v: Value, previous: Option<&Self>) -> Result<Self> {
        let access = v["access_token"]
            .as_str()
            .filter(|s| !s.is_empty())
            .context("OAuth response lacks an access token")?;
        let lifetime = v["expires_in"]
            .as_u64()
            .filter(|s| *s > 0 && *s <= 31_536_000)
            .context("OAuth response lacks a valid expiry")?;
        Ok(Self {
            access_token: access.into(),
            refresh_token: v["refresh_token"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .or_else(|| previous.and_then(|p| p.refresh_token.clone())),
            expires_at: now() + lifetime,
            scope: v["scope"]
                .as_str()
                .unwrap_or_else(|| previous.map_or(SCOPES, |p| p.scope.as_str()))
                .into(),
        })
    }
}
#[derive(Clone)]
pub struct Store {
    pub dir: PathBuf,
}
impl Store {
    pub fn load(&self) -> Result<Credentials> {
        let raw = fs::read(self.dir.join("credentials.json"))
            .context("Not logged in; run `likh login`")?;
        let c: Credentials =
            serde_json::from_slice(&raw).context("Invalid credentials; run `likh login`")?;
        ensure!(
            !c.access_token.is_empty(),
            "Empty access token; run `likh login`"
        );
        Ok(c)
    }
    fn save(&self, value: &Credentials) -> Result<()> {
        private_dir(&self.dir)?;
        let mut file = tempfile::NamedTempFile::new_in(&self.dir)?;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
        serde_json::to_writer(&mut file, value)?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist(self.dir.join("credentials.json"))?;
        File::open(&self.dir)?.sync_all()?;
        Ok(())
    }
    pub async fn token(&self) -> Result<String> {
        if let Ok(token) = std::env::var("CLAUDE_CODE_OAUTH_TOKEN") {
            ensure!(!token.is_empty(), "CLAUDE_CODE_OAUTH_TOKEN is empty");
            return Ok(token);
        }
        private_dir(&self.dir)?;
        let path = self.dir.join("auth.lock");
        let _lock = tokio::task::spawn_blocking(move || Lock::acquire(&path, false)).await??;
        let mut c = self.load()?;
        if c.expires_at <= now() + 60 {
            let refresh = c
                .refresh_token
                .as_ref()
                .context("Token expired; run `likh login`")?;
            let response = token_request(json!({"grant_type":"refresh_token", "refresh_token":refresh, "client_id":CLIENT, "scope":c.scope})).await?;
            c = Credentials::from_response(response, Some(&c))?;
            self.save(&c)?;
        }
        Ok(c.access_token)
    }
    pub fn logout(&self) -> Result<()> {
        private_dir(&self.dir)?;
        let _lock = Lock::acquire(&self.dir.join("auth.lock"), false)?;
        match fs::remove_file(self.dir.join("credentials.json")) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }
    pub async fn login(&self, no_browser: bool) -> Result<()> {
        let (url, verifier, state) = authorization();
        println!("Open this URL and sign in to Claude:\n{url}\n");
        if !no_browser {
            let _ = std::process::Command::new("xdg-open")
                .arg(url)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .and_then(|mut c| c.wait());
        }
        print!("Paste the complete code#state or callback URL: ");
        io::stdout().flush()?;
        let input = hidden_line()?;
        let code = parse_code(input.trim(), &state)?;
        let response = token_request(json!({"grant_type":"authorization_code", "code":code, "code_verifier":verifier, "state":state, "redirect_uri":CALLBACK, "client_id":CLIENT})).await?;
        let creds = Credentials::from_response(response, None)?;
        private_dir(&self.dir)?;
        let _lock = Lock::acquire(&self.dir.join("auth.lock"), false)?;
        self.save(&creds)?;
        println!("Signed in. Start voice typing with `likh run`.");
        Ok(())
    }
}
fn authorization() -> (String, String, String) {
    let random = || {
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        URL_SAFE_NO_PAD.encode(bytes)
    };
    let verifier = random();
    let state = random();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = url::Url::parse("https://claude.com/cai/oauth/authorize").unwrap();
    url.query_pairs_mut().extend_pairs([
        ("code", "true"),
        ("response_type", "code"),
        ("client_id", CLIENT),
        ("redirect_uri", CALLBACK),
        ("scope", SCOPES),
        ("code_challenge_method", "S256"),
        ("code_challenge", &challenge),
        ("state", &state),
    ]);
    (url.into(), verifier, state)
}
fn parse_code(input: &str, expected: &str) -> Result<String> {
    let (code, state) = if input.starts_with("https://") {
        let url = url::Url::parse(input)?;
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        (
            pairs.get("code").cloned().unwrap_or_default(),
            pairs.get("state").cloned().unwrap_or_default(),
        )
    } else {
        let (a, b) = input
            .split_once('#')
            .context("Paste the complete code#state value")?;
        (a.into(), b.into())
    };
    // Constant-time comparison for equal-length random states.
    ensure!(
        state.len() == expected.len()
            && state
                .bytes()
                .zip(expected.bytes())
                .fold(0u8, |a, (b, c)| a | (b ^ c))
                == 0
            && !code.is_empty(),
        "OAuth state mismatch; start login again"
    );
    Ok(code)
}
async fn token_request(payload: Value) -> Result<Value> {
    tokio::time::timeout(Duration::from_secs(30), async {
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_only()
            .enable_http1()
            .build();
        let client =
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                .build::<_, Full<Bytes>>(connector);
        let request = http::Request::post("https://platform.claude.com/v1/oauth/token")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/plain, */*")
            .header("user-agent", "axios/1.13.6")
            .body(Full::new(Bytes::from(serde_json::to_vec(&payload)?)))?;
        let response = client
            .request(request)
            .await
            .map_err(|_| anyhow::anyhow!("Cannot reach OAuth endpoint; check network"))?;
        let status = response.status();
        if !status.is_success() {
            bail!(
                "OAuth rejected request (HTTP {}); run login again or check account access",
                status.as_u16()
            );
        }
        let body = http_body_util::Limited::new(response.into_body(), 64 * 1024)
            .collect()
            .await
            .map_err(|_| anyhow::anyhow!("Invalid OAuth response body"))?
            .to_bytes();
        serde_json::from_slice(&body).context("OAuth returned invalid JSON")
    })
    .await
    .context("OAuth request timed out")?
}
fn hidden_line() -> Result<String> {
    struct Restore(Option<libc::termios>);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(t) = self.0 {
                unsafe {
                    libc::tcsetattr(0, libc::TCSANOW, &t);
                }
            }
        }
    }
    let mut term = std::mem::MaybeUninit::<libc::termios>::uninit();
    let saved = unsafe {
        if libc::tcgetattr(0, term.as_mut_ptr()) == 0 {
            Some(term.assume_init())
        } else {
            None
        }
    };
    let _restore = Restore(saved);
    if let Some(mut t) = saved {
        t.c_lflag &= !(libc::ECHO | libc::ICANON | libc::ISIG);
        t.c_cc[libc::VMIN] = 1;
        t.c_cc[libc::VTIME] = 0;
        unsafe {
            libc::tcsetattr(0, libc::TCSANOW, &t);
        }
    }
    let mut bytes = Vec::new();
    for byte in io::stdin().lock().bytes() {
        match byte? {
            b'\n' | b'\r' => break,
            3 | 4 => bail!("Login cancelled"),
            8 | 127 => {
                bytes.pop();
            }
            byte => bytes.push(byte),
        }
        ensure!(bytes.len() <= 8192, "Authorization result too long");
    }
    println!();
    String::from_utf8(bytes).context("Invalid authorization result")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pkce_and_state() {
        let (url, verifier, state) = authorization();
        let url = url::Url::parse(&url).unwrap();
        assert!(url.query_pairs().any(|(k, v)| k == "code_challenge"
            && v == URL_SAFE_NO_PAD.encode(Sha256::digest(&verifier))));
        assert_ne!(state, authorization().2);
        assert!(parse_code("bare-code", &state).is_err());
        assert!(parse_code("code#wrong", &state).is_err());
        assert_eq!(
            parse_code(&format!("code#{state}"), &state).unwrap(),
            "code"
        );
    }
    #[test]
    fn rotation_and_private_storage() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store {
            dir: dir.path().join("auth"),
        };
        let first = Credentials::from_response(
            json!({"access_token":"a","refresh_token":"r","expires_in":3600}),
            None,
        )
        .unwrap();
        let second =
            Credentials::from_response(json!({"access_token":"b","expires_in":3600}), Some(&first))
                .unwrap();
        assert_eq!(second.refresh_token.as_deref(), Some("r"));
        let third = Credentials::from_response(
            json!({"access_token":"c","refresh_token":"new","expires_in":3600}),
            Some(&second),
        )
        .unwrap();
        store.save(&third).unwrap();
        assert_eq!(store.load().unwrap().refresh_token.as_deref(), Some("new"));
        assert_eq!(
            fs::metadata(store.dir.join("credentials.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(
            Credentials::from_response(json!({"access_token":"b","expires_in":0}), Some(&third))
                .is_err()
        );
        assert_eq!(store.load().unwrap().access_token, "c");
    }
}
