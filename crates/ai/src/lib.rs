//! Code suggestions from a local model served by Ollama: the text that fits at the caret,
//! given the text before and after it (fill-in-the-middle).
//!
//! One Ollama server is shared by every instance of the editor: [`Server::start`] reuses one
//! that already answers and only starts `ollama serve` when none does. The server it started is
//! stopped when the [`Server`] is dropped; another instance that was using it finds it gone on
//! its next question and starts one again.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The model asked: a base model, the kind trained to fill in the middle.
pub const MODEL: &str = "qwen2.5-coder:1.5b-base";

/// Where Ollama listens unless `OLLAMA_HOST` says otherwise.
const DEFAULT_HOST: &str = "127.0.0.1:11434";

/// How long a started server has to answer.
const START_WAIT: Duration = Duration::from_secs(15);

/// How long the model stays loaded after the last question: it holds GPU memory meanwhile.
const KEEP_ALIVE: &str = "10m";

/// The text sent around the caret, in chars: enough context for a small model, short enough
/// to answer quickly.
const PREFIX_CHARS: usize = 6000;
const SUFFIX_CHARS: usize = 2000;

/// The text before the caret starts on a multiple of this many chars (then at the next line),
/// so it stays put while typing: the server keeps what it read of the start of the last prompt
/// and only reads what changed after it.
const BLOCK: usize = 2048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// `ollama` is not installed (not on the PATH).
    NotInstalled,
    /// No server answers (it was stopped, or never came up).
    Unreachable,
    /// The server answered with an error.
    Server(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotInstalled => f.write_str("ollama is not installed"),
            Error::Unreachable => f.write_str("ollama does not answer"),
            Error::Server(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

fn failed(error: ureq::Error) -> Error {
    match error {
        ureq::Error::StatusCode(code) => Error::Server(format!("ollama answered {code}")),
        ureq::Error::ConnectionFailed | ureq::Error::Io(_) | ureq::Error::HostNotFound => {
            Error::Unreachable
        }
        other => Error::Server(other.to_string()),
    }
}

/// The base URL of the server, from `OLLAMA_HOST` (`host`, `host:port` or a URL) when set.
fn base_url(host: Option<&str>) -> String {
    let host = host.map(str::trim).filter(|h| !h.is_empty());
    let host = host.unwrap_or(DEFAULT_HOST).trim_end_matches('/');
    let (scheme, rest) = match host.split_once("://") {
        Some((scheme, rest)) => (scheme, rest),
        None => ("http", host),
    };
    // a server listening on every address is reached on this one
    let rest = rest.replace("0.0.0.0", "127.0.0.1");
    let has_port = rest
        .rsplit_once(':')
        .is_some_and(|(_, p)| p.parse::<u16>().is_ok());
    if has_port {
        format!("{scheme}://{rest}")
    } else {
        format!("{scheme}://{rest}:11434")
    }
}

/// A running Ollama server, and the process when this instance started it.
pub struct Server {
    base: String,
    agent: ureq::Agent,
    child: Option<Child>,
}

impl Server {
    /// Reuses the server that answers where `OLLAMA_HOST` says, or starts `ollama serve` and
    /// waits until it answers. Blocks: run it off the UI thread.
    pub fn start() -> Result<Self, Error> {
        let base = base_url(std::env::var("OLLAMA_HOST").ok().as_deref());
        let mut serve = Command::new("ollama");
        serve.arg("serve");
        Self::start_with(base, serve, START_WAIT)
    }

    fn start_with(base: String, mut serve: Command, wait: Duration) -> Result<Self, Error> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(1)))
            .build()
            .into();
        let mut server = Self {
            base,
            agent,
            child: None,
        };
        if server.answers() {
            return Ok(server);
        }
        let child = serve
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => Error::NotInstalled,
                _ => Error::Server(e.to_string()),
            })?;
        server.child = Some(child);
        let deadline = Instant::now() + wait;
        loop {
            if server.answers() {
                // another instance started one at the same time: ours could not take the port
                if let Some(child) = &mut server.child
                    && child.try_wait().ok().flatten().is_some()
                {
                    server.child = None;
                }
                return Ok(server);
            }
            if Instant::now() >= deadline {
                return Err(Error::Unreachable); // dropping `server` stops what was started
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    /// Whether this instance started the server (and so stops it when dropped).
    pub fn owned(&self) -> bool {
        self.child.is_some()
    }

    /// Whether the server answers now.
    pub fn answers(&self) -> bool {
        self.agent
            .get(format!("{}/api/version", self.base))
            .call()
            .is_ok()
    }

    /// Whether the server has `model` downloaded.
    pub fn has_model(&self, model: &str) -> Result<bool, Error> {
        let text = self
            .agent
            .get(format!("{}/api/tags", self.base))
            .call()
            .map_err(failed)?
            .body_mut()
            .read_to_string()
            .map_err(failed)?;
        let tags: Value = serde_json::from_str(&text).map_err(|e| Error::Server(e.to_string()))?;
        let names = tags["models"].as_array().into_iter().flatten();
        Ok(names
            .filter_map(|m| m["name"].as_str())
            .any(|name| name == model))
    }

    /// Downloads `model`. Takes as long as the download does.
    pub fn pull(&self, model: &str) -> Result<(), Error> {
        let body = json!({"model": model, "stream": false}).to_string();
        let text = self
            .agent
            .post(format!("{}/api/pull", self.base))
            .header("Content-Type", "application/json")
            .send(body)
            .map_err(failed)?
            .body_mut()
            .read_to_string()
            .map_err(failed)?;
        let answer: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        match answer["error"].as_str() {
            Some(error) => Err(Error::Server(error.to_string())),
            None => Ok(()),
        }
    }

    /// What `model` writes at char offset `at` of `text`, read as it streams so that raising
    /// `cancel` stops it (closing the connection stops the generation too): `None` then.
    pub fn complete(
        &self,
        model: &str,
        text: &str,
        at: usize,
        cancel: &AtomicBool,
    ) -> Result<Option<String>, Error> {
        let body = json!({
            "model": model,
            "prompt": fim_prompt(text, at),
            "raw": true,
            "stream": true,
            "keep_alive": KEEP_ALIVE,
            "options": {
                "temperature": 0.2,
                "num_predict": 96,
                "stop": ["\n\n", "<|endoftext|>", "<|fim_pad|>", "<|file_sep|>", "<|im_start|>"],
            },
        })
        .to_string();
        let response = self
            .agent
            .post(format!("{}/api/generate", self.base))
            .header("Content-Type", "application/json")
            .send(body)
            .map_err(failed)?;
        let mut text = String::new();
        for line in BufReader::new(response.into_body().into_reader()).lines() {
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let line = line.map_err(|_| Error::Unreachable)?;
            let Ok(chunk) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(error) = chunk["error"].as_str() {
                return Err(Error::Server(error.to_string()));
            }
            text.push_str(chunk["response"].as_str().unwrap_or_default());
            if chunk["done"].as_bool() == Some(true) {
                break;
            }
        }
        Ok((!cancel.load(Ordering::Relaxed)).then_some(text))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Qwen2.5-coder's fill-in-the-middle prompt for the caret at char offset `at` of `text`: up
/// to [`PREFIX_CHARS`] before it, from a start that stays put while typing (see
/// [`prefix_start`]), and the start of the text after it.
pub fn fim_prompt(text: &str, at: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let at = at.min(chars.len());
    let prefix: String = chars[prefix_start(&chars, at)..at].iter().collect();
    let suffix: String = chars[at..].iter().take(SUFFIX_CHARS).collect();
    format!("<|fim_prefix|>{prefix}<|fim_suffix|>{suffix}<|fim_middle|>")
}

/// Where the text sent before the caret at `at` starts: the first multiple of [`BLOCK`] at
/// most [`PREFIX_CHARS`] back, then the start of the line after it. It only moves when the
/// caret crosses into another block, not at every key.
fn prefix_start(chars: &[char], at: usize) -> usize {
    let earliest = at.saturating_sub(PREFIX_CHARS);
    if earliest == 0 {
        return 0;
    }
    let block = earliest.div_ceil(BLOCK) * BLOCK;
    match chars[block..at].iter().position(|&c| c == '\n') {
        Some(newline) => block + newline + 1,
        None => block,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// A server that answers each request with `answer(method, path, body)`, keeping the
    /// requests it got.
    fn fake(
        listener: TcpListener,
        answer: fn(&str, &str, &str) -> String,
    ) -> Arc<Mutex<Vec<(String, String)>>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut request = Vec::new();
                let mut buf = [0u8; 4096];
                // headers, then as many body bytes as Content-Length says
                loop {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&request);
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text[..end]
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let text = String::from_utf8_lossy(&request).to_string();
                let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
                let mut first = head.lines().next().unwrap_or("").split(' ');
                let method = first.next().unwrap_or("").to_string();
                let path = first.next().unwrap_or("").to_string();
                log.lock().unwrap().push((path.clone(), body.to_string()));
                let reply = answer(&method, &path, body);
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
        });
        seen
    }

    fn listener() -> (TcpListener, String) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        (listener, base)
    }

    fn never() -> Command {
        Command::new("/nonexistent/ollama")
    }

    #[test]
    fn the_host_comes_from_ollama_host_in_any_of_its_forms() {
        assert_eq!(base_url(None), "http://127.0.0.1:11434");
        assert_eq!(base_url(Some("")), "http://127.0.0.1:11434");
        assert_eq!(base_url(Some("0.0.0.0")), "http://127.0.0.1:11434");
        assert_eq!(base_url(Some("box:8080")), "http://box:8080");
        assert_eq!(base_url(Some("https://box/")), "https://box:11434");
    }

    #[test]
    fn a_server_that_answers_is_reused_and_not_started_again() {
        let (listener, base) = listener();
        fake(listener, |_, _, _| r#"{"version":"0.9.0"}"#.into());
        // the command would fail: it is never run
        let server = Server::start_with(base, never(), Duration::from_secs(1)).unwrap();
        assert!(!server.owned());
    }

    #[test]
    fn without_a_server_or_ollama_installed_starting_says_so() {
        let (listener, base) = listener();
        drop(listener); // nothing answers there
        let error = Server::start_with(base, never(), Duration::from_secs(1)).err();
        assert_eq!(error, Some(Error::NotInstalled));
    }

    #[test]
    fn a_started_server_is_owned_and_stopped_when_dropped() {
        let (listener, base) = listener();
        let address = listener.local_addr().unwrap();
        drop(listener);
        // the "server" comes up a moment after it is started
        let mut serve = Command::new("sleep");
        serve.arg("30");
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            fake(TcpListener::bind(address).unwrap(), |_, _, _| "{}".into());
        });
        let server = Server::start_with(base, serve, Duration::from_secs(5)).unwrap();
        assert!(server.owned());
        let pid = server.child.as_ref().unwrap().id();
        drop(server);
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    }

    #[test]
    fn a_server_that_never_answers_fails_and_is_stopped() {
        let (listener, base) = listener();
        drop(listener);
        let mut serve = Command::new("sleep");
        serve.arg("30");
        let started = Instant::now();
        let error = Server::start_with(base, serve, Duration::from_millis(300)).err();
        assert_eq!(error, Some(Error::Unreachable));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn the_model_is_found_by_its_full_name() {
        let (listener, base) = listener();
        fake(listener, |_, path, _| match path {
            "/api/tags" => {
                r#"{"models":[{"name":"llama3:latest"},{"name":"qwen2.5-coder:1.5b-base"}]}"#.into()
            }
            _ => "{}".into(),
        });
        let server = Server::start_with(base, never(), Duration::from_secs(1)).unwrap();
        assert!(server.has_model(MODEL).unwrap());
        assert!(!server.has_model("qwen2.5-coder:7b").unwrap());
    }

    #[test]
    fn pulling_asks_for_the_model_and_reports_its_error() {
        let (listener, base) = listener();
        let seen = fake(listener, |_, path, body| match path {
            "/api/pull" if body.contains("missing") => r#"{"error":"file does not exist"}"#.into(),
            _ => r#"{"status":"success"}"#.into(),
        });
        let server = Server::start_with(base, never(), Duration::from_secs(1)).unwrap();
        server.pull(MODEL).unwrap();
        assert_eq!(
            server.pull("missing").err(),
            Some(Error::Server("file does not exist".into()))
        );
        let seen = seen.lock().unwrap();
        let (_, body) = seen.iter().find(|(p, _)| p == "/api/pull").unwrap();
        let body: Value = serde_json::from_str(body).unwrap();
        assert_eq!(body["model"], MODEL);
        assert_eq!(body["stream"], false);
    }

    #[test]
    fn a_completion_joins_the_streamed_pieces_of_a_raw_fim_prompt() {
        let (listener, base) = listener();
        let seen = fake(listener, |_, path, _| match path {
            "/api/generate" => concat!(
                r#"{"response":"a + ","done":false}"#,
                "\n",
                r#"{"response":"b","done":false}"#,
                "\n",
                r#"{"response":"","done":true}"#,
                "\n"
            )
            .into(),
            _ => "{}".into(),
        });
        let server = Server::start_with(base, never(), Duration::from_secs(1)).unwrap();
        let text = server
            .complete(
                MODEL,
                "fn add(a: u8, b: u8) -> u8 {\n    \n}\n",
                33,
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(text.as_deref(), Some("a + b"));
        let seen = seen.lock().unwrap();
        let (_, body) = seen.iter().find(|(p, _)| p == "/api/generate").unwrap();
        let body: Value = serde_json::from_str(body).unwrap();
        assert_eq!(body["raw"], true);
        assert_eq!(body["model"], MODEL);
        assert_eq!(
            body["prompt"],
            "<|fim_prefix|>fn add(a: u8, b: u8) -> u8 {\n    <|fim_suffix|>\n}\n<|fim_middle|>"
        );
        assert_eq!(body["keep_alive"], KEEP_ALIVE);
    }

    #[test]
    fn a_cancelled_completion_gives_nothing_and_an_error_is_reported() {
        let (listener, base) = listener();
        fake(listener, |_, _, body| {
            if body.contains("broken") {
                r#"{"error":"model not found"}"#.to_string() + "\n"
            } else {
                r#"{"response":"x","done":true}"#.to_string() + "\n"
            }
        });
        let server = Server::start_with(base, never(), Duration::from_secs(1)).unwrap();
        let cancelled = AtomicBool::new(true);
        assert_eq!(server.complete(MODEL, "a", 1, &cancelled).unwrap(), None);
        assert_eq!(
            server
                .complete(MODEL, "broken", 6, &AtomicBool::new(false))
                .err(),
            Some(Error::Server("model not found".into()))
        );
    }

    /// The text before the caret in the prompt.
    fn prefix_of(prompt: &str) -> &str {
        let end = prompt.find("<|fim_suffix|>").unwrap();
        &prompt["<|fim_prefix|>".len()..end]
    }

    #[test]
    fn a_short_text_goes_whole() {
        let prompt = fim_prompt("ab\ncd", 4);
        assert_eq!(prompt, "<|fim_prefix|>ab\nc<|fim_suffix|>d<|fim_middle|>");
        assert_eq!(
            fim_prompt("ab", 9),
            "<|fim_prefix|>ab<|fim_suffix|><|fim_middle|>"
        );
    }

    #[test]
    fn the_text_before_the_caret_starts_on_a_line_and_stays_put_while_typing() {
        // lines of 10 chars: `000000000\n111111111\n`...
        let text: String = (0..2000).map(|i| format!("{:09}\n", i % 10)).collect();
        let at = 9000;
        let start = prefix_start(&text.chars().collect::<Vec<_>>(), at);
        assert!(at - start <= PREFIX_CHARS);
        assert!(at - start > PREFIX_CHARS - BLOCK - 10);
        assert_eq!(text.chars().nth(start - 1), Some('\n')); // the start of a line
        let prefix = prefix_of(&fim_prompt(&text, at)).to_string();
        assert!(text[..at].ends_with(&prefix));
        // typing at the caret keeps the start, so the server reuses what it read
        let mut typed = text.clone();
        for step in 1..=200 {
            typed.insert(at + step - 1, 'x');
            let later = fim_prompt(&typed, at + step);
            assert!(
                prefix_of(&later).starts_with(&prefix),
                "moved after {step} keys"
            );
        }
        // past the block, it moves on once
        let far = prefix_start(&text.chars().collect::<Vec<_>>(), at + BLOCK);
        assert!(far > start);
    }

    #[test]
    fn only_the_start_of_the_text_after_the_caret_goes() {
        let text = "a".to_string() + &"z".repeat(SUFFIX_CHARS + 50);
        let prompt = fim_prompt(&text, 1);
        let suffix = &prompt[prompt.find("<|fim_suffix|>").unwrap() + "<|fim_suffix|>".len()..];
        assert_eq!(suffix, "z".repeat(SUFFIX_CHARS) + "<|fim_middle|>");
    }

    #[test]
    fn a_server_that_went_away_is_unreachable() {
        let (listener, base) = listener();
        let server = {
            fake(listener, |_, _, _| "{}".into());
            Server::start_with(base.clone(), never(), Duration::from_secs(1)).unwrap()
        };
        let gone = Server {
            base: base_url(Some("127.0.0.1:1")),
            agent: server.agent.clone(),
            child: None,
        };
        assert!(!gone.answers());
        assert_eq!(
            gone.complete(MODEL, "", 0, &AtomicBool::new(false)).err(),
            Some(Error::Unreachable)
        );
    }
}
