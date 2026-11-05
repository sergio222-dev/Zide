//! One language server process: the handshake, document sync and the requests we use.

use std::{
    collections::HashMap,
    path::Path,
    process::{Child, Command, Stdio},
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

use lsp_types::Location;
use serde_json::{Value, json};

use crate::{
    connection::{Connection, Error},
    text::path_to_uri,
};

/// How to run the server of some languages.
#[derive(Debug, PartialEq, Eq)]
pub struct ServerSpec {
    /// Identifies the server: files whose languages share a server share its process.
    pub name: &'static str,
    pub command: &'static str,
    pub args: &'static [&'static str],
    /// What to run to get it, shown when the server is missing; `{dir}` stands for Zide's
    /// folder of servers.
    pub install: &'static str,
}

const RUST: ServerSpec = ServerSpec {
    name: "rust",
    command: "rust-analyzer",
    args: &[],
    install: "rustup component add rust-analyzer",
};

const TYPESCRIPT: ServerSpec = ServerSpec {
    name: "typescript",
    command: "typescript-language-server",
    args: &["--stdio"],
    install: "npm install --prefix {dir} typescript-language-server typescript@5",
};

/// The server for the file at `path` and the `languageId` it is opened with.
pub fn server_for(path: &str) -> Option<(&'static ServerSpec, &'static str)> {
    match path.rsplit_once('.')?.1 {
        "rs" => Some((&RUST, "rust")),
        "ts" | "mts" | "cts" => Some((&TYPESCRIPT, "typescript")),
        "tsx" => Some((&TYPESCRIPT, "typescriptreact")),
        "js" | "mjs" | "cjs" => Some((&TYPESCRIPT, "javascript")),
        "jsx" => Some((&TYPESCRIPT, "javascriptreact")),
        _ => None,
    }
}

const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a request waits for the server to finish loading the project.
const LOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// What the server says about being busy: work-done progress and, for servers that send it
/// (rust-analyzer), its own `experimental/serverStatus`.
#[derive(Default)]
struct Activity {
    /// Progress items begun and not yet ended.
    active: usize,
    quiescent: Option<bool>,
    /// Notifications of either kind seen so far.
    signals: usize,
}

#[derive(Default)]
struct Busy {
    state: Mutex<Activity>,
    changed: Condvar,
}

impl Busy {
    fn notification(&self, method: &str, params: &Value) {
        let mut state = self.state.lock().unwrap();
        match method {
            "$/progress" => match params["value"]["kind"].as_str() {
                Some("begin") => state.active += 1,
                Some("end") => state.active = state.active.saturating_sub(1),
                _ => return,
            },
            "experimental/serverStatus" => state.quiescent = params["quiescent"].as_bool(),
            _ => return,
        }
        state.signals += 1;
        self.changed.notify_all();
    }

    /// Waits until nothing is going on, for at most `max`. A server that never says anything
    /// is taken as idle after a short grace period.
    fn wait_idle(&self, since: Instant, max: Duration) {
        const GRACE: Duration = Duration::from_secs(2);
        let deadline = Instant::now() + max;
        let mut state = self.state.lock().unwrap();
        loop {
            let silent_for_long = state.signals == 0 && since.elapsed() >= GRACE;
            let idle = state.active == 0 && state.quiescent != Some(false);
            if (idle && (state.signals > 0 || silent_for_long)) || Instant::now() >= deadline {
                return;
            }
            let wait = Duration::from_millis(100).min(deadline - Instant::now());
            state = self.changed.wait_timeout(state, wait).unwrap().0;
        }
    }
}

pub struct Client {
    connection: Connection,
    busy: Arc<Busy>,
    started: Instant,
    child: Option<Mutex<Child>>,
    /// Version of each open document, by URI.
    versions: Mutex<HashMap<String, i32>>,
}

impl Client {
    /// Runs the server of `spec` with `root` as its workspace and waits for its handshake.
    pub fn start(spec: &ServerSpec, root: &Path) -> Result<Self, Error> {
        let mut child = Command::new(crate::servers::resolve(spec.command))
            .args(spec.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => Error::NotInstalled(spec.command),
                _ => Error::Io(e),
            })?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(Error::Closed);
        };
        let busy = Arc::new(Busy::default());
        let connection = Connection::new(stdout, stdin, {
            let busy = busy.clone();
            move |method, params| busy.notification(method, &params)
        });
        match Self::handshake(connection, busy, root) {
            Ok(mut client) => {
                client.child = Some(Mutex::new(child));
                Ok(client)
            }
            // a program that starts and quits at once (a rustup shim without the component)
            Err(Error::Closed) => {
                let status = child
                    .wait()
                    .map_or_else(|e| e.to_string(), |s| s.to_string());
                Err(Error::Server {
                    code: 0,
                    message: format!("{} exited during startup ({status})", spec.command),
                })
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(e)
            }
        }
    }

    /// The handshake over an existing connection.
    #[cfg(test)]
    pub(crate) fn with_connection(connection: Connection, root: &Path) -> Result<Self, Error> {
        Self::handshake(connection, Arc::default(), root)
    }

    fn handshake(connection: Connection, busy: Arc<Busy>, root: &Path) -> Result<Self, Error> {
        let uri = path_to_uri(root);
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        connection.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": uri,
                "workspaceFolders": [{"uri": uri, "name": name}],
                "capabilities": {
                    "textDocument": {
                        "synchronization": {"didSave": false},
                        "references": {},
                    },
                    "workspace": {"workspaceFolders": true, "configuration": true},
                    "window": {"workDoneProgress": true},
                    "experimental": {"serverStatusNotification": true},
                },
            }),
            INITIALIZE_TIMEOUT,
        )?;
        connection
            .notify("initialized", json!({}))
            .map_err(Error::Io)?;
        Ok(Self {
            connection,
            busy,
            started: Instant::now(),
            child: None,
            versions: Mutex::default(),
        })
    }

    /// Tells the server the document is open with `text`, or its new full `text` when it
    /// already was.
    pub fn sync(&self, path: &Path, language_id: &str, text: &str) -> Result<(), Error> {
        let uri = path_to_uri(path);
        let mut versions = self.versions.lock().unwrap();
        match versions.get_mut(&uri) {
            None => {
                versions.insert(uri.clone(), 1);
                self.connection
                    .notify(
                        "textDocument/didOpen",
                        json!({"textDocument": {
                            "uri": uri, "languageId": language_id, "version": 1, "text": text,
                        }}),
                    )
                    .map_err(Error::Io)
            }
            Some(version) => {
                *version += 1;
                self.connection
                    .notify(
                        "textDocument/didChange",
                        json!({
                            "textDocument": {"uri": uri, "version": *version},
                            "contentChanges": [{"text": text}],
                        }),
                    )
                    .map_err(Error::Io)
            }
        }
    }

    /// Where the symbol at `line`:`character` (0-based, UTF-16) is used, its declaration included.
    pub fn references(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Vec<Location>, Error> {
        // right after startup a server is still loading the project and would say "none"
        self.busy.wait_idle(self.started, LOAD_TIMEOUT);
        let result = self.connection.request(
            "textDocument/references",
            json!({
                "textDocument": {"uri": path_to_uri(path)},
                "position": {"line": line, "character": character},
                "context": {"includeDeclaration": true},
            }),
            REQUEST_TIMEOUT,
        )?;
        if result.is_null() {
            return Ok(Vec::new());
        }
        serde_json::from_value::<Vec<Location>>(result).map_err(|e| Error::Server {
            code: 0,
            message: format!("unexpected references answer: {e}"),
        })
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // a polite goodbye, then make sure the process is gone
        let _ = self
            .connection
            .request("shutdown", Value::Null, Duration::from_secs(2));
        let _ = self.connection.notify("exit", Value::Null);
        if let Some(child) = &self.child {
            let mut child = child.lock().unwrap();
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;
    use crate::connection::testing::*;

    #[test]
    fn files_pick_their_server_and_language() {
        assert_eq!(
            server_for("src/a.rs").map(|s| (s.0.name, s.1)),
            Some(("rust", "rust"))
        );
        assert_eq!(server_for("a.tsx").map(|s| s.1), Some("typescriptreact"));
        assert_eq!(
            server_for("a.mjs").map(|s| (s.0.name, s.1)),
            Some(("typescript", "javascript"))
        );
        assert_eq!(server_for("README.md"), None);
        assert_eq!(server_for("Makefile"), None);
    }

    fn owned(connection: std::sync::Arc<Connection>) -> Connection {
        std::sync::Arc::try_unwrap(connection)
            .ok()
            .expect("only the test holds the connection")
    }

    /// Plays the server: answers `initialize`, then each request by `answer(method)`.
    fn serve(
        wire: Wire,
        server: std::sync::mpsc::Sender<Vec<u8>>,
        answer: fn(&str) -> Value,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let mut answered = 0;
            loop {
                let messages = wire.wait_for(answered + 1);
                let Some(message) = messages.get(answered) else {
                    return;
                };
                answered += 1;
                let (Some(id), Some(method)) = (message.get("id"), message["method"].as_str())
                else {
                    continue;
                };
                let result = if method == "initialize" {
                    json!({"capabilities": {}})
                } else {
                    answer(method)
                };
                if server
                    .send(frame(json!({"jsonrpc": "2.0", "id": id, "result": result})))
                    .is_err()
                {
                    return;
                }
            }
        })
    }

    #[test]
    fn the_handshake_sends_the_root_and_then_initialized() {
        let (connection, wire, server) = connect(|_, _| {});
        let _server = serve(wire.clone(), server, |_| Value::Null);
        let client =
            Client::with_connection(owned(connection), Path::new("/work/my proj")).unwrap();
        let sent = wire.wait_for(2);
        assert_eq!(sent[0]["method"], "initialize");
        assert_eq!(sent[0]["params"]["rootUri"], "file:///work/my%20proj");
        assert_eq!(sent[1]["method"], "initialized");
        drop(client);
    }

    #[test]
    fn documents_open_once_and_then_change_with_growing_versions() {
        let (connection, wire, server) = connect(|_, _| {});
        let _server = serve(wire.clone(), server, |_| Value::Null);
        let client = Client::with_connection(owned(connection), Path::new("/w")).unwrap();
        client.sync(Path::new("/w/a.rs"), "rust", "one").unwrap();
        client.sync(Path::new("/w/a.rs"), "rust", "two").unwrap();
        let sent = wire.wait_for(4);
        assert_eq!(sent[2]["method"], "textDocument/didOpen");
        assert_eq!(sent[2]["params"]["textDocument"]["languageId"], "rust");
        assert_eq!(sent[3]["method"], "textDocument/didChange");
        assert_eq!(sent[3]["params"]["textDocument"]["version"], 2);
        assert_eq!(sent[3]["params"]["contentChanges"][0]["text"], "two");
        drop(client);
    }

    #[test]
    fn references_come_back_as_locations() {
        let (connection, wire, server) = connect(|_, _| {});
        let _server = serve(wire.clone(), server, |method| {
            assert_eq!(method, "textDocument/references");
            json!([{"uri": "file:///w/a.rs", "range": {
                "start": {"line": 3, "character": 4}, "end": {"line": 3, "character": 7}}}])
        });
        let client = Client::with_connection(owned(connection), Path::new("/w")).unwrap();
        let found = client.references(Path::new("/w/a.rs"), 3, 5).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].uri.as_str(), "file:///w/a.rs");
        assert_eq!(found[0].range.start.line, 3);
        let sent = wire.messages();
        let request = sent
            .iter()
            .find(|m| m["method"] == "textDocument/references")
            .unwrap();
        assert_eq!(request["params"]["context"]["includeDeclaration"], true);
        drop(client);
    }

    #[test]
    fn a_null_answer_means_no_references() {
        let (connection, wire, server) = connect(|_, _| {});
        let _server = serve(wire.clone(), server, |_| Value::Null);
        let client = Client::with_connection(owned(connection), Path::new("/w")).unwrap();
        assert!(
            client
                .references(Path::new("/w/a.rs"), 0, 0)
                .unwrap()
                .is_empty()
        );
        drop(client);
    }

    #[test]
    fn a_missing_server_is_reported_by_name() {
        let spec = ServerSpec {
            name: "none",
            command: "zide-no-such-language-server",
            args: &[],
            install: "",
        };
        let error = Client::start(&spec, Path::new("/tmp")).err().unwrap();
        assert!(matches!(
            error,
            Error::NotInstalled("zide-no-such-language-server")
        ));
    }
}
