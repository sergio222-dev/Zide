//! One language server process: the handshake, document sync and the requests we use.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

use lsp_types::{
    Diagnostic, DocumentHighlight, InlayHint, InlayHintKind, InlayHintLabel, Location, Position,
    Range, TextEdit,
};
use serde_json::{Value, json};

use crate::{
    connection::{Connection, Error},
    text::{path_to_uri, uri_to_path},
};

/// What the server last published about each file.
type Published = Arc<Mutex<HashMap<PathBuf, Vec<Diagnostic>>>>;

/// Stores a `textDocument/publishDiagnostics` notification; true when it was one.
fn store_diagnostics(published: &Published, method: &str, params: &Value) -> bool {
    if method != "textDocument/publishDiagnostics" {
        return false;
    }
    let Some(path) = params["uri"].as_str().and_then(uri_to_path) else {
        return false;
    };
    let diagnostics: Vec<Diagnostic> =
        serde_json::from_value(params["diagnostics"].clone()).unwrap_or_default();
    published.lock().unwrap().insert(path, diagnostics);
    true
}

/// The name of a parameter, to show before the argument at `position` (`id:`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterHint {
    pub position: Position,
    pub label: String,
}

/// How to run the server of some languages.
#[derive(Debug)]
pub struct ServerSpec {
    /// Identifies the server: files whose languages share a server share its process.
    pub name: &'static str,
    pub command: &'static str,
    pub args: &'static [&'static str],
    /// What to run to get it, shown when the server is missing; `{dir}` stands for Zide's
    /// folder of servers.
    pub install: &'static str,
    /// A faster build of the same server, preferred when installed in Zide's folder: where to
    /// find it and the arguments it takes.
    pub native: Option<Native>,
}

#[derive(Debug)]
pub struct Native {
    pub find: fn(&Path) -> Option<PathBuf>,
    pub args: &'static [&'static str],
}

const RUST: ServerSpec = ServerSpec {
    name: "rust",
    command: "rust-analyzer",
    args: &[],
    install: "rustup component add rust-analyzer",
    // rust-glancer keeps ~100 MB where rust-analyzer needs ~4 GB on a workspace this size, but
    // has no diagnostics and no proc-macro support: it is used only when installed.
    native: Some(Native {
        find: crate::servers::native_rust,
        args: &["lsp"],
    }),
};

const TYPESCRIPT: ServerSpec = ServerSpec {
    name: "typescript",
    command: "typescript-language-server",
    args: &["--stdio"],
    install: "npm install --prefix {dir} typescript",
    native: Some(Native {
        find: crate::servers::native_typescript,
        args: &["--lsp", "--stdio"],
    }),
};

const GO: ServerSpec = ServerSpec {
    name: "go",
    command: "gopls",
    args: &[],
    install: "GOBIN={dir}/bin go install golang.org/x/tools/gopls@latest",
    native: None,
};

const PYTHON: ServerSpec = ServerSpec {
    name: "python",
    command: "pyright-langserver",
    args: &["--stdio"],
    install: "npm install --prefix {dir} pyright",
    native: None,
};

/// The server for the file at `path` and the `languageId` it is opened with.
pub fn server_for(path: &str) -> Option<(&'static ServerSpec, &'static str)> {
    match path.rsplit_once('.')?.1 {
        "rs" => Some((&RUST, "rust")),
        "ts" | "mts" | "cts" => Some((&TYPESCRIPT, "typescript")),
        "tsx" => Some((&TYPESCRIPT, "typescriptreact")),
        "js" | "mjs" | "cjs" => Some((&TYPESCRIPT, "javascript")),
        "jsx" => Some((&TYPESCRIPT, "javascriptreact")),
        "go" => Some((&GO, "go")),
        "py" | "pyi" => Some((&PYTHON, "python")),
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
    /// What to call the running server: its command, or `<name> (native)`.
    label: String,
    /// Version of each open document, by URI.
    versions: Mutex<HashMap<String, (i32, u64)>>,
    published: Published,
    /// The server answers `textDocument/diagnostic` (some, like TypeScript 7, never publish).
    pulls_diagnostics: bool,
    /// The server answers `textDocument/inlayHint`.
    has_inlay_hints: bool,
}

impl Client {
    /// Runs the server of `spec` with `root` as its workspace and waits for its handshake.
    /// `on_diagnostics` is called, from the server's reader thread, whenever the server
    /// publishes the diagnostics of a file.
    pub fn start(
        spec: &ServerSpec,
        root: &Path,
        on_diagnostics: impl Fn() + Send + 'static,
    ) -> Result<Self, Error> {
        let launch = crate::servers::launch(spec);
        let mut child = Command::new(&launch.program)
            .args(launch.args)
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
        let published = Published::default();
        let connection = Connection::new(stdout, stdin, {
            let busy = busy.clone();
            let published = published.clone();
            move |method, params| {
                busy.notification(method, &params);
                if store_diagnostics(&published, method, &params) {
                    on_diagnostics();
                }
            }
        });
        match Self::handshake(connection, busy, published, root) {
            Ok(mut client) => {
                client.child = Some(Mutex::new(child));
                client.label = if launch.native {
                    format!("{} (native)", spec.name)
                } else {
                    spec.command.to_string()
                };
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
        Self::handshake(connection, Arc::default(), Published::default(), root)
    }

    fn handshake(
        connection: Connection,
        busy: Arc<Busy>,
        published: Published,
        root: &Path,
    ) -> Result<Self, Error> {
        let uri = path_to_uri(root);
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let initialized = connection.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": uri,
                "workspaceFolders": [{"uri": uri, "name": name}],
                "capabilities": {
                    "textDocument": {
                        "synchronization": {"didSave": false},
                        "references": {},
                        "publishDiagnostics": {"relatedInformation": true},
                        "diagnostic": {"dynamicRegistration": false},
                        "inlayHint": {"dynamicRegistration": false},
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
            label: String::new(),
            versions: Mutex::default(),
            published,
            pulls_diagnostics: !initialized["capabilities"]["diagnosticProvider"].is_null(),
            has_inlay_hints: !initialized["capabilities"]["inlayHintProvider"].is_null()
                && initialized["capabilities"]["inlayHintProvider"] != false,
        })
    }

    /// Asks a server that does not publish for the diagnostics of `path` (synced before) and
    /// keeps the answer for `diagnostics`. Does nothing for a server that publishes by itself.
    pub fn pull_diagnostics(&self, path: &Path) -> Result<(), Error> {
        if !self.pulls_diagnostics {
            return Ok(());
        }
        let report = self.connection.request(
            "textDocument/diagnostic",
            json!({"textDocument": {"uri": path_to_uri(path)}}),
            REQUEST_TIMEOUT,
        )?;
        // "unchanged" keeps what was stored by the last full report
        if report["kind"] == "full" {
            let items = serde_json::from_value(report["items"].clone()).unwrap_or_default();
            self.published
                .lock()
                .unwrap()
                .insert(path.to_path_buf(), items);
        }
        Ok(())
    }

    /// The parameter names to show in the first `lines` lines of `path` (synced before): the
    /// hints of kind parameter, the rest (types) are left out.
    pub fn parameter_hints(&self, path: &Path, lines: u32) -> Result<Vec<ParameterHint>, Error> {
        if !self.has_inlay_hints {
            return Ok(Vec::new());
        }
        let result = self.connection.request(
            "textDocument/inlayHint",
            json!({
                "textDocument": {"uri": path_to_uri(path)},
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": lines, "character": 0}},
            }),
            REQUEST_TIMEOUT,
        )?;
        let hints: Vec<InlayHint> = serde_json::from_value(result).unwrap_or_default();
        Ok(hints
            .into_iter()
            .filter(|hint| hint.kind == Some(InlayHintKind::PARAMETER))
            .map(|hint| ParameterHint {
                position: hint.position,
                label: match hint.label {
                    InlayHintLabel::String(label) => label,
                    InlayHintLabel::LabelParts(parts) => {
                        parts.into_iter().map(|part| part.value).collect()
                    }
                },
            })
            .collect())
    }

    /// What the server last published about `path`: empty until it says something.
    pub fn diagnostics(&self, path: &Path) -> Vec<Diagnostic> {
        let published = self.published.lock().unwrap();
        published.get(path).cloned().unwrap_or_default()
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    /// Tells the server the document is open with `text`, or its new full `text` when it
    /// already was. Sending the same text again is free.
    pub fn sync(&self, path: &Path, language_id: &str, text: &str) -> Result<(), Error> {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let uri = path_to_uri(path);
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let fingerprint = hasher.finish();
        let mut versions = self.versions.lock().unwrap();
        match versions.get_mut(&uri) {
            None => {
                versions.insert(uri.clone(), (1, fingerprint));
                self.connection
                    .notify(
                        "textDocument/didOpen",
                        json!({"textDocument": {
                            "uri": uri, "languageId": language_id, "version": 1, "text": text,
                        }}),
                    )
                    .map_err(Error::Io)
            }
            Some((_, known)) if *known == fingerprint => Ok(()),
            Some((version, known)) => {
                *version += 1;
                *known = fingerprint;
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

    /// Where the symbol at `line`:`character` (0-based, UTF-16) appears in its own document,
    /// as ranges.
    pub fn document_highlight(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Vec<Range>, Error> {
        let result = self.connection.request(
            "textDocument/documentHighlight",
            json!({
                "textDocument": {"uri": path_to_uri(path)},
                "position": {"line": line, "character": character},
            }),
            REQUEST_TIMEOUT,
        )?;
        if result.is_null() {
            return Ok(Vec::new());
        }
        serde_json::from_value::<Vec<DocumentHighlight>>(result)
            .map(|found| found.into_iter().map(|h| h.range).collect())
            .map_err(|e| Error::Server {
                code: 0,
                message: format!("unexpected highlight answer: {e}"),
            })
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

    /// Where the symbol at `line`:`character` (0-based, UTF-16) is defined: none when the server
    /// does not know it, several for overloads or re-exports.
    pub fn definition(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Vec<Location>, Error> {
        // right after startup a server is still loading the project and would say "none"
        self.busy.wait_idle(self.started, LOAD_TIMEOUT);
        let result = self.connection.request(
            "textDocument/definition",
            json!({
                "textDocument": {"uri": path_to_uri(path)},
                "position": {"line": line, "character": character},
            }),
            REQUEST_TIMEOUT,
        )?;
        parse_definition(result).map_err(|e| Error::Server {
            code: 0,
            message: format!("unexpected definition answer: {e}"),
        })
    }

    /// The edits that format the document at `path`, or only `range` of it. Positions in the
    /// edits are in the document as last synced.
    pub fn format(&self, path: &Path, range: Option<Range>) -> Result<Vec<TextEdit>, Error> {
        let options = json!({"tabSize": 4, "insertSpaces": true});
        let (method, params) = match range {
            Some(range) => (
                "textDocument/rangeFormatting",
                json!({"textDocument": {"uri": path_to_uri(path)}, "range": range, "options": options}),
            ),
            None => (
                "textDocument/formatting",
                json!({"textDocument": {"uri": path_to_uri(path)}, "options": options}),
            ),
        };
        let result = self.connection.request(method, params, REQUEST_TIMEOUT)?;
        if result.is_null() {
            return Ok(Vec::new());
        }
        serde_json::from_value(result).map_err(|e| Error::Server {
            code: 0,
            message: format!("unexpected formatting answer: {e}"),
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

/// The locations of a `textDocument/definition` answer, which is a `Location`, a list of them,
/// or a list of `LocationLink`s (whose selection range is the name, preferred over the whole body).
fn parse_definition(result: Value) -> Result<Vec<Location>, serde_json::Error> {
    use lsp_types::GotoDefinitionResponse as Answer;
    if result.is_null() {
        return Ok(Vec::new());
    }
    Ok(match serde_json::from_value::<Answer>(result)? {
        Answer::Scalar(location) => vec![location],
        Answer::Array(locations) => locations,
        Answer::Link(links) => links
            .into_iter()
            .map(|link| Location {
                uri: link.target_uri,
                range: link.target_selection_range,
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;
    use crate::connection::testing::*;

    #[test]
    fn a_definition_answer_may_be_a_location_a_list_or_links() {
        let range =
            json!({"start": {"line": 3, "character": 4}, "end": {"line": 3, "character": 9}});
        let location = json!({"uri": "file:///p/a.rs", "range": range});
        let link = json!({
            "targetUri": "file:///p/b.rs",
            "targetRange": {"start": {"line": 1, "character": 0}, "end": {"line": 8, "character": 1}},
            "targetSelectionRange": range,
        });
        let lines = |value| -> Vec<(String, u32)> {
            parse_definition(value)
                .unwrap()
                .iter()
                .map(|l| (l.uri.to_string(), l.range.start.line))
                .collect()
        };
        assert_eq!(lines(Value::Null), []);
        assert_eq!(lines(json!([])), []);
        assert_eq!(lines(location.clone()), [("file:///p/a.rs".to_string(), 3)]);
        assert_eq!(
            lines(json!([location])),
            [("file:///p/a.rs".to_string(), 3)]
        );
        // a link points at the name, not at the start of the whole declaration
        assert_eq!(lines(json!([link])), [("file:///p/b.rs".to_string(), 3)]);
    }

    #[test]
    fn published_diagnostics_are_kept_per_file_and_replaced_by_the_next_publish() {
        let published = Published::default();
        let publish = |count: usize| {
            let diagnostic = json!({
                "range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 5}},
                "severity": 1,
                "message": "Expected 8 arguments, but got 7",
            });
            let params = json!({
                "uri": "file:///tmp/a%20b.ts",
                "diagnostics": vec![diagnostic; count],
            });
            store_diagnostics(&published, "textDocument/publishDiagnostics", &params)
        };
        assert!(publish(2));
        let stored = || published.lock().unwrap()[Path::new("/tmp/a b.ts")].clone();
        assert_eq!(stored().len(), 2);
        assert_eq!(stored()[0].message, "Expected 8 arguments, but got 7");
        assert!(publish(0)); // the file is clean again
        assert!(stored().is_empty());
        assert!(!store_diagnostics(&published, "$/progress", &json!({})));
    }

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
        assert_eq!(
            server_for("cmd/main.go").map(|s| (s.0.name, s.1)),
            Some(("go", "go"))
        );
        assert_eq!(
            server_for("app.pyi").map(|s| (s.0.name, s.1)),
            Some(("python", "python"))
        );
        assert!(server_for("README.md").is_none());
        assert!(server_for("Makefile").is_none());
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
                    json!({"capabilities": {"diagnosticProvider": {}, "inlayHintProvider": true}})
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
    fn syncing_the_same_text_again_sends_nothing() {
        let (connection, wire, server) = connect(|_, _| {});
        let _server = serve(wire.clone(), server, |_| Value::Null);
        let client = Client::with_connection(owned(connection), Path::new("/w")).unwrap();
        let path = Path::new("/w/a.rs");
        client.sync(path, "rust", "one").unwrap();
        client.sync(path, "rust", "one").unwrap();
        client.sync(path, "rust", "two").unwrap();
        let methods: Vec<_> = wire
            .wait_for(4)
            .iter()
            .map(|m| m["method"].as_str().unwrap_or("").to_string())
            .collect();
        assert_eq!(
            &methods[2..],
            ["textDocument/didOpen", "textDocument/didChange"]
        );
        drop(client);
    }

    #[test]
    fn a_server_that_does_not_publish_is_asked_for_the_diagnostics_of_a_file() {
        let (connection, wire, server) = connect(|_, _| {});
        let _server = serve(wire.clone(), server, |method| {
            assert_eq!(method, "textDocument/diagnostic");
            json!({"kind": "full", "items": [{
                "range": {"start": {"line": 3, "character": 10}, "end": {"line": 3, "character": 12}},
                "severity": 1,
                "code": 2339,
                "source": "ts",
                "message": "Property 'id' does not exist",
            }]})
        });
        let client = Client::with_connection(owned(connection), Path::new("/w")).unwrap();
        let path = Path::new("/w/a.ts");
        assert!(client.diagnostics(path).is_empty());
        client.pull_diagnostics(path).unwrap();
        let found = client.diagnostics(path);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].message, "Property 'id' does not exist");
        assert_eq!(
            wire.wait_for(3)[2]["params"]["textDocument"]["uri"],
            "file:///w/a.ts"
        );
        drop(client);
    }

    #[test]
    fn parameter_hints_keep_only_parameter_names_and_join_label_parts() {
        let (connection, wire, server) = connect(|_, _| {});
        let _server = serve(wire.clone(), server, |method| {
            assert_eq!(method, "textDocument/inlayHint");
            json!([
                {"position": {"line": 2, "character": 2}, "kind": 2,
                 "label": [{"value": "id"}, {"value": ":"}]},
                {"position": {"line": 3, "character": 4}, "kind": 2, "label": "name:"},
                {"position": {"line": 3, "character": 9}, "kind": 1, "label": ": string"},
            ])
        });
        let client = Client::with_connection(owned(connection), Path::new("/w")).unwrap();
        let found = client.parameter_hints(Path::new("/w/a.ts"), 10).unwrap();
        let shown: Vec<_> = found
            .iter()
            .map(|h| (h.position.line, h.position.character, h.label.as_str()))
            .collect();
        assert_eq!(shown, [(2, 2, "id:"), (3, 4, "name:")]);
        drop(client);
    }

    #[test]
    fn highlights_come_back_as_ranges() {
        let (connection, wire, server) = connect(|_, _| {});
        let _server = serve(wire.clone(), server, |method| {
            assert_eq!(method, "textDocument/documentHighlight");
            json!([
                {"range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 5}}, "kind": 2},
                {"range": {"start": {"line": 4, "character": 0}, "end": {"line": 4, "character": 3}}}
            ])
        });
        let client = Client::with_connection(owned(connection), Path::new("/w")).unwrap();
        let found = client
            .document_highlight(Path::new("/w/a.rs"), 1, 3)
            .unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[1].start.line, 4);
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
    fn formatting_asks_for_the_whole_document_or_a_range() {
        let (connection, wire, server) = connect(|_, _| {});
        let _server = serve(wire.clone(), server, |_| {
            json!([{"range": {
                "start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 3}},
                "newText": "fn"}])
        });
        let client = Client::with_connection(owned(connection), Path::new("/w")).unwrap();
        let path = Path::new("/w/a.rs");
        let edits = client.format(path, None).unwrap();
        assert_eq!((edits.len(), edits[0].new_text.as_str()), (1, "fn"));
        let range = Range::new(
            lsp_types::Position::new(2, 0),
            lsp_types::Position::new(4, 1),
        );
        client.format(path, Some(range)).unwrap();
        let sent = wire.messages();
        let whole = sent
            .iter()
            .find(|m| m["method"] == "textDocument/formatting")
            .unwrap();
        assert_eq!(whole["params"]["options"]["insertSpaces"], true);
        let part = sent
            .iter()
            .find(|m| m["method"] == "textDocument/rangeFormatting")
            .unwrap();
        assert_eq!(part["params"]["range"]["end"]["line"], 4);
        drop(client);
    }

    #[test]
    fn a_null_formatting_answer_means_nothing_to_change() {
        let (connection, wire, server) = connect(|_, _| {});
        let _server = serve(wire.clone(), server, |_| Value::Null);
        let client = Client::with_connection(owned(connection), Path::new("/w")).unwrap();
        assert!(
            client
                .format(Path::new("/w/a.rs"), None)
                .unwrap()
                .is_empty()
        );
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
            native: None,
        };
        let error = Client::start(&spec, Path::new("/tmp"), || {})
            .err()
            .unwrap();
        assert!(matches!(
            error,
            Error::NotInstalled("zide-no-such-language-server")
        ));
    }
}
