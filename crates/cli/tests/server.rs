//! Stdio language-server lifecycle and shared formatter behavior.

use std::{
    fmt::Write as FormatWrite,
    fs,
    io::{self, BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::mpsc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde_json::{Value, json};

struct Client {
    child: Child,
    input: Option<ChildStdin>,
    messages: mpsc::Receiver<io::Result<Value>>,
    reader: Option<JoinHandle<()>>,
    progress: mpsc::Receiver<()>,
    logging: Option<JoinHandle<()>>,
}

impl Client {
    fn new(directory: &Path) -> io::Result<Self> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_instar"))
            .current_dir(directory)
            .args(["--verbose", "server"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let input = child.stdin.take();
        let output = child.stdout.take().expect("server stdout");
        let (sender, messages) = mpsc::channel();

        let reader = thread::spawn(move || {
            let mut reader = BufReader::new(output);

            loop {
                let message = read(&mut reader);
                let finished = message.is_err();

                if sender.send(message).is_err() || finished {
                    break;
                }
            }
        });

        let errors = child.stderr.take().expect("server stderr");
        let (sender, progress) = mpsc::channel();

        let logging = thread::spawn(move || {
            for line in BufReader::new(errors).lines() {
                let line = line.expect("server log");

                if line.contains("phase=\"configuration\"") {
                    let _ = sender.send(());
                }
            }
        });

        Ok(Self {
            child,
            input,
            messages,
            reader: Some(reader),
            progress,
            logging: Some(logging),
        })
    }

    fn send(&mut self, message: &Value) -> io::Result<()> {
        let body = serde_json::to_vec(message)?;
        let input = self.input.as_mut().expect("open server stdin");
        write!(input, "Content-Length: {}\r\n\r\n", body.len())?;
        input.write_all(&body)?;

        input.flush()
    }

    fn receive(&self, identifier: i64) -> Value {
        let response = self
            .messages
            .recv_timeout(Duration::from_secs(10))
            .expect("responsive server")
            .expect("framed response");

        assert_eq!(response["id"], identifier, "{response}");

        response
    }

    fn wait(&mut self) -> io::Result<ExitStatus> {
        let started = Instant::now();

        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }

            assert!(
                started.elapsed() < Duration::from_secs(5),
                "server waited for stdin EOF or left work running"
            );

            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();

        if let Some(reader) = self.reader.take() {
            reader.join().expect("protocol reader");
        }

        if let Some(logging) = self.logging.take() {
            logging.join().expect("log reader");
        }
    }
}

fn read(reader: &mut impl BufRead) -> io::Result<Value> {
    let mut length = None;

    loop {
        let mut line = String::new();

        if reader.read_line(&mut line)? == 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }

        if line == "\r\n" {
            break;
        }

        if let Some(value) = line.strip_prefix("Content-Length: ") {
            length = Some(value.trim().parse::<usize>().map_err(io::Error::other)?);
        }
    }

    let mut bytes = vec![0; length.ok_or_else(|| io::Error::other("missing Content-Length"))?];
    reader.read_exact(&mut bytes)?;

    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

fn uri(path: &Path) -> String {
    let path = path
        .to_str()
        .expect("UTF-8 fixture path")
        .replace('\\', "/");

    let mut uri = if path.starts_with('/') {
        "file://"
    } else {
        "file:///"
    }
    .to_owned();

    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b':' | b'-' | b'_' | b'.' | b'~') {
            uri.push(char::from(byte));
        } else {
            write!(uri, "%{byte:02X}").expect("URI string");
        }
    }

    uri
}

#[test]
fn stdio_formatting_shares_cli_configuration_and_exits_with_stdin_open() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let source = include_str!("fixtures/server/document.luau");
    let configuration = include_str!("fixtures/server/instar.toml");
    let path = directory.path().join("entry.luau");
    fs::write(&path, source)?;
    fs::write(directory.path().join("instar.toml"), configuration)?;
    fs::write(directory.path().join("sourcemap.json"), "{")?;

    let mut formatter = Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(directory.path())
        .args(["--stdin-filepath", "entry.luau", "format", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;

    formatter
        .stdin
        .take()
        .expect("formatter stdin")
        .write_all(source.as_bytes())?;

    let expected = formatter.wait_with_output()?;
    assert!(expected.status.success());
    let expected = String::from_utf8(expected.stdout).expect("formatted UTF-8");
    let mut client = Client::new(directory.path())?;
    let document = uri(&path);
    let root = uri(directory.path());
    client.send(&json!({"jsonrpc":"2.0","id":0,"method":"textDocument/formatting","params":{"textDocument":{"uri":document},"options":{"tabSize":8,"insertSpaces":false}}}))?;
    assert_eq!(client.receive(0)["error"]["code"], -32002);
    client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"general":{"positionEncodings":["utf-8","utf-16"]},"workspace":{"didChangeWatchedFiles":{"dynamicRegistration":true}}},"workspaceFolders":[{"uri":root,"name":"fixture"}]}}))?;
    let initialized = client.receive(1);

    assert_eq!(
        initialized["result"]["capabilities"]["positionEncoding"],
        "utf-8"
    );

    assert_eq!(
        initialized["result"]["capabilities"]["documentFormattingProvider"],
        true
    );

    assert!(
        initialized["result"]["capabilities"]
            .get("hoverProvider")
            .is_none()
    );

    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;

    let registration = client
        .messages
        .recv_timeout(Duration::from_secs(10))
        .expect("watch registration")
        .expect("framed registration");

    assert_eq!(registration["method"], "client/registerCapability");

    assert_eq!(
        registration["params"]["registrations"][0]["method"],
        "workspace/didChangeWatchedFiles"
    );

    client.send(&json!({"jsonrpc":"2.0","id":registration["id"],"result":null}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/formatting","params":{"textDocument":{"uri":document},"options":{"tabSize":8,"insertSpaces":false}}}))?;
    assert_eq!(client.receive(2)["result"][0]["newText"], expected);
    assert_eq!(fs::read_to_string(&path)?, source);
    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWorkspaceFolders","params":{"event":{"added":[],"removed":[{"uri":root,"name":"fixture"}]}}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":20,"method":"textDocument/formatting","params":{"textDocument":{"uri":document},"options":{"tabSize":8,"insertSpaces":false}}}))?;
    assert_eq!(client.receive(20)["result"][0]["newText"], expected);

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/server/excluded.toml"),
    )?;

    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":uri(&directory.path().join("instar.toml")),"type":2}]}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":3,"method":"textDocument/formatting","params":{"textDocument":{"uri":document},"options":{"tabSize":8,"insertSpaces":false}}}))?;
    assert_eq!(client.receive(3)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","id":4,"method":"shutdown"}))?;
    assert_eq!(client.receive(4)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());
    assert!(client.input.is_some());

    Ok(())
}

#[test]
fn disconnect_and_exit_without_shutdown_release_the_process() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    for disconnect in [true, false] {
        let mut client = Client::new(directory.path())?;

        client.send(
            &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}),
        )?;

        assert!(client.receive(1).get("result").is_some());

        if disconnect {
            client.input.take();
        } else {
            client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
        }

        assert!(!client.wait()?.success());
    }

    Ok(())
}

#[test]
fn cancelled_configuration_does_not_delay_shutdown() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("entry.luau");
    fs::write(&path, include_str!("fixtures/server/document.luau"))?;

    fs::write(
        directory.path().join(".config.luau"),
        include_str!("../../bridge/tests/fixtures/timeout/.config.luau"),
    )?;

    let mut client = Client::new(directory.path())?;

    client.send(
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}),
    )?;

    assert!(client.receive(1).get("result").is_some());
    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/formatting","params":{"textDocument":{"uri":uri(&path)},"options":{"tabSize":4,"insertSpaces":false}}}))?;

    client
        .progress
        .recv_timeout(Duration::from_secs(10))
        .expect("configuration VM started");

    client.send(&json!({"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":2}}))?;
    assert_eq!(client.receive(2)["error"]["code"], -32800);
    client.send(&json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}))?;
    assert_eq!(client.receive(3)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}
