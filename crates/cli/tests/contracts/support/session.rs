use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader, Read as _, Write as _},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use super::Fixture;

const DEADLINE: Duration = Duration::from_secs(20);

type WriteRequest = (Value, mpsc::Sender<Result<(), String>>);

pub(in crate::contracts) struct Session {
    child: Child,
    writer: Option<mpsc::Sender<WriteRequest>>,
    messages: mpsc::Receiver<Result<Value, String>>,
    threads: Vec<JoinHandle<()>>,
    errors: Arc<Mutex<String>>,
    retained: Vec<Value>,
    replies: HashMap<String, Value>,
    held: HashSet<String>,
    identifier: u64,
    shutdown: bool,
}

impl Session {
    pub(in crate::contracts) fn new(fixture: &Fixture) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_instar"))
            .args(["--timeout", "120", "server"])
            .current_dir(fixture.root())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("launch real Instar server");

        let input = child.stdin.take().expect("server stdin");
        let output = child.stdout.take().expect("server stdout");
        let mut error = child.stderr.take().expect("server stderr");
        let (writer, outgoing) = mpsc::channel::<WriteRequest>();
        let (sender, messages) = mpsc::channel();
        let errors = Arc::new(Mutex::new(String::new()));
        let captured = Arc::clone(&errors);

        let threads = vec![
            thread::spawn(move || {
                let mut input = input;
                for (value, completion) in outgoing {
                    let bytes = serde_json::to_vec(&value).expect("serialize client frame");
                    let result = write!(input, "Content-Length: {}\r\n\r\n", bytes.len())
                        .and_then(|()| input.write_all(&bytes))
                        .and_then(|()| input.flush())
                        .map_err(|error| error.to_string());
                    let failed = result.is_err();
                    if completion.send(result).is_err() || failed {
                        break;
                    }
                }
            }),
            thread::spawn(move || {
                let mut reader = BufReader::new(output);
                loop {
                    let frame = frame(&mut reader);
                    let failed = frame.is_err();
                    if sender.send(frame).is_err() || failed {
                        break;
                    }
                }
            }),
            thread::spawn(move || {
                let mut bytes = [0; 4096];
                while let Ok(length) = error.read(&mut bytes) {
                    if length == 0 {
                        break;
                    }
                    let mut captured = captured.lock().expect("server stderr buffer");
                    if captured.len() < 65_536 {
                        captured.push_str(&String::from_utf8_lossy(&bytes[..length]));
                    }
                }
            }),
        ];

        Self {
            child,
            writer: Some(writer),
            messages,
            threads,
            errors,
            retained: Vec::new(),
            replies: HashMap::new(),
            held: HashSet::new(),
            identifier: 0,
            shutdown: false,
        }
    }

    pub(in crate::contracts) fn request(&mut self, method: &str, parameters: &Value) -> Value {
        let response = self.response(method, parameters);
        assert!(response.get("error").is_none(), "{method}: {response}");

        response.get("result").expect("request result").clone()
    }

    pub(in crate::contracts) fn response(&mut self, method: &str, parameters: &Value) -> Value {
        let identifier = self.start(method, parameters);
        let response = self.receive(identifier);

        if method == "shutdown" && response.get("error").is_none() {
            self.shutdown = true;
        }

        response
    }

    pub(in crate::contracts) fn start(&mut self, method: &str, parameters: &Value) -> u64 {
        self.identifier += 1;
        let identifier = self.identifier;
        self.send(json!({"jsonrpc":"2.0", "id":identifier, "method":method, "params":parameters}));

        identifier
    }

    pub(in crate::contracts) fn receive(&mut self, identifier: u64) -> Value {
        let deadline = Instant::now() + DEADLINE;

        loop {
            if let Some(index) = self
                .retained
                .iter()
                .position(|message| message.get("method").is_none() && message["id"] == identifier)
            {
                return self.retained.remove(index);
            }

            self.pump(deadline);
        }
    }

    pub(in crate::contracts) fn notify(&self, method: &str, parameters: &Value) {
        self.send(json!({"jsonrpc":"2.0", "method":method, "params":parameters}));
    }

    pub(in crate::contracts) fn message(&mut self, method: &str) -> Value {
        let deadline = Instant::now() + DEADLINE;

        loop {
            if let Some(index) = self
                .retained
                .iter()
                .position(|message| message["method"] == method)
            {
                return self.retained.remove(index);
            }

            self.pump(deadline);
        }
    }

    pub(in crate::contracts) fn reply(&mut self, method: &str, result: Value) {
        self.replies.insert(method.to_owned(), result);
    }

    pub(in crate::contracts) fn hold(&mut self, method: &str) {
        self.retained.retain(|message| message["method"] != method);
        self.held.insert(method.to_owned());
    }

    pub(in crate::contracts) fn respond(&self, identifier: &Value, result: &Value) {
        self.send(json!({"jsonrpc":"2.0", "id":identifier, "result":result}));
    }

    pub(in crate::contracts) fn finish(mut self) -> i32 {
        if !self.shutdown {
            assert_eq!(self.request("shutdown", &Value::Null), Value::Null);
        }

        self.notify("exit", &Value::Null);
        let deadline = Instant::now() + DEADLINE;

        loop {
            if let Some(status) = self.child.try_wait().expect("server process status") {
                return status.code().expect("server exit code");
            }

            assert!(
                Instant::now() < deadline,
                "server failed to exit: {}",
                self.errors.lock().expect("server stderr")
            );

            thread::sleep(Duration::from_millis(10));
        }
    }

    fn send(&self, value: Value) {
        let (sender, completion) = mpsc::channel();

        self.writer
            .as_ref()
            .expect("live server stdin")
            .send((value, sender))
            .expect("server writer thread");

        completion
            .recv_timeout(DEADLINE)
            .expect("bounded server write")
            .expect("write server frame");
    }

    fn pump(&mut self, deadline: Instant) {
        let duration = deadline.saturating_duration_since(Instant::now());

        let message = self
            .messages
            .recv_timeout(duration)
            .unwrap_or_else(|error| {
                panic!(
                    "server response deadline or disconnect: {error}; stderr: {}",
                    self.errors.lock().expect("server stderr")
                )
            })
            .expect("valid server frame");

        assert_eq!(message["jsonrpc"], "2.0");

        if let (Some(method), Some(identifier)) = (message["method"].as_str(), message.get("id"))
            && !self.held.remove(method)
        {
            let result = self
                .replies
                .get(method)
                .cloned()
                .unwrap_or_else(|| match method {
                    "workspace/configuration" => json!(vec![
                        Value::Null;
                        message["params"]["items"]
                            .as_array()
                            .expect("configuration scopes")
                            .len()
                    ]),

                    "workspace/applyEdit" => {
                        json!({"applied":false,"failureReason":"contract client declined edit"})
                    }

                    "window/showDocument" => json!({"success":true}),

                    "workspace/workspaceFolders"
                    | "window/showMessageRequest"
                    | "client/registerCapability"
                    | "client/unregisterCapability"
                    | "window/workDoneProgress/create"
                    | "workspace/diagnostic/refresh"
                    | "workspace/semanticTokens/refresh"
                    | "workspace/inlayHint/refresh"
                    | "workspace/codeLens/refresh"
                    | "workspace/inlineValue/refresh"
                    | "workspace/foldingRange/refresh"
                    | "workspace/textDocumentContent/refresh" => Value::Null,

                    _ => panic!("unexpected server request: {message}"),
                });

            self.respond(identifier, &result);
        }

        self.retained.push(message);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        {
            let _writer = self.writer.take();
        }

        if self.child.try_wait().ok().flatten().is_none() {
            let _result = self.child.kill();
        }

        let _result = self.child.wait();

        for thread in self.threads.drain(..) {
            let _result = thread.join();
        }
    }
}

fn frame(reader: &mut impl BufRead) -> Result<Value, String> {
    let mut length = None;

    for _index in 0..32 {
        let mut header = String::new();

        if reader
            .read_line(&mut header)
            .map_err(|error| error.to_string())?
            == 0
        {
            return Err("server stdout closed".to_owned());
        }

        if header == "\r\n" {
            let length = length.ok_or_else(|| "missing Content-Length".to_owned())?;

            if length > 16 * 1024 * 1024 {
                return Err("server frame exceeds contract bound".to_owned());
            }

            let mut bytes = vec![0; length];

            reader
                .read_exact(&mut bytes)
                .map_err(|error| error.to_string())?;

            return serde_json::from_slice(&bytes).map_err(|error| error.to_string());
        }

        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("Content-Length")
        {
            if length.is_some() {
                return Err("duplicate Content-Length".to_owned());
            }

            length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|error| error.to_string())?,
            );
        }
    }

    Err("too many server headers".to_owned())
}
