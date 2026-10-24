use crate::config::Config;
use anyhow::{Context, Result, bail, ensure};
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc,
    time::{Instant, timeout},
};
use tokio_util::sync::CancellationToken;
use tokio_websockets::{ClientBuilder, Message, WebSocketStream};

pub type AudioFrame = Result<Vec<u8>, String>;
#[derive(Default)]
struct Transcript {
    committed: Vec<String>,
    pending: String,
    endpoint: bool,
}
impl Transcript {
    fn handle(&mut self, raw: &str) -> Result<bool> {
        let v: serde_json::Value =
            serde_json::from_str(raw).context("Invalid transcription message")?;
        match v["type"].as_str() {
            Some("TranscriptText") => {
                self.pending = v["data"]
                    .as_str()
                    .context("Invalid transcript text")?
                    .into();
            }
            Some("TranscriptEndpoint") => {
                self.endpoint = true;
                if !self.pending.trim().is_empty() {
                    self.committed.push(self.pending.trim().to_owned());
                }
                self.pending.clear();
                return Ok(true);
            }
            Some("TranscriptError" | "error") => bail!("Transcription service reported an error"),
            _ => (),
        }
        ensure!(
            self.text().len() <= 128 * 1024,
            "Transcript exceeded size limit"
        );
        Ok(false)
    }
    fn text(&self) -> String {
        self.committed
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(self.pending.trim()))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }
}
pub struct Outcome {
    pub text: String,
    pub error: Option<String>,
}

pub async fn transcribe(
    config: Config,
    token: String,
    frames: mpsc::Receiver<AudioFrame>,
    cancel: CancellationToken,
) -> Outcome {
    let mut transcript = Transcript::default();
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Outcome { text: String::new(), error: Some("Cancelled".into()) },
        r = connect_and_stream(&config, &token, frames, &mut transcript) => r,
    };
    Outcome {
        text: transcript.text(),
        error: result.err().map(|e| e.to_string()),
    }
}
async fn connect_and_stream(
    config: &Config,
    token: &str,
    frames: mpsc::Receiver<AudioFrame>,
    transcript: &mut Transcript,
) -> Result<()> {
    let mut url = url::Url::parse(&config.endpoint)?;
    url.query_pairs_mut().extend_pairs([
        ("encoding", "linear16"),
        ("sample_rate", "16000"),
        ("channels", "1"),
        ("endpointing_ms", "300"),
        ("utterance_end_ms", "1000"),
        ("language", &config.language),
        ("use_conversation_engine", "true"),
        ("stt_provider", "deepgram-nova3"),
    ]);
    let client = ClientBuilder::new()
        .uri(url.as_str())?
        .limits(tokio_websockets::Limits::default().max_payload_len(Some(128 * 1024)))
        .add_header(
            http::header::AUTHORIZATION,
            format!("Bearer {token}")
                .parse()
                .context("Invalid access token")?,
        )?
        .add_header(
            http::header::HeaderName::from_static("x-app"),
            http::header::HeaderValue::from_static("cli"),
        )?
        .add_header(
            http::header::USER_AGENT,
            format!("claude-cli/{} (external, cli)", config.cli_version)
                .parse()
                .context("Invalid CLI version")?,
        )?;
    let (socket, _) = timeout(
        Duration::from_secs(config.connect_timeout_seconds),
        client.connect(),
    )
    .await
    .context("Voice connection timed out")?
    .map_err(|_| {
        anyhow::anyhow!("Voice connection failed; check network and Claude login/account access")
    })?;
    session(
        socket,
        frames,
        transcript,
        Duration::from_secs(config.finalize_timeout_seconds),
    )
    .await
}
async fn send<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
    message: Message,
) -> Result<()> {
    timeout(Duration::from_secs(5), socket.send(message))
        .await
        .context("Voice upload stalled")?
        .map_err(|_| anyhow::anyhow!("Voice connection lost during upload"))
}
async fn session<S: AsyncRead + AsyncWrite + Unpin>(
    mut socket: WebSocketStream<S>,
    mut frames: mpsc::Receiver<AudioFrame>,
    transcript: &mut Transcript,
    finalize: Duration,
) -> Result<()> {
    send(&mut socket, Message::text("{\"type\":\"KeepAlive\"}")).await?;
    let mut keepalive = tokio::time::interval(Duration::from_secs(8));
    keepalive.tick().await;
    let mut finishing = false;
    let mut deadline = Instant::now() + Duration::from_secs(180);
    loop {
        tokio::select! {
            frame = frames.recv(), if !finishing => match frame {
                Some(Ok(bytes)) => send(&mut socket, Message::binary(bytes)).await?,
                Some(Err(error)) => bail!("{error}"),
                None => {
                    send(&mut socket, Message::text("{\"type\":\"CloseStream\"}")).await?;
                    finishing = true; deadline = Instant::now() + finalize;
                }
            },
            message = socket.next() => match message {
                Some(Ok(message)) if message.is_text() => {
                    let raw = message.as_text().context("Invalid transcript encoding")?;
                    ensure!(raw.len() <= 128*1024, "Transcription message too large");
                    transcript.handle(raw)?;
                },
                None => {
                    ensure!(finishing, "Voice connection closed while recording");
                    ensure!(transcript.pending.is_empty() && transcript.endpoint, "Final transcript was not confirmed; use `likh copy-last` to recover available text");
                    return Ok(());
                },
                Some(Err(_)) => bail!("Voice connection failed; available text can be recovered with `likh copy-last`"),
                Some(Ok(message)) if message.is_close() => {
                    let code = u16::from(message.as_close().expect("close message").0);
                    ensure!(matches!(code, 1000 | 1005), "Voice service closed with error code {code}");
                    ensure!(finishing && transcript.pending.is_empty() && transcript.endpoint, "Voice connection closed without a confirmed final transcript");
                    return Ok(());
                },
                _ => (),
            },
            _ = keepalive.tick(), if !finishing => send(&mut socket, Message::text("{\"type\":\"KeepAlive\"}")).await?,
            _ = tokio::time::sleep_until(deadline) => {
                // An utterance endpoint cannot confirm that CloseStream drained
                // all uploaded audio, even when no newer text has arrived yet.
                bail!("Finalization timed out; use `likh copy-last` to recover available text");
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn revisions_and_endpoints() {
        let mut t = Transcript::default();
        t.handle(r#"{"type":"TranscriptText","data":"Hel"}"#)
            .unwrap();
        t.handle(r#"{"type":"TranscriptText","data":"Hello."}"#)
            .unwrap();
        t.handle(r#"{"type":"TranscriptEndpoint"}"#).unwrap();
        t.handle(r#"{"type":"TranscriptEndpoint"}"#).unwrap();
        t.handle(r#"{"type":"TranscriptText","data":"World."}"#)
            .unwrap();
        assert_eq!(t.text(), "Hello. World.");
        assert!(t.handle(r#"{"type":"TranscriptText","data":3}"#).is_err());
        assert!(t.handle(r#"{"type":"TranscriptError"}"#).is_err());
    }
    #[tokio::test]
    async fn mock_stream_finalizes_once() {
        let (client_io, server_io) = tokio::io::duplex(8192);
        let server = tokio::spawn(async move {
            let (request, mut ws) = tokio_websockets::ServerBuilder::new()
                .accept(server_io)
                .await
                .unwrap();
            assert_eq!(request.headers()["x-app"], "cli");
            let mut audio = Vec::new();
            while let Some(Ok(msg)) = ws.next().await {
                if msg.is_binary() {
                    audio.extend_from_slice(msg.as_payload());
                }
                if msg.as_text() == Some("{\"type\":\"CloseStream\"}") {
                    assert_eq!(audio, vec![1, 2, 3, 4]);
                    ws.send(Message::text(r#"{"type":"TranscriptText","data":"Hello"}"#))
                        .await
                        .unwrap();
                    ws.send(Message::text(
                        r#"{"type":"TranscriptText","data":"Hello world."}"#,
                    ))
                    .await
                    .unwrap();
                    ws.send(Message::text(r#"{"type":"TranscriptEndpoint"}"#))
                        .await
                        .unwrap();
                    ws.send(Message::close(None, "")).await.unwrap();
                    return;
                }
            }
        });
        let (socket, _) = ClientBuilder::new()
            .uri("ws://localhost/voice")
            .unwrap()
            .add_header(
                http::header::HeaderName::from_static("x-app"),
                http::HeaderValue::from_static("cli"),
            )
            .unwrap()
            .connect_on(client_io)
            .await
            .unwrap();
        let (tx, rx) = mpsc::channel(4);
        tx.send(Ok(vec![1, 2])).await.unwrap();
        tx.send(Ok(vec![3, 4])).await.unwrap();
        drop(tx);
        let mut transcript = Transcript::default();
        session(socket, rx, &mut transcript, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(transcript.text(), "Hello world.");
        server.await.unwrap();
    }
    #[tokio::test]
    async fn unconfirmed_text_is_recoverable() {
        let (a, b) = tokio::io::duplex(8192);
        let socket = ClientBuilder::new().take_over(a);
        let mut server = tokio_websockets::ServerBuilder::new().serve(b);
        let task = tokio::spawn(async move {
            while let Some(Ok(m)) = server.next().await {
                if m.as_text() == Some("{\"type\":\"CloseStream\"}") {
                    server
                        .send(Message::text(
                            r#"{"type":"TranscriptText","data":"recover me"}"#,
                        ))
                        .await
                        .unwrap();
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    break;
                }
            }
        });
        let (tx, rx) = mpsc::channel(1);
        drop(tx);
        let mut t = Transcript::default();
        assert!(
            session(socket, rx, &mut t, Duration::from_millis(50))
                .await
                .is_err()
        );
        assert_eq!(t.text(), "recover me");
        task.abort();
    }

    #[tokio::test]
    async fn does_not_finish_before_release_or_drop_delayed_utterances() {
        let (a, b) = tokio::io::duplex(8192);
        let socket = ClientBuilder::new().take_over(a);
        let mut server = tokio_websockets::ServerBuilder::new().serve(b);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let server_task = tokio::spawn(async move {
            server
                .send(Message::text(
                    r#"{"type":"TranscriptText","data":"First."}"#,
                ))
                .await
                .unwrap();
            server
                .send(Message::text(r#"{"type":"TranscriptEndpoint"}"#))
                .await
                .unwrap();
            ready_tx.send(()).unwrap();
            while let Some(Ok(message)) = server.next().await {
                if message.as_text() == Some("{\"type\":\"CloseStream\"}") {
                    // A final endpoint does not guarantee no further utterances.
                    tokio::time::sleep(Duration::from_millis(220)).await;
                    server
                        .send(Message::text(
                            r#"{"type":"TranscriptText","data":"Second."}"#,
                        ))
                        .await
                        .unwrap();
                    server
                        .send(Message::text(r#"{"type":"TranscriptEndpoint"}"#))
                        .await
                        .unwrap();
                    server.send(Message::close(None, "")).await.unwrap();
                    break;
                }
            }
        });
        let (tx, rx) = mpsc::channel(2);
        let task = tokio::spawn(async move {
            let mut transcript = Transcript::default();
            session(socket, rx, &mut transcript, Duration::from_secs(1))
                .await
                .unwrap();
            transcript.text()
        });
        ready_rx.await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!task.is_finished());
        drop(tx);
        assert_eq!(task.await.unwrap(), "First. Second.");
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn endpoint_without_stream_close_times_out_and_preserves_text() {
        // Cover both an older endpoint followed by more audio and an endpoint
        // received during finalization. Neither confirms the stream is drained.
        for earlier_endpoint in [true, false] {
            let (a, b) = tokio::io::duplex(8192);
            let socket = ClientBuilder::new().take_over(a);
            let mut server = tokio_websockets::ServerBuilder::new().serve(b);
            let mut transcript = Transcript::default();
            if earlier_endpoint {
                transcript
                    .handle(r#"{"type":"TranscriptText","data":"First."}"#)
                    .unwrap();
                transcript
                    .handle(r#"{"type":"TranscriptEndpoint"}"#)
                    .unwrap();
            }
            let server_task = tokio::spawn(async move {
                let mut audio = Vec::new();
                while let Some(Ok(message)) = server.next().await {
                    if message.is_binary() {
                        audio.extend_from_slice(message.as_payload());
                    }
                    if message.as_text() == Some(r#"{"type":"CloseStream"}"#) {
                        assert_eq!(audio, vec![1, 2]);
                        if !earlier_endpoint {
                            server
                                .send(Message::text(
                                    r#"{"type":"TranscriptText","data":"First."}"#,
                                ))
                                .await
                                .unwrap();
                            server
                                .send(Message::text(r#"{"type":"TranscriptEndpoint"}"#))
                                .await
                                .unwrap();
                        }
                        // Keep the socket open without confirming finalization.
                        std::future::pending::<()>().await;
                    }
                }
            });
            let (tx, rx) = mpsc::channel(1);
            tx.send(Ok(vec![1, 2])).await.unwrap();
            drop(tx);
            let error = session(socket, rx, &mut transcript, Duration::from_millis(50))
                .await
                .unwrap_err();
            assert!(error.to_string().contains("Finalization timed out"));
            assert_eq!(transcript.text(), "First.");
            server_task.abort();
        }
    }

    #[tokio::test]
    async fn capture_errors_prevent_success_and_preserve_text() {
        let (a, b) = tokio::io::duplex(8192);
        let socket = ClientBuilder::new().take_over(a);
        let _server = tokio_websockets::ServerBuilder::new().serve(b);
        let (tx, rx) = mpsc::channel(1);
        tx.send(Err("microphone lost".into())).await.unwrap();
        let mut transcript = Transcript {
            pending: "available".into(),
            ..Default::default()
        };
        let error = session(socket, rx, &mut transcript, Duration::from_millis(50))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "microphone lost");
        assert_eq!(transcript.text(), "available");
    }

    #[tokio::test]
    async fn cancellation_drops_audio_without_connecting() {
        let (tx, rx) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let outcome = transcribe(Config::default(), "unused".into(), rx, cancel).await;
        assert_eq!(outcome.error.as_deref(), Some("Cancelled"));
        assert!(outcome.text.is_empty());
        assert!(tx.is_closed());
    }

    #[tokio::test]
    async fn server_rejection_never_commits_even_after_endpoint() {
        let (a, b) = tokio::io::duplex(8192);
        let socket = ClientBuilder::new().take_over(a);
        let mut server = tokio_websockets::ServerBuilder::new().serve(b);
        let task = tokio::spawn(async move {
            while let Some(Ok(message)) = server.next().await {
                if message.as_text() == Some("{\"type\":\"CloseStream\"}") {
                    server
                        .send(Message::text(
                            r#"{"type":"TranscriptText","data":"available"}"#,
                        ))
                        .await
                        .unwrap();
                    server
                        .send(Message::text(r#"{"type":"TranscriptEndpoint"}"#))
                        .await
                        .unwrap();
                    server
                        .send(Message::close(
                            Some(tokio_websockets::CloseCode::try_from(1008).unwrap()),
                            "rejected",
                        ))
                        .await
                        .unwrap();
                    break;
                }
            }
        });
        let (tx, rx) = mpsc::channel(1);
        drop(tx);
        let mut transcript = Transcript::default();
        assert!(
            session(socket, rx, &mut transcript, Duration::from_secs(1))
                .await
                .unwrap_err()
                .to_string()
                .contains("1008")
        );
        assert_eq!(transcript.text(), "available");
        task.await.unwrap();
    }
}
