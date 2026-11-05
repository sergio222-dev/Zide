//! JSON-RPC over a pair of byte streams: requests wait for their response, server
//! notifications go to a callback and server requests get an empty answer.

use std::{
    collections::HashMap,
    io::{self, BufReader, Read, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Sender},
    },
    thread,
    time::Duration,
};

use serde_json::{Value, json};

use crate::framing::{read_message, write_message};

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// The server answered with an error.
    Server {
        code: i64,
        message: String,
    },
    Timeout,
    /// The server went away before answering.
    Closed,
    /// The server program is not on the PATH.
    NotInstalled(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Server { message, .. } => write!(f, "{message}"),
            Error::Timeout => write!(f, "the server did not answer in time"),
            Error::Closed => write!(f, "the server closed the connection"),
            Error::NotInstalled(command) => write!(f, "{command} is not installed"),
        }
    }
}

impl std::error::Error for Error {}

type Pending = Arc<Mutex<HashMap<u64, Sender<Result<Value, Error>>>>>;

pub struct Connection {
    writer: Mutex<Box<dyn Write + Send>>,
    pending: Pending,
    next_id: AtomicU64,
}

impl Connection {
    /// Starts reading `reader` on its own thread. `on_notification` gets every notification
    /// the server sends, on that thread.
    pub fn new(
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
        on_notification: impl Fn(&str, Value) + Send + 'static,
    ) -> Self {
        let pending: Pending = Arc::default();
        let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(Box::new(writer)));
        let connection = Self {
            // the reader thread answers server requests through its own handle
            writer: Mutex::new(Box::new(SharedWriter(writer.clone()))),
            pending: pending.clone(),
            next_id: AtomicU64::new(1),
        };
        thread::spawn(move || {
            let mut reader = BufReader::new(reader);
            while let Ok(Some(body)) = read_message(&mut reader) {
                let Ok(message) = serde_json::from_slice::<Value>(&body) else {
                    continue;
                };
                handle_incoming(message, &pending, &writer, &on_notification);
            }
            // the stream ended: everyone still waiting learns it
            for (_, sender) in pending.lock().unwrap().drain() {
                let _ = sender.send(Err(Error::Closed));
            }
        });
        connection
    }

    pub fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, Error> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let sent =
            self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        if let Err(e) = sent {
            self.pending.lock().unwrap().remove(&id);
            return Err(Error::Io(e));
        }
        match rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.pending.lock().unwrap().remove(&id);
                Err(Error::Timeout)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(Error::Closed),
        }
    }

    pub fn notify(&self, method: &str, params: Value) -> io::Result<()> {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    fn send(&self, message: &Value) -> io::Result<()> {
        let body = serde_json::to_vec(message)?;
        write_message(&mut *self.writer.lock().unwrap(), &body)
    }
}

/// A writer that locks the shared one for each write.
struct SharedWriter(Arc<Mutex<Box<dyn Write + Send>>>);

impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.lock().unwrap().flush()
    }
}

fn handle_incoming(
    message: Value,
    pending: &Pending,
    writer: &Mutex<Box<dyn Write + Send>>,
    on_notification: &impl Fn(&str, Value),
) {
    let method = message.get("method").and_then(Value::as_str);
    match (method, message.get("id")) {
        // a response to one of our requests
        (None, Some(id)) => {
            let Some(sender) = id
                .as_u64()
                .and_then(|id| pending.lock().unwrap().remove(&id))
            else {
                return;
            };
            let result = match message.get("error") {
                Some(error) => Err(Error::Server {
                    code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                    message: error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown error")
                        .to_string(),
                }),
                None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = sender.send(result);
        }
        // a request from the server: it only needs to be told "nothing to configure"
        (Some(method), Some(id)) => {
            let result = if method == "workspace/configuration" {
                let items = message["params"]["items"].as_array().map_or(0, Vec::len);
                Value::Array(vec![Value::Null; items])
            } else {
                Value::Null
            };
            let reply = json!({"jsonrpc": "2.0", "id": id, "result": result});
            if let Ok(body) = serde_json::to_vec(&reply) {
                let _ = write_message(&mut *writer.lock().unwrap(), &body);
            }
        }
        (Some(method), None) => on_notification(
            method,
            message.get("params").cloned().unwrap_or(Value::Null),
        ),
        (None, None) => {}
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::{io::Cursor, sync::mpsc::Receiver, time::Instant};

    use super::*;

    /// A reader fed from the test, blocking like a pipe; dropping the sender ends the stream.
    pub struct ChannelReader {
        rx: Receiver<Vec<u8>>,
        current: Cursor<Vec<u8>>,
    }

    impl Read for ChannelReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            loop {
                let read = self.current.read(buf)?;
                if read > 0 {
                    return Ok(read);
                }
                match self.rx.recv() {
                    Ok(next) => self.current = Cursor::new(next),
                    Err(_) => return Ok(0),
                }
            }
        }
    }

    #[derive(Clone, Default)]
    pub struct Wire(Arc<Mutex<Vec<u8>>>);

    impl Write for Wire {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Wire {
        /// Every message written so far.
        pub fn messages(&self) -> Vec<Value> {
            let bytes = self.0.lock().unwrap().clone();
            let mut reader = BufReader::new(bytes.as_slice());
            let mut found = Vec::new();
            while let Ok(Some(body)) = read_message(&mut reader) {
                found.push(serde_json::from_slice(&body).unwrap());
            }
            found
        }

        pub fn wait_for(&self, count: usize) -> Vec<Value> {
            let started = Instant::now();
            loop {
                let messages = self.messages();
                if messages.len() >= count || started.elapsed() > Duration::from_secs(5) {
                    return messages;
                }
                thread::sleep(Duration::from_millis(5));
            }
        }
    }

    pub fn frame(message: Value) -> Vec<u8> {
        let mut bytes = Vec::new();
        write_message(&mut bytes, &serde_json::to_vec(&message).unwrap()).unwrap();
        bytes
    }

    pub fn connect(
        on_notification: impl Fn(&str, Value) + Send + 'static,
    ) -> (Arc<Connection>, Wire, Sender<Vec<u8>>) {
        let (tx, rx) = mpsc::channel();
        let wire = Wire::default();
        let reader = ChannelReader {
            rx,
            current: Cursor::new(Vec::new()),
        };
        let connection = Arc::new(Connection::new(reader, wire.clone(), on_notification));
        (connection, wire, tx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::testing::*;

    #[test]
    fn a_response_reaches_the_request_it_answers() {
        let (connection, wire, server) = connect(|_, _| {});
        let asker = connection.clone();
        let handle = thread::spawn(move || {
            asker.request(
                "textDocument/references",
                json!({"x": 1}),
                Duration::from_secs(5),
            )
        });
        let sent = wire.wait_for(1);
        assert_eq!(sent[0]["method"], "textDocument/references");
        assert_eq!(sent[0]["params"], json!({"x": 1}));
        server
            .send(frame(
                json!({"jsonrpc": "2.0", "id": sent[0]["id"], "result": [1, 2]}),
            ))
            .unwrap();
        assert_eq!(handle.join().unwrap().unwrap(), json!([1, 2]));
    }

    #[test]
    fn concurrent_requests_get_their_own_answers_in_any_order() {
        let (connection, wire, server) = connect(|_, _| {});
        let asks: Vec<_> = ["a", "b"]
            .into_iter()
            .map(|name| {
                let connection = connection.clone();
                thread::spawn(move || connection.request(name, Value::Null, Duration::from_secs(5)))
            })
            .collect();
        let sent = wire.wait_for(2);
        // answer in reverse, echoing the method
        for message in sent.iter().rev() {
            server
                .send(frame(
                    json!({"jsonrpc": "2.0", "id": message["id"], "result": message["method"]}),
                ))
                .unwrap();
        }
        let answers: Vec<_> = asks
            .into_iter()
            .map(|h| h.join().unwrap().unwrap())
            .collect();
        assert_eq!(answers, [json!("a"), json!("b")]);
    }

    #[test]
    fn server_errors_are_reported_with_their_message() {
        let (connection, wire, server) = connect(|_, _| {});
        let asker = connection.clone();
        let handle = thread::spawn(move || asker.request("m", Value::Null, Duration::from_secs(5)));
        let sent = wire.wait_for(1);
        server
            .send(frame(json!({"jsonrpc": "2.0", "id": sent[0]["id"], "error": {"code": -32801, "message": "content modified"}})))
            .unwrap();
        match handle.join().unwrap() {
            Err(Error::Server { code, message }) => {
                assert_eq!((code, message.as_str()), (-32801, "content modified"))
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_silent_server_times_out_and_a_late_answer_is_ignored() {
        let (connection, wire, server) = connect(|_, _| {});
        let result = connection.request("m", Value::Null, Duration::from_millis(30));
        assert!(matches!(result, Err(Error::Timeout)));
        let sent = wire.wait_for(1);
        server
            .send(frame(
                json!({"jsonrpc": "2.0", "id": sent[0]["id"], "result": 1}),
            ))
            .unwrap();
        // the connection still works afterwards
        let asker = connection.clone();
        let handle = thread::spawn(move || asker.request("n", Value::Null, Duration::from_secs(5)));
        let sent = wire.wait_for(2);
        server
            .send(frame(
                json!({"jsonrpc": "2.0", "id": sent[1]["id"], "result": "ok"}),
            ))
            .unwrap();
        assert_eq!(handle.join().unwrap().unwrap(), json!("ok"));
    }

    #[test]
    fn a_closed_stream_fails_waiting_requests() {
        let (connection, wire, server) = connect(|_, _| {});
        let asker = connection.clone();
        let handle = thread::spawn(move || asker.request("m", Value::Null, Duration::from_secs(5)));
        wire.wait_for(1);
        drop(server);
        assert!(matches!(handle.join().unwrap(), Err(Error::Closed)));
    }

    #[test]
    fn notifications_go_to_the_callback() {
        let (seen_tx, seen_rx) = mpsc::channel();
        let (_connection, _wire, server) = connect(move |method, params| {
            seen_tx.send((method.to_string(), params)).unwrap();
        });
        server
            .send(frame(
                json!({"jsonrpc": "2.0", "method": "$/progress", "params": {"v": 1}}),
            ))
            .unwrap();
        let (method, params) = seen_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!((method.as_str(), params), ("$/progress", json!({"v": 1})));
    }

    #[test]
    fn server_requests_are_answered_with_nothing() {
        let (_connection, wire, server) = connect(|_, _| {});
        server
            .send(frame(json!({"jsonrpc": "2.0", "id": 7, "method": "window/workDoneProgress/create", "params": {}})))
            .unwrap();
        server
            .send(frame(json!({"jsonrpc": "2.0", "id": "c", "method": "workspace/configuration", "params": {"items": [{}, {}]}})))
            .unwrap();
        let replies = wire.wait_for(2);
        assert_eq!(
            replies[0],
            json!({"jsonrpc": "2.0", "id": 7, "result": null})
        );
        assert_eq!(
            replies[1],
            json!({"jsonrpc": "2.0", "id": "c", "result": [null, null]})
        );
    }

    #[test]
    fn notifications_we_send_have_no_id() {
        let (connection, wire, _server) = connect(|_, _| {});
        connection.notify("initialized", json!({})).unwrap();
        let sent = wire.wait_for(1);
        assert_eq!(
            sent[0],
            json!({"jsonrpc": "2.0", "method": "initialized", "params": {}})
        );
    }
}
