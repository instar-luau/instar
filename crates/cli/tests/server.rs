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

use instar_project::{documentation_cache_directory, roblox_cache_directory};
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
        Self::with_arguments(directory, &[])
    }

    fn with_arguments(directory: &Path, arguments: &[&str]) -> io::Result<Self> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_instar"))
            .current_dir(directory)
            .args(["--verbose", "server"])
            .args(arguments)
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

    fn receive(&mut self, identifier: i64) -> Value {
        let started = Instant::now();

        loop {
            let response = self.message(started);

            if response["method"] == "client/unregisterCapability" {
                assert_eq!(
                    response["params"]["unregisterations"],
                    json!([{"id":"instar-files","method":"workspace/didChangeWatchedFiles"}]),
                    "{response}"
                );

                self.send(&json!({"jsonrpc":"2.0","id":response["id"],"result":null}))
                    .expect("unregister response");

                continue;
            }

            if response.get("method").is_some() {
                assert_eq!(
                    response["method"], "textDocument/publishDiagnostics",
                    "{response}"
                );

                continue;
            }

            assert_eq!(response["id"], identifier, "{response}");

            return response;
        }
    }

    fn diagnostics(&self, document: &str, version: Option<i64>) -> Value {
        let started = Instant::now();

        loop {
            let message = self.message(started);

            assert_eq!(
                message["method"], "textDocument/publishDiagnostics",
                "{message}"
            );

            if message["params"]["uri"] == document
                && message["params"]["version"].as_i64() == version
            {
                return message["params"]["diagnostics"].clone();
            }
        }
    }

    fn error(&self, method: &str, text: &str) {
        let started = Instant::now();

        loop {
            let message = self.message(started);

            if message["method"] == "textDocument/publishDiagnostics" {
                continue;
            }

            assert_eq!(
                message,
                json!({"jsonrpc":"2.0","method":method,"params":{"type":1,"message":text}})
            );

            return;
        }
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

    fn message(&self, started: Instant) -> Value {
        loop {
            let message = self
                .messages
                .recv_timeout(Duration::from_secs(10).saturating_sub(started.elapsed()))
                .expect("responsive server")
                .expect("framed response");

            if !Self::logged(&message) {
                return message;
            }
        }
    }

    fn logged(message: &Value) -> bool {
        if !matches!(
            message["method"].as_str(),
            Some("window/logMessage" | "window/showMessage")
        ) {
            return false;
        }

        assert!(
            message["params"]["type"]
                .as_i64()
                .is_some_and(|kind| (1..=4).contains(&kind)),
            "{message}"
        );

        assert!(message["params"]["message"].is_string(), "{message}");

        message["params"]["type"] != 1
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

fn signature_parameter(signature: &Value, parameter: usize) -> String {
    let label = &signature["parameters"][parameter]["label"];

    if let Some(label) = label.as_str() {
        return label.to_owned();
    }

    let start = usize::try_from(label[0].as_u64().expect("parameter start")).expect("start offset");
    let end = usize::try_from(label[1].as_u64().expect("parameter end")).expect("end offset");

    let label = signature["label"]
        .as_str()
        .expect("signature label")
        .encode_utf16()
        .collect::<Vec<_>>();

    String::from_utf16(&label[start..end]).expect("valid parameter label")
}

fn position(source: &str, offset: usize, encoding: &str) -> Value {
    let prefix = &source[..offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count();
    let prefix = prefix.rsplit('\n').next().expect("source line");

    let character = match encoding {
        "utf-8" => prefix.len(),
        "utf-32" => prefix.chars().count(),
        _ => prefix.encode_utf16().count(),
    };

    json!({"line":line,"character":character})
}

fn decode_tokens(data: &Value, legend: &Value) -> Vec<Value> {
    let data = data.as_array().expect("encoded tokens");
    assert_eq!(data.len() % 5, 0);
    let types = legend["tokenTypes"].as_array().expect("token types");

    let modifiers = legend["tokenModifiers"]
        .as_array()
        .expect("token modifiers");

    let mut line = 0_u64;
    let mut character = 0_u64;

    data.as_chunks::<5>().0.iter().map(|token| {
        let delta = token[0].as_u64().expect("token line");
        line += delta;

        character = if delta == 0 { character } else { 0 } + token[1].as_u64().expect("token column");

        let length = token[2].as_u64().expect("token length");
        assert!(length > 0);
        let kind = usize::try_from(token[3].as_u64().expect("token kind")).expect("kind index");
        let mask = token[4].as_u64().expect("token modifiers");
        assert_eq!(mask >> modifiers.len(), 0);
        let modifiers = modifiers.iter().enumerate().filter(|(index, _)| mask & (1 << index) != 0).map(|(_, modifier)| modifier.clone()).collect::<Vec<_>>();

        json!({"line":line,"character":character,"length":length,"kind":types[kind],"modifiers":modifiers})
    }).collect()
}

fn assert_tokens(source: &str, encoding: &str, data: &Value, legend: &Value, cases: &[Value]) {
    let tokens = decode_tokens(data, legend);

    for case in cases {
        let offset = source
            .find(case["query"].as_str().expect("token query"))
            .expect("token occurrence")
            + usize::try_from(case["offset"].as_u64().expect("token offset")).expect("byte offset");

        let start = position(source, offset, encoding);
        let text = case["text"].as_str().expect("token text");
        let length = position(text, text.len(), encoding);
        let expected = json!({"line":start["line"],"character":start["character"],"length":length["character"],"kind":case["kind"],"modifiers":case["modifiers"]});

        assert!(
            tokens.contains(&expected),
            "{encoding}: missing {expected} in {tokens:?}"
        );
    }

    for pair in tokens.windows(2) {
        if pair[0]["line"] == pair[1]["line"] {
            assert!(
                pair[0]["character"].as_u64().expect("column")
                    + pair[0]["length"].as_u64().expect("length")
                    <= pair[1]["character"].as_u64().expect("column")
            );
        } else {
            assert!(
                pair[0]["line"].as_u64().expect("line") < pair[1]["line"].as_u64().expect("line")
            );
        }
    }
}

fn reference_targets(directory: &Path, case: &Value, declarations: bool) -> io::Result<Vec<Value>> {
    case["targets"].as_array().expect("reference targets").iter()
        .filter(|target| declarations || target["declaration"] == false)
        .map(|target| {
            let path = directory.join(target["file"].as_str().expect("target file"));
            let source = fs::read_to_string(&path)?;

            let offset = source.find(target["query"].as_str().expect("target query")).expect("target occurrence")
                + usize::try_from(target["offset"].as_u64().expect("target offset")).expect("source offset");

            let length = usize::try_from(target.get("length").unwrap_or(&case["length"]).as_u64().expect("target length")).expect("source length");

            Ok(json!({"uri":uri(&path),"range":{"start":position(&source, offset, "utf-16"),"end":position(&source, offset + length, "utf-16")}}))
        }).collect()
}

fn apply_edits(source: &str, edits: &[Value]) -> String {
    let offset = |position: &Value| {
        let line =
            usize::try_from(position["line"].as_u64().expect("edit line")).expect("line index");

        let character = usize::try_from(position["character"].as_u64().expect("edit character"))
            .expect("character index");

        let start = source
            .split_inclusive('\n')
            .take(line)
            .map(str::len)
            .sum::<usize>();

        let mut units = 0;

        for (offset, character_value) in source[start..].char_indices() {
            if units == character {
                return start + offset;
            }

            units += character_value.len_utf16();
        }

        assert_eq!(units, character);

        source.len()
    };

    let mut edits = edits
        .iter()
        .map(|edit| {
            (
                offset(&edit["range"]["start"]),
                offset(&edit["range"]["end"]),
                edit["newText"].as_str().expect("replacement"),
            )
        })
        .collect::<Vec<_>>();

    edits.sort_by_key(|edit| edit.0);
    assert!(edits.windows(2).all(|pair| pair[0].1 <= pair[1].0));
    let mut text = source.to_owned();

    for (start, end, replacement) in edits.into_iter().rev() {
        text.replace_range(start..end, replacement);
    }

    text
}

fn document_edits<'response>(
    response: &'response Value,
    uri: &str,
    version: Option<i32>,
    versioned: bool,
) -> &'response [Value] {
    if versioned {
        let change = response["result"]["documentChanges"]
            .as_array()
            .expect("versioned edits")
            .iter()
            .find(|change| change["textDocument"]["uri"] == uri)
            .expect("changed document");

        assert_eq!(change["textDocument"]["version"], json!(version));

        change["edits"].as_array().expect("document edits")
    } else {
        response["result"]["changes"][uri]
            .as_array()
            .expect("document edits")
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

fn roblox_assets(client: &mut Client) -> io::Result<()> {
    let cache = roblox_cache_directory()?;

    let fixtures =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../project/tests/fixtures/assets/project");

    for entry in fs::read_dir(fixtures)? {
        let entry = entry?;
        let text = fs::read_to_string(entry.path())?;
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri(&cache.join(entry.file_name())),"languageId":"luau","version":1,"text":text}}}))?;
    }

    Ok(())
}

fn contextual_assets(client: &mut Client) -> io::Result<()> {
    roblox_assets(client)?;
    let cache = roblox_cache_directory()?;

    for (name, text) in [
        (
            "metadata.json",
            include_str!("fixtures/diagnostics/metadata.json"),
        ),
        (
            "plugin.d.luau",
            include_str!("fixtures/diagnostics/plugin.luau"),
        ),
    ] {
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri(&cache.join(name)),"version":2},"contentChanges":[{"text":text}]}}))?;
    }

    Ok(())
}

#[test]
fn client_preserves_error_notifications() {
    for method in ["window/logMessage", "window/showMessage"] {
        for severity in 1..=4 {
            let message = json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": {"type": severity, "message": "notification"}
            });

            assert_eq!(Client::logged(&message), severity != 1, "{message}");
        }
    }
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

    assert_eq!(initialized["result"]["capabilities"]["hoverProvider"], true);

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
fn range_formatting_preserves_surrounding_source_and_uses_project_configuration() -> io::Result<()>
{
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/formatting/cases.json"))?;

    for encoding in ["utf-8", "utf-16", "utf-32"] {
        let directory = tempfile::tempdir()?;

        fs::write(
            directory.path().join("instar.toml"),
            include_str!("fixtures/formatting/instar.toml"),
        )?;

        let path = directory.path().join("entry.luau");

        let original = cases[0]["source"]
            .as_str()
            .expect("source")
            .replace('\n', "\r\n");

        fs::write(&path, &original)?;

        let document = if encoding == "utf-32" {
            "untitled:formatting".to_owned()
        } else {
            uri(&path)
        };

        let mut client = Client::new(directory.path())?;
        client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"general":{"positionEncodings":[encoding]}},"workspaceFolders":[{"uri":uri(directory.path()),"name":"workspace"}]}}))?;

        assert_eq!(
            client.receive(1)["result"]["capabilities"]["documentRangeFormattingProvider"],
            true
        );

        client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":original}}}))?;

        for (index, case) in cases.iter().enumerate() {
            let source = case["source"]
                .as_str()
                .expect("source")
                .replace('\n', "\r\n");

            let expected = case["expected"]
                .as_str()
                .expect("expected source")
                .replace('\n', "\r\n");

            let offset = source
                .find(case["needle"].as_str().expect("selection"))
                .expect("selection location");

            let selected = position(&source, offset, encoding);
            client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":index + 2},"contentChanges":[{"text":source}]}}))?;
            client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/rangeFormatting","params":{"textDocument":{"uri":document},"range":{"start":selected,"end":selected},"options":{"tabSize":8,"insertSpaces":false}}}))?;
            let response = client.receive(2);
            assert!(response.get("error").is_none(), "{case}: {response}");

            if source == expected {
                assert_eq!(response["result"], Value::Null, "{case}");
            } else {
                let edits = response["result"].as_array().expect("formatting edits");
                assert_eq!(edits.len(), 1);
                assert_eq!(edits[0]["range"]["start"]["character"], 0);
                assert_eq!(edits[0]["range"]["end"]["character"], 0);
                assert_eq!(apply_edits(&source, edits), expected, "{case}");
                client.send(&json!({"jsonrpc":"2.0","id":3,"method":"textDocument/rangeFormatting","params":{"textDocument":{"uri":document},"range":{"start":selected,"end":selected},"options":{"tabSize":8,"insertSpaces":false}}}))?;
                assert_eq!(client.receive(3)["result"], response["result"]);
            }

            assert_eq!(fs::read_to_string(&path)?, original);
        }

        client.send(&json!({"jsonrpc":"2.0","id":4,"method":"shutdown"}))?;
        assert_eq!(client.receive(4)["result"], Value::Null);
        client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
        assert!(client.wait()?.success());
    }

    Ok(())
}

#[test]
fn on_type_formatting_uses_complete_statements_in_incomplete_documents() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/formatting/instar.toml"),
    )?;

    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/formatting/typing.json"))?;
    let document = uri(&directory.path().join("entry.luau"));
    let mut client = Client::new(directory.path())?;
    client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"workspaceFolders":[{"uri":uri(directory.path()),"name":"workspace"}]}}))?;
    let response = client.receive(1);

    assert_eq!(
        response["result"]["capabilities"]["documentOnTypeFormattingProvider"],
        json!({"firstTriggerCharacter":"\n","moreTriggerCharacter":[";"]})
    );

    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":""}}}))?;

    for (index, case) in cases.iter().enumerate() {
        let source = case["source"]
            .as_str()
            .expect("source")
            .replace('\n', "\r\n");

        let expected = case["expected"]
            .as_str()
            .expect("expected source")
            .replace('\n', "\r\n");

        let character = case["character"].as_str().expect("trigger");
        let cursor = source.rfind(character).expect("typed trigger") + character.len();
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":index + 2},"contentChanges":[{"text":source}]}}))?;
        client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/onTypeFormatting","params":{"textDocument":{"uri":document},"position":position(&source,cursor,"utf-16"),"ch":character,"options":{"tabSize":8,"insertSpaces":false}}}))?;
        let response = client.receive(2);
        assert!(response.get("error").is_none(), "{response}");

        if source == expected {
            assert_eq!(response["result"], Value::Null);
        } else {
            assert_eq!(
                apply_edits(
                    &source,
                    response["result"].as_array().expect("typing edits")
                ),
                expected
            );
        }
    }

    client.send(&json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}))?;
    assert_eq!(client.receive(3)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

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
        directory.path().join("instar.luau"),
        include_str!("../../luau/tests/fixtures/timeout/instar.luau"),
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

#[test]
fn diagnostics_follow_versions_clear_on_close_and_recover_from_configuration_errors()
-> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/diagnostics/instar.toml"),
    )?;

    let path = directory.path().join("entry.luau");
    let invalid = include_str!("fixtures/diagnostics/invalid.luau");
    let valid = include_str!("fixtures/diagnostics/valid.luau");
    fs::write(&path, valid)?;
    let document = uri(&path);
    let mut client = Client::new(directory.path())?;

    client.send(
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}),
    )?;

    assert!(client.receive(1).get("result").is_some());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":invalid}}}))?;
    let diagnostics = client.diagnostics(&document, Some(1));

    assert!(
        diagnostics
            .as_array()
            .expect("diagnostics")
            .iter()
            .any(|diagnostic| diagnostic["source"] == "Luau"
                && diagnostic["severity"] == 1
                && diagnostic["range"]["start"]["line"] == 1),
        "{diagnostics}"
    );

    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":2},"contentChanges":[{"text":invalid}]}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":3},"contentChanges":[{"text":valid}]}}))?;
    assert_eq!(client.diagnostics(&document, Some(3)), json!([]));
    fs::write(directory.path().join("instar.toml"), "[")?;
    let configuration = uri(&directory.path().join("instar.toml"));
    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":configuration,"type":2}]}}))?;

    client.error(
        "window/logMessage",
        &format!(
            "{}: TOML parse error at line 1, column 2\n  |\n1 | [\n  |  ^\nunquoted keys cannot be empty, expected letters, numbers, `-`, `_`\n",
            directory.path().join("instar.toml").display()
        ),
    );

    assert_ne!(
        client
            .diagnostics(&configuration, None)
            .as_array()
            .expect("failure diagnostics")
            .as_slice(),
        Vec::<Value>::new().as_slice()
    );

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/diagnostics/instar.toml"),
    )?;

    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":configuration,"type":2}]}}))?;
    assert_eq!(client.diagnostics(&document, Some(3)), json!([]));
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":document}}}))?;
    assert_eq!(client.diagnostics(&document, None), json!([]));
    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"shutdown"}))?;
    assert_eq!(client.receive(2)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn dependency_failures_follow_disk_changes_without_standalone_dependency_findings() -> io::Result<()>
{
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/diagnostics/instar.toml"),
    )?;

    let path = directory.path().join("entry.luau");
    let dependency = directory.path().join("dependency.luau");
    let caller = include_str!("fixtures/diagnostics/caller.luau");
    fs::write(&path, caller)?;

    fs::write(&dependency, "local =")?;

    let document = uri(&path);
    let imported = uri(&dependency);
    let mut client = Client::new(directory.path())?;

    client.send(
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}),
    )?;

    assert!(client.receive(1).get("result").is_some());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":caller}}}))?;

    assert_ne!(
        client
            .diagnostics(&document, Some(1))
            .as_array()
            .expect("attributed dependency diagnostics")
            .as_slice(),
        Vec::<Value>::new().as_slice()
    );

    fs::write(&dependency, include_str!("fixtures/diagnostics/valid.luau"))?;
    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":imported,"type":2}]}}))?;
    assert_eq!(client.diagnostics(&document, Some(1)), json!([]));
    fs::remove_file(&dependency)?;
    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":imported,"type":3}]}}))?;

    assert_ne!(
        client
            .diagnostics(&document, Some(1))
            .as_array()
            .expect("resolution diagnostics")
            .as_slice(),
        Vec::<Value>::new().as_slice()
    );

    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"shutdown"}))?;
    assert_eq!(client.receive(2)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn diagnostics_preserve_informational_severity_and_rule_codes() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/information/instar.toml"),
    )?;

    let source = include_str!("fixtures/information/entry.luau");
    let document = uri(&directory.path().join("entry.luau"));
    let mut client = Client::new(directory.path())?;

    client.send(
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}),
    )?;

    assert!(client.receive(1).get("result").is_some());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
    let findings = client.diagnostics(&document, Some(1));

    assert!(
        findings
            .as_array()
            .expect("diagnostics")
            .iter()
            .any(|diagnostic| diagnostic["severity"] == 3
                && diagnostic["source"] == "Instar"
                && diagnostic["code"] == "unused_variable"
                && diagnostic["tags"] == json!([1])),
        "{findings}"
    );

    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"shutdown"}))?;
    assert_eq!(client.receive(2)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn virtual_documents_keep_protocol_uris_and_enforce_read_only_formatting() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/diagnostics/instar.toml"),
    )?;

    let source = include_str!("fixtures/diagnostics/invalid.luau");
    let mut client = Client::new(directory.path())?;

    client.send(
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}),
    )?;

    assert!(client.receive(1).get("result").is_some());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;

    for document in ["untitled:example", "instar:definition"] {
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
        let findings = client.diagnostics(document, Some(1));

        assert!(
            findings
                .as_array()
                .expect("virtual diagnostics")
                .iter()
                .any(|finding| finding["source"] == "Luau"),
            "{findings}"
        );

        client.send(&json!({"jsonrpc":"2.0","id":2,"method":"workspace/textDocumentContent","params":{"uri":document}}))?;
        assert_eq!(client.receive(2)["result"]["text"], source);

        if document.starts_with("instar:") {
            client.send(&json!({"jsonrpc":"2.0","id":3,"method":"textDocument/formatting","params":{"textDocument":{"uri":document},"options":{"tabSize":4,"insertSpaces":false}}}))?;
            assert_eq!(client.receive(3)["error"]["code"], -32602);
        }

        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":document}}}))?;
        assert_eq!(client.diagnostics(document, None), json!([]));
    }

    client.send(&json!({"jsonrpc":"2.0","id":4,"method":"shutdown"}))?;
    assert_eq!(client.receive(4)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn negotiated_progress_ends_when_running_analysis_is_cancelled_by_shutdown() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.luau"),
        include_str!("../../luau/tests/fixtures/timeout/instar.luau"),
    )?;

    let document = uri(&directory.path().join("entry.luau"));
    let source = include_str!("fixtures/diagnostics/valid.luau");
    let mut client = Client::new(directory.path())?;
    client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"window":{"workDoneProgress":true}}}}))?;
    assert!(client.receive(1).get("result").is_some());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;

    client
        .progress
        .recv_timeout(Duration::from_secs(10))
        .expect("configuration VM started");

    let create = client
        .messages
        .recv_timeout(Duration::from_secs(10))
        .expect("progress creation")
        .expect("progress request");

    assert_eq!(create["method"], "window/workDoneProgress/create");
    let token = create["params"]["token"].clone();
    client.send(&json!({"jsonrpc":"2.0","id":create["id"],"result":null}))?;

    let begin = client
        .messages
        .recv_timeout(Duration::from_secs(10))
        .expect("progress begin")
        .expect("progress notification");

    assert_eq!(begin["method"], "$/progress");
    assert_eq!(begin["params"]["token"], token);
    assert_eq!(begin["params"]["value"]["kind"], "begin");
    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"shutdown"}))?;
    let mut stopped = false;
    let mut ended = false;
    let started = Instant::now();

    while !stopped || !ended {
        let message = client
            .messages
            .recv_timeout(Duration::from_secs(10).saturating_sub(started.elapsed()))
            .expect("shutdown progress")
            .expect("protocol message");

        if message["id"] == 2 {
            assert_eq!(message["result"], Value::Null);
            stopped = true;
        } else {
            assert_eq!(message["method"], "$/progress");
            assert_eq!(message["params"]["token"], token);
            assert_eq!(message["params"]["value"]["kind"], "end");
            ended = true;
        }
    }

    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn workspace_diagnostics_discover_unopened_files_and_follow_deletion() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/workspace/instar.toml"),
    )?;

    let source = directory.path().join("entry.luau");
    let document = uri(&source);
    let invalid = include_str!("fixtures/diagnostics/invalid.luau");
    fs::write(&source, invalid)?;
    fs::write(directory.path().join("excluded.luau"), invalid)?;
    let mut client = Client::new(directory.path())?;
    client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"workspaceFolders":[{"uri":uri(directory.path()),"name":"workspace"}]}}))?;
    assert!(client.receive(1).get("result").is_some());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;

    let notification = client.message(Instant::now());

    assert_eq!(notification["method"], "textDocument/publishDiagnostics");
    assert_eq!(notification["params"]["uri"], document);
    assert!(notification["params"]["version"].is_null());

    assert!(
        notification["params"]["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .any(|finding| finding["source"] == "Luau")
    );

    fs::remove_file(source)?;
    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didDeleteFiles","params":{"files":[{"uri":document}]}}))?;

    let cleared = client.message(Instant::now());

    assert_eq!(cleared["params"]["uri"], document);
    assert_eq!(cleared["params"]["diagnostics"], json!([]));
    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"shutdown"}))?;

    let response = client.message(Instant::now());

    assert_eq!(response["id"], 2);
    assert_eq!(response["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn hover_uses_negotiated_positions_and_snapshot_types() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/diagnostics/instar.toml"),
    )?;

    let source = include_str!("fixtures/hover/input.luau");
    let document = uri(&directory.path().join("entry.luau"));
    let offset = source.rfind("count").expect("reference");

    for encoding in ["utf-8", "utf-16", "utf-32"] {
        let character = match encoding {
            "utf-8" => offset,
            "utf-16" => source[..offset].encode_utf16().count(),
            _ => source[..offset].chars().count(),
        };

        let mut client = Client::new(directory.path())?;
        client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"general":{"positionEncodings":[encoding]},"textDocument":{"hover":{"contentFormat":["markdown"]}}}}}))?;
        let initialized = client.receive(1);
        assert_eq!(initialized["result"]["capabilities"]["hoverProvider"], true);

        assert_eq!(
            initialized["result"]["capabilities"]["positionEncoding"],
            encoding
        );

        client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
        client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":document},"position":{"line":0,"character":character}}}))?;
        let hovered = client.receive(2);

        assert_eq!(
            hovered["result"]["contents"],
            json!({"kind":"markdown","value":"```luau\nnumber\n```"})
        );

        assert_eq!(
            hovered["result"]["range"],
            json!({"start":{"line":0,"character":character},"end":{"line":0,"character":character + 5}})
        );

        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":2},"contentChanges":[{"text":include_str!("fixtures/hover/updated.luau")}]}}))?;
        client.send(&json!({"jsonrpc":"2.0","id":3,"method":"textDocument/hover","params":{"textDocument":{"uri":document},"position":{"line":1,"character":7}}}))?;

        assert_eq!(
            client.receive(3)["result"]["contents"]["value"],
            "```luau\nstring\n```"
        );

        client.send(&json!({"jsonrpc":"2.0","id":4,"method":"textDocument/hover","params":{"textDocument":{"uri":document},"position":{"line":100,"character":0}}}))?;
        assert_eq!(client.receive(4)["error"]["code"], -32602);
        client.send(&json!({"jsonrpc":"2.0","id":5,"method":"shutdown"}))?;
        assert_eq!(client.receive(5)["result"], Value::Null);
        client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
        assert!(client.wait()?.success());
    }

    Ok(())
}

#[test]
fn hover_resolves_custom_documentation_and_refreshes_changed_indexes() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    for (name, text) in [
        ("instar.toml", include_str!("fixtures/hover/instar.toml")),
        (
            "definitions.luau",
            include_str!("fixtures/hover/definitions.luau"),
        ),
        (
            "documentation.json",
            include_str!("fixtures/hover/documentation.json"),
        ),
    ] {
        fs::write(directory.path().join(name), text)?;
    }

    let document = uri(&directory.path().join("entry.luau"));
    let source = include_str!("fixtures/hover/documented.luau");
    let mut client = Client::new(directory.path())?;

    client.send(
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}),
    )?;

    assert_eq!(
        client.receive(1)["result"]["capabilities"]["hoverProvider"],
        true
    );

    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":document},"position":{"line":0,"character":8}}}))?;
    let hovered = client.receive(2);
    assert_eq!(hovered["result"]["contents"]["kind"], "plaintext");

    let content = hovered["result"]["contents"]["value"]
        .as_str()
        .expect("documented hover");

    assert!(content.contains("number"), "{content}");
    assert!(content.contains("string"), "{content}");
    assert!(content.contains("Deprecated."), "{content}");

    assert!(
        content.contains("Explains the supplied **number**."),
        "{content}"
    );

    assert!(content.contains("explain(42)"), "{content}");

    assert!(
        content.contains("https://example.invalid/explain"),
        "{content}"
    );

    let documentation = directory.path().join("documentation.json");

    fs::write(
        &documentation,
        include_str!("fixtures/hover/replacement.json"),
    )?;

    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":uri(&documentation),"type":2}]}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":3,"method":"textDocument/hover","params":{"textDocument":{"uri":document},"position":{"line":0,"character":8}}}))?;
    let refreshed = client.receive(3);

    let content = refreshed["result"]["contents"]["value"]
        .as_str()
        .expect("refreshed documentation");

    assert!(content.contains("Updated explanation."), "{content}");
    assert!(!content.contains("Explains the supplied"), "{content}");
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":2},"contentChanges":[{"text":include_str!("fixtures/hover/overloads.luau")}]}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":4,"method":"textDocument/hover","params":{"textDocument":{"uri":document},"position":{"line":0,"character":8}}}))?;
    let overloaded = client.receive(4);

    let content = overloaded["result"]["contents"]["value"]
        .as_str()
        .expect("overloaded signature");

    assert!(content.contains("(number) -> string"), "{content}");
    assert!(content.contains("(string) -> number"), "{content}");
    client.send(&json!({"jsonrpc":"2.0","id":5,"method":"shutdown"}))?;
    assert_eq!(client.receive(5)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn hover_resolves_builtin_documentation_from_snapshot_assets() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/diagnostics/instar.toml"),
    )?;

    let cache = documentation_cache_directory()?;
    let mut client = Client::new(directory.path())?;

    client.send(
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}),
    )?;

    assert!(client.receive(1).get("result").is_some());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;

    for (name, text) in [
        (
            "metadata.json",
            include_str!("../../project/tests/fixtures/assets/documentation/metadata.json"),
        ),
        (
            "documentation.json",
            include_str!("../../project/tests/fixtures/assets/documentation/documentation.json"),
        ),
    ] {
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri(&cache.join(name)),"languageId":"json","version":1,"text":text}}}))?;
    }

    let document = uri(&directory.path().join("entry.luau"));
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":include_str!("../../project/tests/fixtures/environment/builtin/entry.luau")}}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":document},"position":{"line":0,"character":1}}}))?;
    let response = client.receive(2);

    let content = response["result"]["contents"]["value"]
        .as_str()
        .expect("builtin documentation");

    assert!(content.contains("Prints values."), "{content}");
    client.send(&json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}))?;
    assert_eq!(client.receive(3)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn hover_resolves_roblox_documentation_from_snapshot_assets() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("../../project/tests/fixtures/checking/roblox/instar.toml"),
    )?;

    let cache = roblox_cache_directory()?;
    let mut client = Client::new(directory.path())?;

    client.send(
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}),
    )?;

    assert!(client.receive(1).get("result").is_some());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;

    for (name, text) in [
        (
            "metadata.json",
            include_str!("fixtures/hover/roblox/metadata.json"),
        ),
        (
            "documentation.json",
            include_str!("fixtures/hover/roblox/documentation.json"),
        ),
        (
            "none.d.luau",
            include_str!("../../project/tests/fixtures/assets/project/none.d.luau"),
        ),
        (
            "local.d.luau",
            include_str!("../../project/tests/fixtures/assets/project/local.d.luau"),
        ),
        (
            "plugin.d.luau",
            include_str!("../../project/tests/fixtures/assets/project/plugin.d.luau"),
        ),
        (
            "roblox.d.luau",
            include_str!("../../project/tests/fixtures/assets/project/roblox.d.luau"),
        ),
        (
            "enumerations.d.luau",
            include_str!("../../project/tests/fixtures/assets/project/enumerations.d.luau"),
        ),
    ] {
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri(&cache.join(name)),"languageId":"luau","version":1,"text":text}}}))?;
    }

    let document = uri(&directory.path().join("entry.luau"));
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":include_str!("fixtures/hover/roblox/input.luau")}}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":document},"position":{"line":0,"character":13}}}))?;
    let response = client.receive(2);

    let content = response["result"]["contents"]["value"]
        .as_str()
        .expect("Roblox documentation");

    assert!(content.contains("string"), "{content}");
    assert!(content.contains("The instance name."), "{content}");
    client.send(&json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}))?;
    assert_eq!(client.receive(3)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn navigation_resolves_imports_exported_types_and_members() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/diagnostics/instar.toml"),
    )?;

    let dependency = directory.path().join("dependency.luau");

    fs::write(
        &dependency,
        include_str!("fixtures/navigation/dependency.luau"),
    )?;

    let document = uri(&directory.path().join("entry.luau"));

    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/navigation/expected.json"))?;

    let mut client = Client::new(directory.path())?;

    client.send(
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}),
    )?;

    let initialized = client.receive(1);

    assert_eq!(
        initialized["result"]["capabilities"]["definitionProvider"],
        true
    );

    assert_eq!(
        initialized["result"]["capabilities"]["typeDefinitionProvider"],
        true
    );

    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":include_str!("fixtures/navigation/input.luau")}}}))?;

    for case in cases {
        client.send(&json!({"jsonrpc":"2.0","id":2,"method":case["method"],"params":{"textDocument":{"uri":document},"position":case["position"]}}))?;
        let response = client.receive(2);

        assert_eq!(
            response["result"],
            json!([{"uri":uri(&dependency),"range":case["range"]}]),
            "{case}: {response}"
        );
    }

    client.send(&json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}))?;
    assert_eq!(client.receive(3)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn navigation_publishes_immutable_read_only_declarations() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let definitions = include_str!("fixtures/hover/definitions.luau");

    for (name, text) in [
        ("instar.toml", include_str!("fixtures/hover/instar.toml")),
        ("definitions.luau", definitions),
        (
            "documentation.json",
            include_str!("fixtures/hover/documentation.json"),
        ),
    ] {
        fs::write(directory.path().join(name), text)?;
    }

    let document = uri(&directory.path().join("entry.luau"));
    let mut client = Client::new(directory.path())?;

    client.send(
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}),
    )?;

    assert!(client.receive(1).get("result").is_some());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":include_str!("fixtures/hover/documented.luau")}}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/definition","params":{"textDocument":{"uri":document},"position":{"line":0,"character":8}}}))?;
    let response = client.receive(2);
    let targets = response["result"].as_array().expect("declaration targets");
    assert_eq!(targets.len(), 1);
    let target = targets[0]["uri"].as_str().expect("declaration URI");
    assert!(target.starts_with("instar://definition/"), "{response}");

    assert_eq!(
        targets[0]["range"],
        json!({"start":{"line":1,"character":17},"end":{"line":1,"character":24}})
    );

    client.send(&json!({"jsonrpc":"2.0","id":3,"method":"workspace/textDocumentContent","params":{"uri":target}}))?;
    assert_eq!(client.receive(3)["result"]["text"], definitions);
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":target,"languageId":"luau","version":1,"text":definitions}}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":4,"method":"textDocument/formatting","params":{"textDocument":{"uri":target},"options":{"tabSize":4,"insertSpaces":true}}}))?;
    assert_eq!(client.receive(4)["error"]["code"], -32602);
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":target,"version":2},"contentChanges":[{"text":"return 0"}]}}))?;
    client.error("window/logMessage", "Invalid params: document is read-only");

    client.error(
        "window/showMessage",
        "Invalid params: document is read-only",
    );

    client.send(&json!({"jsonrpc":"2.0","id":5,"method":"workspace/textDocumentContent","params":{"uri":target}}))?;
    assert_eq!(client.receive(5)["result"]["text"], definitions);

    assert_eq!(
        fs::read_to_string(directory.path().join("definitions.luau"))?,
        definitions
    );

    client.send(&json!({"jsonrpc":"2.0","id":6,"method":"shutdown"}))?;
    assert_eq!(client.receive(6)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn symbols_negotiate_hierarchy_and_survive_incomplete_code_without_environments() -> io::Result<()>
{
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/hover/instar.toml"),
    )?;

    let document = uri(&directory.path().join("entry.luau"));

    for hierarchical in [true, false] {
        let mut client = Client::new(directory.path())?;
        client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"textDocument":{"documentSymbol":{"hierarchicalDocumentSymbolSupport":hierarchical}}}}}))?;

        assert_eq!(
            client.receive(1)["result"]["capabilities"]["documentSymbolProvider"],
            true
        );

        client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":include_str!("fixtures/symbols/input.luau")}}}))?;
        client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/documentSymbol","params":{"textDocument":{"uri":document}}}))?;

        let mut expected: Value = serde_json::from_str(if hierarchical {
            include_str!("fixtures/symbols/expected.json")
        } else {
            include_str!("fixtures/symbols/flat.json")
        })?;

        if !hierarchical {
            for symbol in expected.as_array_mut().expect("flat symbol fixture") {
                symbol["location"]["uri"] = json!(document);
            }
        }

        let response = client.receive(2);
        assert_eq!(response["result"], expected, "{response}");
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":2},"contentChanges":[{"text":include_str!("fixtures/symbols/incomplete.luau")}]}}))?;
        client.send(&json!({"jsonrpc":"2.0","id":3,"method":"textDocument/documentSymbol","params":{"textDocument":{"uri":document}}}))?;
        let response = client.receive(3);

        let symbols = response["result"]
            .as_array()
            .expect("incomplete document symbols");

        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0]["name"], "pending");
        assert_eq!(symbols[0]["kind"], 12);
        client.send(&json!({"jsonrpc":"2.0","id":4,"method":"shutdown"}))?;
        assert_eq!(client.receive(4)["result"], Value::Null);
        client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
        assert!(client.wait()?.success());
    }

    Ok(())
}

#[test]
fn completion_negotiate_positions_and_member_notation() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/diagnostics/instar.toml"),
    )?;

    let source = include_str!("fixtures/completion/member.luau");
    let document = uri(&directory.path().join("entry.luau"));

    for encoding in ["utf-8", "utf-16", "utf-32"] {
        let character = match encoding {
            "utf-8" => source.len(),
            "utf-16" => source.encode_utf16().count(),
            _ => source.chars().count(),
        };

        let mut client = Client::new(directory.path())?;
        client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"general":{"positionEncodings":[encoding]},"textDocument":{"completion":{"completionItem":{"commitCharactersSupport":true},"completionItemKind":{"valueSet":[1]}}}}}}))?;
        let initialized = client.receive(1);

        assert_eq!(
            initialized["result"]["capabilities"]["completionProvider"]["resolveProvider"],
            true
        );

        client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
        client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":document},"position":{"line":0,"character":character}}}))?;
        let response = client.receive(2);

        let items = response["result"]["items"]
            .as_array()
            .expect("completion items");

        let value = items
            .iter()
            .find(|item| item["label"] == "value")
            .expect("value member");

        assert_eq!(
            value["textEdit"],
            json!({"range":{"start":{"line":0,"character":character},"end":{"line":0,"character":character}},"newText":"value"})
        );

        assert_eq!(value["detail"], "number");
        assert_eq!(value["kind"], 1);
        assert_eq!(value["commitCharacters"], json!([".", ":"]));

        let indexed = items
            .iter()
            .find(|item| item["label"] == "odd key")
            .expect("indexed member");

        assert_eq!(
            indexed["textEdit"],
            json!({"range":{"start":{"line":0,"character":character - 1},"end":{"line":0,"character":character}},"newText":"[\"odd key\"]"})
        );

        assert_eq!(indexed["insertTextFormat"], Value::Null);
        client.send(&json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}))?;
        assert_eq!(client.receive(3)["result"], Value::Null);
        client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
        assert!(client.wait()?.success());
    }

    Ok(())
}

#[test]
fn completion_negotiates_function_snippets() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/diagnostics/instar.toml"),
    )?;

    let source = include_str!("fixtures/completion/function.luau");
    let document = uri(&directory.path().join("entry.luau"));

    for snippets in [false, true] {
        let mut client = Client::new(directory.path())?;
        client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"textDocument":{"completion":{"completionItem":{"snippetSupport":snippets}}}}}}))?;
        assert!(client.receive(1)["error"].is_null());
        client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
        client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":document},"position":{"line":1,"character":source.lines().last().expect("query line").len()}}}))?;
        let response = client.receive(2);

        let candidate = response["result"]["items"]
            .as_array()
            .expect("completion items")
            .iter()
            .find(|item| item["label"] == "identify")
            .expect("function completion");

        assert_eq!(
            candidate["textEdit"]["newText"],
            if snippets { "identify($0)" } else { "identify" }
        );

        assert_eq!(
            candidate["insertTextFormat"],
            if snippets { json!(2) } else { Value::Null }
        );

        client.send(&json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}))?;
        assert_eq!(client.receive(3)["result"], Value::Null);
        client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
        assert!(client.wait()?.success());
    }

    Ok(())
}

#[test]
fn completion_resolves_documentation_and_rejects_old_edits() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    for (name, text) in [
        ("instar.toml", include_str!("fixtures/hover/instar.toml")),
        (
            "definitions.luau",
            include_str!("fixtures/hover/definitions.luau"),
        ),
        (
            "documentation.json",
            include_str!("fixtures/hover/documentation.json"),
        ),
    ] {
        fs::write(directory.path().join(name), text)?;
    }

    let source = include_str!("fixtures/hover/documented.luau");
    let document = uri(&directory.path().join("entry.luau"));
    let mut client = Client::new(directory.path())?;
    client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"textDocument":{"completion":{"completionItem":{"documentationFormat":["markdown"],"tagSupport":{"valueSet":[1]}}}}}}}))?;
    assert!(client.receive(1)["error"].is_null());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":document},"position":{"line":0,"character":8}}}))?;
    let response = client.receive(2);

    let candidate = response["result"]["items"]
        .as_array()
        .expect("completion items")
        .iter()
        .find(|item| item["label"] == "explain")
        .expect("documented completion");

    assert_eq!(candidate["tags"], json!([1]));
    assert_eq!(candidate["textEdit"]["newText"], "explain");

    client.send(
        &json!({"jsonrpc":"2.0","id":3,"method":"completionItem/resolve","params":candidate}),
    )?;

    let resolved = client.receive(3);
    assert_eq!(resolved["result"]["documentation"]["kind"], "markdown");

    assert!(
        resolved["result"]["documentation"]["value"]
            .as_str()
            .expect("documentation")
            .contains("Explains the supplied")
    );

    assert_eq!(resolved["result"]["textEdit"], candidate["textEdit"]);
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":2},"contentChanges":[{"text":source}]}}))?;

    client.send(
        &json!({"jsonrpc":"2.0","id":4,"method":"completionItem/resolve","params":candidate}),
    )?;

    assert_eq!(client.receive(4)["error"]["code"], -32801);
    client.send(&json!({"jsonrpc":"2.0","id":5,"method":"shutdown"}))?;
    assert_eq!(client.receive(5)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn completion_uses_host_import_resolution_and_aliases() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/diagnostics/instar.toml"),
    )?;

    fs::write(
        directory.path().join(".luaurc"),
        include_str!("fixtures/completion/.luaurc"),
    )?;

    fs::create_dir(directory.path().join("modules"))?;

    fs::write(
        directory.path().join("dependency.luau"),
        include_str!("fixtures/completion/dependency.luau"),
    )?;

    let document = uri(&directory.path().join("entry.luau"));
    let source = include_str!("../../luau/tests/fixtures/suggestions/broken.luau");
    let mut client = Client::new(directory.path())?;

    client.send(
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}),
    )?;

    assert!(client.receive(1)["error"].is_null());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":document},"position":{"line":0,"character":source.len()}}}))?;
    let response = client.receive(2);
    assert_eq!(response["result"]["isIncomplete"], true);

    let candidate = response["result"]["items"]
        .as_array()
        .expect("path items")
        .iter()
        .find(|item| item["label"] == "./dependency")
        .expect("relative import");

    assert_eq!(candidate["textEdit"]["newText"], "\"./dependency\"");
    let source = include_str!("fixtures/completion/alias.luau");
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":2},"contentChanges":[{"text":source}]}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":3,"method":"textDocument/completion","params":{"textDocument":{"uri":document},"position":{"line":0,"character":source.len()}}}))?;
    let response = client.receive(3);
    assert!(response["error"].is_null(), "{response}");

    let candidate = response["result"]["items"]
        .as_array()
        .expect("alias items")
        .iter()
        .find(|item| item["label"] == "@package/")
        .expect("configured alias");

    assert_eq!(candidate["textEdit"]["newText"], "\"@package/\"");
    client.send(&json!({"jsonrpc":"2.0","id":4,"method":"shutdown"}))?;
    assert_eq!(client.receive(4)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn completion_preserves_mapped_import_contexts() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    for (name, text) in [
        (
            "instar.toml",
            include_str!("../../project/tests/fixtures/checking/roblox/instar.toml"),
        ),
        (
            "sourcemap.json",
            include_str!("../../project/tests/fixtures/checking/explicit/sourcemap.json"),
        ),
        (
            "number.luau",
            include_str!("fixtures/completion/dependency.luau"),
        ),
        (
            "string.luau",
            include_str!("../../project/tests/fixtures/checking/explicit/string.luau"),
        ),
    ] {
        fs::write(directory.path().join(name), text)?;
    }

    let document = uri(&directory.path().join("entry.luau"));
    let mut client = Client::new(directory.path())?;

    client.send(
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}),
    )?;

    assert!(client.receive(1)["error"].is_null());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    roblox_assets(&mut client)?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":0,"text":""}}}))?;

    for (index, (source, label)) in [
        (include_str!("fixtures/completion/root.luau"), "@game/"),
        (
            include_str!("fixtures/completion/folder.luau"),
            "@game/One/",
        ),
        (
            include_str!("fixtures/completion/mapped.luau"),
            "@game/One/Target",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let identifier = i64::try_from(index).expect("request identifier") + 2;
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":index + 1},"contentChanges":[{"text":source}]}}))?;
        client.send(&json!({"jsonrpc":"2.0","id":identifier,"method":"textDocument/completion","params":{"textDocument":{"uri":document},"position":{"line":0,"character":source.len()}}}))?;
        let response = client.receive(identifier);
        assert!(response["error"].is_null(), "{response}");

        let candidate = response["result"]["items"]
            .as_array()
            .expect("mapped suggestions")
            .iter()
            .find(|item| item["label"] == label)
            .expect("mapped import");

        assert_eq!(candidate["textEdit"]["newText"], format!("\"{label}\""));
    }

    let document = uri(&directory.path().join("caller.luau"));
    let source = include_str!("fixtures/completion/mapped.luau");
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":5,"method":"textDocument/completion","params":{"textDocument":{"uri":document},"position":{"line":0,"character":source.len()}}}))?;
    assert_eq!(client.receive(5)["error"]["code"], -32602);
    client.send(&json!({"jsonrpc":"2.0","id":6,"method":"shutdown"}))?;
    assert_eq!(client.receive(6)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn completion_uses_snapshot_roblox_constructor_metadata() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("../../project/tests/fixtures/checking/roblox/instar.toml"),
    )?;

    let source = include_str!("fixtures/completion/constructor.luau");
    let document = uri(&directory.path().join("entry.luau"));
    let mut client = Client::new(directory.path())?;

    client.send(
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}),
    )?;

    assert!(client.receive(1)["error"].is_null());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    roblox_assets(&mut client)?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":document},"position":{"line":0,"character":source.len()}}}))?;
    let response = client.receive(2);
    assert!(response["error"].is_null(), "{response}");

    let items = response["result"]["items"]
        .as_array()
        .expect("constructor suggestions");

    let candidate = items
        .iter()
        .find(|item| item["label"] == "Part")
        .expect("creatable class");

    assert_eq!(candidate["textEdit"]["newText"], "\"Part\"");
    assert!(!items.iter().any(|item| item["label"] == "DataModel"));
    client.send(&json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}))?;
    assert_eq!(client.receive(3)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn completion_resolves_versioned_automatic_imports() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    let configuration = format!(
        "{}\n[editing]\nimport_binding = \"local\"\n",
        include_str!("fixtures/diagnostics/instar.toml")
    );

    for (name, text) in [
        ("instar.toml", configuration.as_str()),
        (
            "dependency.luau",
            include_str!("fixtures/completion/dependency.luau"),
        ),
    ] {
        fs::write(directory.path().join(name), text)?;
    }

    let source = include_str!("fixtures/completion/automatic.luau");
    let imported = include_str!("fixtures/completion/imported.luau");
    let document = uri(&directory.path().join("entry.luau"));

    for resolve in [false, true] {
        let capabilities = if resolve {
            json!({"resolveSupport":{"properties":["additionalTextEdits"]}})
        } else {
            json!({})
        };

        let mut client = Client::new(directory.path())?;
        client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"textDocument":{"completion":{"completionItem":capabilities}}}}}))?;
        assert!(client.receive(1)["error"].is_null());
        client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
        client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":document},"position":{"line":1,"character":17}}}))?;
        let response = client.receive(2);
        assert!(response["error"].is_null(), "{response}");

        let candidate = response["result"]["items"]
            .as_array()
            .expect("completion items")
            .iter()
            .find(|item| item["label"] == "dependency");

        assert_eq!(candidate.is_some(), resolve, "{response}");

        if let Some(candidate) = candidate {
            assert_eq!(response["result"]["isIncomplete"], true);
            assert!(candidate["additionalTextEdits"].is_null());

            assert_eq!(
                candidate["textEdit"],
                json!({"range":{"start":{"line":1,"character":14},"end":{"line":1,"character":17}},"newText":"dependency"})
            );

            client.send(&json!({"jsonrpc":"2.0","id":3,"method":"completionItem/resolve","params":candidate}))?;
            let resolved = client.receive(3);
            assert!(resolved["error"].is_null(), "{resolved}");
            assert_eq!(resolved["result"]["textEdit"], candidate["textEdit"]);

            let edits = resolved["result"]["additionalTextEdits"]
                .as_array()
                .expect("supporting edits");

            assert_eq!(edits.len(), 1);

            assert_eq!(
                edits[0]["range"],
                json!({"start":{"line":1,"character":0},"end":{"line":1,"character":0}})
            );

            let mut applied = source.to_owned();
            let replacement = applied.rfind("dep").expect("completion prefix");
            applied.replace_range(replacement.., "dependency");
            let insertion = applied.find('\n').expect("directive ending") + 1;

            applied.insert_str(
                insertion,
                edits[0]["newText"].as_str().expect("import binding"),
            );

            assert_eq!(applied, imported);
            client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":2},"contentChanges":[{"text":applied}]}}))?;
            client.send(&json!({"jsonrpc":"2.0","id":4,"method":"completionItem/resolve","params":candidate}))?;
            assert_eq!(client.receive(4)["error"]["code"], -32801);
            client.send(&json!({"jsonrpc":"2.0","id":5,"method":"textDocument/hover","params":{"textDocument":{"uri":document},"position":{"line":2,"character":15}}}))?;
            let hovered = client.receive(5);

            assert_eq!(
                hovered["result"]["contents"]["value"], "number",
                "{hovered}"
            );
        }

        client.send(&json!({"jsonrpc":"2.0","id":6,"method":"shutdown"}))?;
        assert_eq!(client.receive(6)["result"], Value::Null);
        client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
        assert!(client.wait()?.success());
    }

    Ok(())
}

#[test]
fn completion_rejects_unsafe_automatic_imports() -> io::Result<()> {
    for (source, dependency) in [
        (
            include_str!("fixtures/completion/shadow.luau"),
            include_str!("fixtures/completion/dependency.luau"),
        ),
        (
            include_str!("fixtures/completion/occupied.luau"),
            include_str!("fixtures/completion/dependency.luau"),
        ),
        (
            include_str!("fixtures/completion/overlap.luau"),
            include_str!("fixtures/completion/dependency.luau"),
        ),
        (
            include_str!("fixtures/completion/automatic.luau"),
            include_str!("fixtures/completion/cycle.luau"),
        ),
    ] {
        let directory = tempfile::tempdir()?;

        fs::write(
            directory.path().join("instar.toml"),
            include_str!("fixtures/diagnostics/instar.toml"),
        )?;

        fs::write(directory.path().join("dependency.luau"), dependency)?;
        let document = uri(&directory.path().join("entry.luau"));
        let mut client = Client::new(directory.path())?;
        client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"textDocument":{"completion":{"completionItem":{"resolveSupport":{"properties":["additionalTextEdits"]}}}}}}}))?;
        assert!(client.receive(1)["error"].is_null());
        client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
        client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":document},"position":{"line":source.lines().count()-1,"character":source.lines().last().expect("query line").len()}}}))?;
        let response = client.receive(2);
        assert!(response["error"].is_null(), "{response}");

        assert!(
            !response["result"]["items"]
                .as_array()
                .expect("completion items")
                .iter()
                .any(|item| item["detail"]
                    .as_str()
                    .is_some_and(|detail| detail.starts_with("Auto import from "))),
            "{source}\n{response}"
        );

        client.send(&json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}))?;
        assert_eq!(client.receive(3)["result"], Value::Null);
        client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
        assert!(client.wait()?.success());
    }

    Ok(())
}

#[test]
fn signatures_preserve_native_overloads_arguments_and_negotiated_labels() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/diagnostics/instar.toml"),
    )?;

    let source = include_str!("fixtures/signatures/input.luau");

    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/signatures/expected.json"))?;

    let document = uri(&directory.path().join("entry.luau"));

    for (encoding, offsets) in [("utf-8", false), ("utf-16", true), ("utf-32", true)] {
        let mut client = Client::new(directory.path())?;
        client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"general":{"positionEncodings":[encoding]},"textDocument":{"signatureHelp":{"signatureInformation":{"activeParameterSupport":true,"parameterInformation":{"labelOffsetSupport":offsets}}}}}}}))?;
        let response = client.receive(1);

        assert_eq!(
            response["result"]["capabilities"]["signatureHelpProvider"]["triggerCharacters"],
            json!(["(", ","])
        );

        client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;

        for (index, case) in cases.iter().enumerate() {
            let query = case["query"].as_str().expect("query text");

            let parameter = usize::try_from(case["parameter"].as_u64().expect("parameter index"))
                .expect("parameter offset");

            let alternatives =
                usize::try_from(case["alternatives"].as_u64().expect("alternative count"))
                    .expect("signature count");

            let offset = source.find(query).expect("query prefix") + query.len();
            let prefix = &source[..offset];
            let line = prefix.bytes().filter(|byte| *byte == b'\n').count();
            let prefix = prefix.rsplit('\n').next().expect("query line");

            let character = match encoding {
                "utf-8" => prefix.len(),
                "utf-32" => prefix.chars().count(),
                _ => prefix.encode_utf16().count(),
            };

            let identifier = i64::try_from(index).expect("request identifier") + 2;
            client.send(&json!({"jsonrpc":"2.0","id":identifier,"method":"textDocument/signatureHelp","params":{"textDocument":{"uri":document},"position":{"line":line,"character":character}}}))?;
            let response = client.receive(identifier);
            assert!(response["error"].is_null(), "{response}");
            let result = &response["result"];
            let signatures = result["signatures"].as_array().expect("native signatures");
            assert!(signatures.len() >= alternatives, "{response}");

            let selected = usize::try_from(
                result["activeSignature"]
                    .as_u64()
                    .expect("native selected signature"),
            )
            .expect("signature index");

            assert_eq!(
                result["activeParameter"], case["parameter"],
                "{query}: {response}"
            );

            assert_eq!(signatures[selected]["activeParameter"], case["parameter"]);

            assert_eq!(
                signature_parameter(&signatures[selected], parameter),
                case["label"].as_str().expect("parameter label")
            );

            assert_eq!(
                signatures[selected]["parameters"][parameter]["label"].is_array(),
                offsets
            );
        }

        client.send(&json!({"jsonrpc":"2.0","id":100,"method":"textDocument/signatureHelp","params":{"textDocument":{"uri":document},"position":{"line":0,"character":0}}}))?;
        assert_eq!(client.receive(100)["result"], Value::Null);
        client.send(&json!({"jsonrpc":"2.0","id":101,"method":"shutdown"}))?;
        assert_eq!(client.receive(101)["result"], Value::Null);
        client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
        assert!(client.wait()?.success());
    }

    Ok(())
}

#[test]
fn signatures_recover_incomplete_calls() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/diagnostics/instar.toml"),
    )?;

    let source = include_str!("fixtures/signatures/incomplete.luau");
    let document = uri(&directory.path().join("entry.luau"));
    let mut client = Client::new(directory.path())?;

    client.send(
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}),
    )?;

    assert!(client.receive(1)["error"].is_null());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/signatureHelp","params":{"textDocument":{"uri":document},"position":{"line":3,"character":source.lines().last().expect("query line").len()}}}))?;
    let response = client.receive(2);
    assert_eq!(response["result"]["activeParameter"], 1, "{response}");
    let signature = &response["result"]["signatures"][0];
    assert_eq!(signature_parameter(signature, 1), "count: number");
    assert!(signature.get("activeParameter").is_none());
    client.send(&json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}))?;
    assert_eq!(client.receive(3)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn signatures_render_documentation_and_negotiate_inactive_parameters() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    for (name, text) in [
        ("instar.toml", include_str!("fixtures/hover/instar.toml")),
        (
            "definitions.luau",
            include_str!("fixtures/hover/definitions.luau"),
        ),
        (
            "documentation.json",
            include_str!("fixtures/hover/documentation.json"),
        ),
    ] {
        fs::write(directory.path().join(name), text)?;
    }

    let document = uri(&directory.path().join("entry.luau"));

    for format in ["plaintext", "markdown"] {
        let source = include_str!("fixtures/hover/documented.luau");
        let mut client = Client::new(directory.path())?;
        client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"textDocument":{"signatureHelp":{"signatureInformation":{"documentationFormat":[format],"activeParameterSupport":true,"noActiveParameterSupport":true}}}}}}))?;
        assert!(client.receive(1)["error"].is_null());
        client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
        client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/signatureHelp","params":{"textDocument":{"uri":document},"position":{"line":0,"character":source.find('(').expect("call")+1}}}))?;
        let response = client.receive(2);
        assert!(response["error"].is_null(), "{response}");
        let signature = &response["result"]["signatures"][0];
        assert_eq!(signature["documentation"]["kind"], format);

        assert!(
            signature["documentation"]["value"]
                .as_str()
                .expect("documentation")
                .contains("Explains the supplied")
        );

        assert_eq!(signature["activeParameter"], 0);
        let source = include_str!("fixtures/signatures/excess.luau");
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":2},"contentChanges":[{"text":source}]}}))?;
        client.send(&json!({"jsonrpc":"2.0","id":3,"method":"textDocument/signatureHelp","params":{"textDocument":{"uri":document},"position":{"line":0,"character":source.len()}}}))?;
        let response = client.receive(3);
        assert!(response["error"].is_null(), "{response}");

        assert_eq!(
            response["result"].get("activeParameter"),
            Some(&Value::Null)
        );

        assert_eq!(
            response["result"]["signatures"][0].get("activeParameter"),
            Some(&Value::Null)
        );

        client.send(&json!({"jsonrpc":"2.0","id":4,"method":"shutdown"}))?;
        assert_eq!(client.receive(4)["result"], Value::Null);
        client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
        assert!(client.wait()?.success());
    }

    Ok(())
}

#[test]
fn workspace_symbols_share_index_selection_and_live_source_snapshots() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/symbols/instar.toml"),
    )?;

    let source = include_str!("fixtures/symbols/input.luau");

    for name in ["first", "second"] {
        fs::create_dir(directory.path().join(name))?;
        fs::write(directory.path().join(name).join("visible.luau"), source)?;
        fs::write(directory.path().join(name).join("excluded.luau"), source)?;
    }

    let first = uri(&directory.path().join("first"));
    let second = uri(&directory.path().join("second"));
    let first_document = uri(&directory.path().join("first/visible.luau"));
    let second_document = uri(&directory.path().join("second/visible.luau"));
    let mut client = Client::new(directory.path())?;
    client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"workspaceFolders":[{"uri":first,"name":"first"},{"uri":second,"name":"second"}],"capabilities":{"workspace":{"symbol":{"symbolKind":{"valueSet":[13]}}}}}}))?;

    assert_eq!(
        client.receive(1)["result"]["capabilities"]["workspaceSymbolProvider"],
        true
    );

    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;

    client.send(
        &json!({"jsonrpc":"2.0","id":2,"method":"workspace/symbol","params":{"query":"INN"}}),
    )?;

    let response = client.receive(2);
    assert!(response["error"].is_null(), "{response}");
    let symbols = response["result"].as_array().expect("workspace symbols");
    assert_eq!(symbols.len(), 2, "{response}");

    for symbol in symbols {
        assert_eq!(symbol["name"], "inner");
        assert_eq!(symbol["kind"], 13);
        assert_eq!(symbol["containerName"], "outer");

        assert_eq!(
            symbol["location"]["range"],
            json!({"start":{"line":6,"character":19},"end":{"line":6,"character":24}})
        );
    }

    assert_eq!(symbols[0]["location"]["uri"], first_document);
    assert_eq!(symbols[1]["location"]["uri"], second_document);
    let source = include_str!("fixtures/symbols/incomplete.luau");
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":first_document,"languageId":"luau","version":1,"text":source}}}))?;

    client.send(
        &json!({"jsonrpc":"2.0","id":3,"method":"workspace/symbol","params":{"query":"INN"}}),
    )?;

    let response = client.receive(3);

    assert_eq!(
        response["result"]
            .as_array()
            .expect("updated symbols")
            .len(),
        1,
        "{response}"
    );

    assert_eq!(response["result"][0]["location"]["uri"], second_document);
    fs::remove_file(directory.path().join("second/visible.luau"))?;
    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didDeleteFiles","params":{"files":[{"uri":second_document}]}}))?;

    client.send(
        &json!({"jsonrpc":"2.0","id":4,"method":"workspace/symbol","params":{"query":"INN"}}),
    )?;

    assert_eq!(client.receive(4)["result"], json!([]));
    let document = "untitled:workspace.luau";
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;

    client.send(
        &json!({"jsonrpc":"2.0","id":5,"method":"workspace/symbol","params":{"query":"pending"}}),
    )?;

    let response = client.receive(5);
    let symbols = response["result"].as_array().expect("overlay symbols");
    assert_eq!(symbols.len(), 2, "{response}");

    assert!(
        symbols
            .iter()
            .any(|symbol| symbol["location"]["uri"] == document)
    );

    assert!(
        symbols
            .iter()
            .any(|symbol| symbol["location"]["uri"] == first_document)
    );

    client.send(&json!({"jsonrpc":"2.0","id":6,"method":"shutdown"}))?;
    assert_eq!(client.receive(6)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn references_preserve_cross_file_identity_shadowing_and_declarations() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    for (name, source) in [
        (
            "instar.toml",
            include_str!("fixtures/diagnostics/instar.toml"),
        ),
        ("input.luau", include_str!("fixtures/references/input.luau")),
        (
            "dependency.luau",
            include_str!("fixtures/references/dependency.luau"),
        ),
        (
            "consumer.luau",
            include_str!("fixtures/references/consumer.luau"),
        ),
    ] {
        fs::write(directory.path().join(name), source)?;
    }

    let source = include_str!("fixtures/references/input.luau");

    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/references/expected.json"))?;

    let document = uri(&directory.path().join("input.luau"));
    let root = uri(directory.path());
    let mut client = Client::new(directory.path())?;
    client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"workspaceFolders":[{"uri":root,"name":"references"}]}}))?;
    let response = client.receive(1);

    assert_eq!(
        response["result"]["capabilities"]["referencesProvider"],
        true
    );

    assert_eq!(
        response["result"]["capabilities"]["documentHighlightProvider"],
        true
    );

    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;

    for case in cases {
        let offset = source
            .find(case["query"].as_str().expect("reference query"))
            .expect("query occurrence")
            + usize::try_from(case["offset"].as_u64().expect("query offset"))
                .expect("source offset");

        for declarations in [true, false] {
            client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/references","params":{"textDocument":{"uri":document},"position":position(source,offset,"utf-16"),"context":{"includeDeclaration":declarations}}}))?;
            let response = client.receive(2);
            assert!(response["error"].is_null(), "{response}");

            assert_eq!(
                response["result"],
                json!(reference_targets(directory.path(), &case, declarations)?),
                "{case}: {response}"
            );
        }

        client.send(&json!({"jsonrpc":"2.0","id":3,"method":"textDocument/documentHighlight","params":{"textDocument":{"uri":document},"position":position(source,offset,"utf-16")}}))?;
        let response = client.receive(3);

        let expected = reference_targets(directory.path(), &case, true)?.into_iter()
            .zip(case["targets"].as_array().expect("targets"))
            .filter(|(target, _)| target["uri"] == document)
            .map(|(target, metadata)| json!({"range":target["range"],"kind":if metadata["write"] == true {3} else {2}}))
            .collect::<Vec<_>>();

        assert_eq!(response["result"], json!(expected), "{case}: {response}");
    }

    let consumer = uri(&directory.path().join("consumer.luau"));
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":consumer,"languageId":"luau","version":1,"text":include_str!("fixtures/server/document.luau")}}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":4,"method":"textDocument/references","params":{"textDocument":{"uri":document},"position":{"line":1,"character":25},"context":{"includeDeclaration":false}}}))?;
    let response = client.receive(4);
    let references = response["result"].as_array().expect("updated references");
    assert_eq!(references.len(), 3, "{response}");

    assert!(
        references
            .iter()
            .all(|reference| reference["uri"] != consumer)
    );

    client.send(&json!({"jsonrpc":"2.0","id":5,"method":"shutdown"}))?;
    assert_eq!(client.receive(5)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn references_and_highlights_negotiate_unicode_and_preserve_virtual_uris() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/diagnostics/instar.toml"),
    )?;

    let source = include_str!("fixtures/references/unicode.luau");
    let document = "untitled:references.luau";

    for encoding in ["utf-8", "utf-16", "utf-32"] {
        let mut client = Client::new(directory.path())?;
        client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"general":{"positionEncodings":[encoding]}}}}))?;
        assert!(client.receive(1)["error"].is_null());
        client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
        let offset = source.find("value +=").expect("query");

        let expected = [source.find("value = 1").expect("declaration"), offset, source.rfind("value").expect("read")]
            .into_iter().map(|offset| json!({"start":position(source,offset,encoding),"end":position(source,offset+5,encoding)})).collect::<Vec<_>>();

        client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/references","params":{"textDocument":{"uri":document},"position":position(source,offset,encoding),"context":{"includeDeclaration":true}}}))?;
        let response = client.receive(2);

        assert_eq!(
            response["result"],
            json!(
                expected
                    .iter()
                    .map(|range| json!({"uri":document,"range":range}))
                    .collect::<Vec<_>>()
            ),
            "{response}"
        );

        client.send(&json!({"jsonrpc":"2.0","id":3,"method":"textDocument/documentHighlight","params":{"textDocument":{"uri":document},"position":position(source,offset,encoding)}}))?;
        let response = client.receive(3);

        assert_eq!(
            response["result"],
            json!(
                expected
                    .iter()
                    .zip([3, 3, 2])
                    .map(|(range, kind)| json!({"range":range,"kind":kind}))
                    .collect::<Vec<_>>()
            ),
            "{response}"
        );

        client.send(&json!({"jsonrpc":"2.0","id":4,"method":"shutdown"}))?;
        assert_eq!(client.receive(4)["result"], Value::Null);
        client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
        assert!(client.wait()?.success());
    }

    Ok(())
}

#[test]
fn rename_validates_cross_file_edits_and_negotiates_document_versions() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    let files = [
        (
            "input.luau",
            include_str!("fixtures/references/input.luau"),
            include_str!("fixtures/rename/input.luau"),
            Some(7),
        ),
        (
            "dependency.luau",
            include_str!("fixtures/references/dependency.luau"),
            include_str!("fixtures/rename/dependency.luau"),
            None,
        ),
        (
            "consumer.luau",
            include_str!("fixtures/references/consumer.luau"),
            include_str!("fixtures/rename/consumer.luau"),
            Some(12),
        ),
    ];

    fs::write(
        directory.path().join("instar.toml"),
        format!(
            "{}\n[selection.index]\noverride_exclusions = true\n",
            include_str!("fixtures/selection/graph.toml")
        ),
    )?;

    for (name, source, _, _) in files {
        fs::write(directory.path().join(name), source)?;
    }

    let root = uri(directory.path());
    let document = uri(&directory.path().join("input.luau"));

    for versioned in [true, false] {
        let mut client = Client::new(directory.path())?;
        client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"workspace":{"workspaceEdit":{"documentChanges":versioned}},"textDocument":{"rename":{"prepareSupport":true}}},"workspaceFolders":[{"uri":root,"name":"rename"}]}}))?;

        assert_eq!(
            client.receive(1)["result"]["capabilities"]["renameProvider"]["prepareProvider"],
            true
        );

        client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;

        for (name, source, _, version) in files {
            if let Some(version) = version {
                client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri(&directory.path().join(name)),"languageId":"luau","version":version,"text":source}}}))?;
            }
        }

        client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/prepareRename","params":{"textDocument":{"uri":document},"position":{"line":1,"character":25}}}))?;

        assert_eq!(
            client.receive(2)["result"],
            json!({"start":{"line":1,"character":25},"end":{"line":1,"character":30}})
        );

        client.send(&json!({"jsonrpc":"2.0","id":3,"method":"textDocument/rename","params":{"textDocument":{"uri":document},"position":{"line":1,"character":25},"newName":"amount"}}))?;
        let response = client.receive(3);
        assert!(response["error"].is_null(), "{response}");

        if versioned {
            assert_eq!(
                response["result"]["documentChanges"]
                    .as_array()
                    .expect("versioned edits")
                    .len(),
                3,
                "{response}"
            );

            assert!(response["result"].get("changes").is_none());
        } else {
            assert_eq!(
                response["result"]["changes"]
                    .as_object()
                    .expect("workspace edits")
                    .len(),
                3,
                "{response}"
            );

            assert!(response["result"].get("documentChanges").is_none());
        }

        for (name, source, expected, version) in files {
            let target = uri(&directory.path().join(name));
            let edits = document_edits(&response, &target, version, versioned);
            let changed = apply_edits(source, edits);
            assert_eq!(changed, expected, "{name}");
            assert_eq!(fs::read_to_string(directory.path().join(name))?, source);

            if let Some(version) = version {
                client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":target,"version":version+1},"contentChanges":[{"text":changed}]}}))?;
            } else {
                client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":target,"languageId":"luau","version":1,"text":changed}}}))?;
            }
        }

        client.send(&json!({"jsonrpc":"2.0","id":4,"method":"textDocument/references","params":{"textDocument":{"uri":document},"position":{"line":1,"character":25},"context":{"includeDeclaration":true}}}))?;
        let response = client.receive(4);

        assert_eq!(
            response["result"]
                .as_array()
                .expect("renamed references")
                .len(),
            5,
            "{response}"
        );

        client.send(&json!({"jsonrpc":"2.0","id":5,"method":"shutdown"}))?;
        assert_eq!(client.receive(5)["result"], Value::Null);
        client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
        assert!(client.wait()?.success());
    }

    Ok(())
}

#[test]
fn rename_rejects_capture_keywords_and_read_only_symbols_without_rejecting_disjoint_scopes()
-> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/diagnostics/instar.toml"),
    )?;

    let document = "untitled:rename.luau";
    let mut client = Client::new(directory.path())?;

    client.send(
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"textDocument":{"rename":{"prepareSupport":true}}}}}),
    )?;

    assert!(client.receive(1)["error"].is_null());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    let source = include_str!("fixtures/rename/conflict.luau");
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;

    for name in ["occupied", "end", "invalid name"] {
        client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/rename","params":{"textDocument":{"uri":document},"position":{"line":0,"character":6},"newName":name}}))?;
        let response = client.receive(2);

        assert_eq!(
            response["error"]["code"],
            if name == "occupied" { -32803 } else { -32602 },
            "{name}: {response}"
        );
    }

    let source = include_str!("fixtures/rename/disjoint.luau");
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":2},"contentChanges":[{"text":source}]}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":3,"method":"textDocument/rename","params":{"textDocument":{"uri":document},"position":{"line":0,"character":6},"newName":"available"}}))?;
    let response = client.receive(3);
    assert!(response["error"].is_null(), "{response}");

    let edits = response["result"]["changes"][document]
        .as_array()
        .expect("local edits");

    assert_eq!(edits.len(), 2);
    assert!(edits.iter().all(|edit| edit["newText"] == "available"));
    client.send(&json!({"jsonrpc":"2.0","id":4,"method":"textDocument/prepareRename","params":{"textDocument":{"uri":document},"position":{"line":3,"character":4}}}))?;
    let response = client.receive(4);
    assert!(response["error"].is_null(), "{response}");
    assert_eq!(response["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","id":5,"method":"textDocument/rename","params":{"textDocument":{"uri":document},"position":{"line":3,"character":4},"newName":"renamed"}}))?;
    assert_eq!(client.receive(5)["error"]["code"], -32803);
    client.send(&json!({"jsonrpc":"2.0","id":6,"method":"shutdown"}))?;
    assert_eq!(client.receive(6)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn rename_preserves_unique_mapped_identity_and_rejects_multiple_placements() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    for (name, text) in [
        (
            "instar.toml",
            include_str!("../../project/tests/fixtures/checking/roblox/instar.toml"),
        ),
        (
            "sourcemap.json",
            include_str!("../../project/tests/fixtures/checking/explicit/sourcemap.json"),
        ),
        ("number.luau", include_str!("fixtures/rename/disjoint.luau")),
        ("caller.luau", include_str!("fixtures/rename/disjoint.luau")),
        (
            "string.luau",
            include_str!("../../project/tests/fixtures/checking/explicit/string.luau"),
        ),
    ] {
        fs::write(directory.path().join(name), text)?;
    }

    let mut client = Client::new(directory.path())?;

    client.send(
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"textDocument":{"rename":{"prepareSupport":true}}}}}),
    )?;

    assert!(client.receive(1)["error"].is_null());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    roblox_assets(&mut client)?;
    let document = uri(&directory.path().join("number.luau"));
    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/rename","params":{"textDocument":{"uri":document},"position":{"line":0,"character":6},"newName":"renamed"}}))?;
    let response = client.receive(2);
    assert!(response["error"].is_null(), "{response}");

    assert_eq!(
        response["result"]["changes"][&document]
            .as_array()
            .expect("mapped edits")
            .len(),
        2
    );

    let document = uri(&directory.path().join("caller.luau"));
    client.send(&json!({"jsonrpc":"2.0","id":3,"method":"textDocument/prepareRename","params":{"textDocument":{"uri":document},"position":{"line":0,"character":6}}}))?;
    assert_eq!(client.receive(3)["error"]["code"], -32803);
    client.send(&json!({"jsonrpc":"2.0","id":4,"method":"textDocument/rename","params":{"textDocument":{"uri":document},"position":{"line":0,"character":6},"newName":"renamed"}}))?;
    assert_eq!(client.receive(4)["error"]["code"], -32803);
    client.send(&json!({"jsonrpc":"2.0","id":5,"method":"shutdown"}))?;
    assert_eq!(client.receive(5)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn actions_apply_only_safe_current_diagnostic_fixes() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/actions/instar.toml"),
    )?;

    let source = include_str!("fixtures/actions/input.luau");
    let expected = include_str!("fixtures/actions/expected.luau");
    let path = directory.path().join("input.luau");
    fs::write(&path, source)?;
    let document = uri(&path);
    let mut client = Client::new(directory.path())?;
    client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"workspace":{"workspaceEdit":{"documentChanges":true}},"textDocument":{"codeAction":{"codeActionLiteralSupport":{"codeActionKind":{"valueSet":["quickfix","source.fixAll.instar"]}}}}}}}))?;

    assert_eq!(
        client.receive(1)["result"]["capabilities"]["codeActionProvider"],
        json!({"resolveProvider":true,"codeActionKinds":["quickfix","source.fixAll.instar","source.organizeImports","refactor.extract"]})
    );

    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;

    let range =
        json!({"start":{"line":0,"character":0},"end":position(source, source.len(), "utf-16")});

    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/codeAction","params":{"textDocument":{"uri":document},"range":range,"context":{"diagnostics":[],"only":["quickfix"]}}}))?;
    let response = client.receive(2);

    let actions = response["result"]
        .as_array()
        .unwrap_or_else(|| panic!("quick fixes: {response}"));

    assert_eq!(actions.len(), 2, "{response}");
    assert!(actions.iter().all(|action| action["kind"] == "quickfix"));

    let codes = actions
        .iter()
        .map(|action| {
            action["diagnostics"][0]["code"]
                .as_str()
                .expect("diagnostic code")
        })
        .collect::<Vec<_>>();

    assert!(codes.contains(&"parenthesized_conditions"));
    assert!(codes.contains(&"self_assignment"));
    client.send(&json!({"jsonrpc":"2.0","id":3,"method":"textDocument/codeAction","params":{"textDocument":{"uri":document},"range":range,"context":{"diagnostics":[],"only":["source.fixAll"]}}}))?;
    let response = client.receive(3);
    let actions = response["result"].as_array().expect("fix-all actions");
    assert_eq!(actions.len(), 1, "{response}");
    assert_eq!(actions[0]["kind"], "source.fixAll.instar");
    let edit_response = json!({"result":actions[0]["edit"]});
    let edits = document_edits(&edit_response, &document, Some(1), true);
    assert_eq!(edits.len(), 3);
    assert_eq!(apply_edits(source, edits), expected);
    assert_eq!(fs::read_to_string(&path)?, source);
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":2},"contentChanges":[{"text":expected}]}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":4,"method":"textDocument/codeAction","params":{"textDocument":{"uri":document},"range":{"start":{"line":0,"character":0},"end":position(expected, expected.len(), "utf-16")},"context":{"diagnostics":[],"only":["source.fixAll"]}}}))?;
    let response = client.receive(4);
    assert_eq!(response["result"], json!([]), "{response}");
    client.send(&json!({"jsonrpc":"2.0","id":5,"method":"shutdown"}))?;
    assert_eq!(client.receive(5)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn actions_organize_imports_without_semantic_preparation() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/symbols/instar.toml"),
    )?;

    let source = include_str!("fixtures/actions/imports.luau");
    let expected = include_str!("fixtures/actions/organized.luau");
    let path = directory.path().join("imports.luau");
    fs::write(&path, source)?;
    let document = uri(&path);
    let mut client = Client::new(directory.path())?;

    client.send(
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"textDocument":{"codeAction":{"codeActionLiteralSupport":{"codeActionKind":{"valueSet":["source.organizeImports"]}}}}}}}),
    )?;

    assert!(client.receive(1).get("error").is_none());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
    let range = json!({"start":{"line":0,"character":0},"end":{"line":0,"character":0}});
    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/codeAction","params":{"textDocument":{"uri":document},"range":range,"context":{"diagnostics":[],"only":["source.organizeImports"]}}}))?;
    let response = client.receive(2);
    let actions = response["result"].as_array().expect("import organization");
    assert_eq!(actions.len(), 1, "{response}");
    assert_eq!(actions[0]["kind"], "source.organizeImports");
    let edit_response = json!({"result":actions[0]["edit"]});
    let edits = document_edits(&edit_response, &document, None, false);
    assert_eq!(apply_edits(source, edits), expected);
    assert_eq!(fs::read_to_string(&path)?, source);

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/actions/ordering.toml"),
    )?;

    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":uri(&directory.path().join("instar.toml")),"type":2}]}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":2},"contentChanges":[{"text":include_str!("fixtures/actions/reverse.luau")}]}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":3,"method":"textDocument/codeAction","params":{"textDocument":{"uri":document},"range":range,"context":{"diagnostics":[],"only":["source.organizeImports"]}}}))?;
    let response = client.receive(3);

    let actions = response["result"]
        .as_array()
        .expect("ordered import organization");

    assert_eq!(actions.len(), 1, "{response}");
    assert_eq!(actions[0]["kind"], "source.organizeImports");
    let edit_response = json!({"result":actions[0]["edit"]});
    let edits = document_edits(&edit_response, &document, None, false);
    let expected = "local first = require(\"./first\")\nlocal second = require(\"./second\")\nreturn first, second\n";

    assert_eq!(
        apply_edits(include_str!("fixtures/actions/reverse.luau"), edits),
        expected
    );

    assert_eq!(fs::read_to_string(&path)?, source);
    client.send(&json!({"jsonrpc":"2.0","id":4,"method":"shutdown"}))?;
    assert_eq!(client.receive(4)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn actions_extract_literals_into_fresh_checked_bindings() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/actions/instar.toml"),
    )?;

    let source = include_str!("fixtures/actions/literal.luau");
    let expected = include_str!("fixtures/actions/extracted.luau");
    let path = directory.path().join("literal.luau");
    fs::write(&path, source)?;
    let document = uri(&path);
    let mut client = Client::new(directory.path())?;
    client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"workspace":{"workspaceEdit":{"documentChanges":true}},"textDocument":{"codeAction":{"codeActionLiteralSupport":{"codeActionKind":{"valueSet":["refactor.extract"]}}}}}}}))?;
    assert!(client.receive(1).get("error").is_none());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
    let offset = source.find("42").expect("literal value");
    let range = json!({"start":position(source, offset, "utf-16"),"end":position(source, offset + 2, "utf-16")});
    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/codeAction","params":{"textDocument":{"uri":document},"range":range,"context":{"diagnostics":[],"only":["refactor.extract"]}}}))?;
    let response = client.receive(2);
    let actions = response["result"].as_array().expect("literal extraction");
    assert_eq!(actions.len(), 1, "{response}");
    assert_eq!(actions[0]["kind"], "refactor.extract");
    let edit_response = json!({"result":actions[0]["edit"]});
    let edits = document_edits(&edit_response, &document, Some(1), true);
    assert_eq!(edits.len(), 2);
    assert_eq!(apply_edits(source, edits), expected);
    assert_eq!(fs::read_to_string(&path)?, source);
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":2},"contentChanges":[{"text":expected}]}}))?;
    let offset = expected.find("value =").expect("replacement binding") + "value = ".len();
    let range = json!({"start":position(expected, offset, "utf-16"),"end":position(expected, offset + "extracted1".len(), "utf-16")});
    client.send(&json!({"jsonrpc":"2.0","id":3,"method":"textDocument/codeAction","params":{"textDocument":{"uri":document},"range":range,"context":{"diagnostics":[],"only":["refactor.extract"]}}}))?;
    let response = client.receive(3);
    assert_eq!(response["result"], json!([]), "{response}");
    client.send(&json!({"jsonrpc":"2.0","id":4,"method":"shutdown"}))?;
    assert_eq!(client.receive(4)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn movements_update_imports_and_moved_consumers_without_writing_sources() -> io::Result<()> {
    for versioned in [false, true] {
        let directory = tempfile::tempdir()?;

        fs::write(
            directory.path().join("instar.toml"),
            include_str!("fixtures/selection/graph.toml"),
        )?;

        fs::write(
            directory.path().join(".luaurc"),
            include_str!("fixtures/movement/.luaurc"),
        )?;

        let dependency = directory.path().join("dependency.luau");
        let consumer = directory.path().join("consumer.luau");
        let main = directory.path().join("main.luau");
        let destination = directory.path().join("modules/renamed.luau");
        let relocated = directory.path().join("clients/use.luau");
        let dependency_source = include_str!("fixtures/movement/dependency.luau");
        let consumer_source = include_str!("fixtures/movement/consumer.luau");
        let main_source = include_str!("fixtures/movement/main.luau");
        let expected_consumer = include_str!("fixtures/movement/moved.luau");
        let expected_main = include_str!("fixtures/movement/expected.luau");
        fs::write(&dependency, dependency_source)?;
        fs::write(&consumer, consumer_source)?;
        fs::write(&main, main_source)?;
        let mut client = Client::new(directory.path())?;
        client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"workspaceFolders":[{"uri":uri(directory.path()),"name":"movement"}],"capabilities":{"workspace":{"workspaceEdit":{"documentChanges":versioned}}}}}))?;
        let response = client.receive(1);

        assert_eq!(
            response["result"]["capabilities"]["workspace"]["fileOperations"]["willRename"]["filters"]
                [0]["pattern"]["matches"],
            "file"
        );

        client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri(&consumer),"languageId":"luau","version":3,"text":consumer_source}}}))?;
        let files = json!([{"oldUri":uri(&dependency),"newUri":uri(&destination)},{"oldUri":uri(&consumer),"newUri":uri(&relocated)}]);
        client.send(&json!({"jsonrpc":"2.0","id":2,"method":"workspace/willRenameFiles","params":{"files":files}}))?;
        let response = client.receive(2);
        assert!(response.get("error").is_none(), "{response}");
        let edits = document_edits(&response, &uri(&consumer), Some(3), versioned);
        assert_eq!(apply_edits(consumer_source, edits), expected_consumer);
        let edits = document_edits(&response, &uri(&main), None, versioned);
        assert_eq!(apply_edits(main_source, edits), expected_main);
        assert_eq!(fs::read_to_string(&dependency)?, dependency_source);
        assert_eq!(fs::read_to_string(&consumer)?, consumer_source);
        assert_eq!(fs::read_to_string(&main)?, main_source);
        assert!(!destination.exists());
        assert!(!relocated.exists());
        fs::write(&consumer, expected_consumer)?;
        fs::write(&main, expected_main)?;
        fs::create_dir_all(destination.parent().expect("destination directory"))?;
        fs::create_dir_all(relocated.parent().expect("consumer directory"))?;
        fs::rename(&dependency, &destination)?;
        fs::rename(&consumer, &relocated)?;
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri(&consumer),"version":4},"contentChanges":[{"text":expected_consumer}]}}))?;
        client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":uri(&main),"type":2}]}}))?;

        client.send(
            &json!({"jsonrpc":"2.0","method":"workspace/didRenameFiles","params":{"files":files}}),
        )?;

        client.send(&json!({"jsonrpc":"2.0","id":3,"method":"workspace/willRenameFiles","params":{"files":[{"oldUri":uri(&destination),"newUri":uri(&directory.path().join("modules/final.luau"))}]}}))?;
        let response = client.receive(3);
        assert!(response.get("error").is_none(), "{response}");

        assert_eq!(
            document_edits(&response, &uri(&relocated), Some(4), versioned).len(),
            1
        );

        assert_eq!(
            document_edits(&response, &uri(&main), None, versioned).len(),
            1
        );

        client.send(&json!({"jsonrpc":"2.0","id":4,"method":"shutdown"}))?;
        assert_eq!(client.receive(4)["result"], Value::Null);
        client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
        assert!(client.wait()?.success());
    }

    Ok(())
}

#[test]
fn movements_reject_collisions_ambiguity_and_resolution_capture() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/symbols/instar.toml"),
    )?;

    let source = include_str!("fixtures/movement/dependency.luau");
    let dependency = directory.path().join("dependency.luau");
    fs::write(&dependency, source)?;
    fs::write(directory.path().join("ambiguous.lua"), source)?;

    fs::write(
        directory.path().join("capture.luau"),
        include_str!("fixtures/movement/capture.luau"),
    )?;

    let cases: Vec<String> =
        serde_json::from_str(include_str!("fixtures/movement/rejections.json"))?;

    let mut client = Client::new(directory.path())?;
    client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"workspaceFolders":[{"uri":uri(directory.path()),"name":"movement"}],"capabilities":{}}}))?;
    assert!(client.receive(1).get("error").is_none());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;

    for (index, destination) in cases.iter().enumerate() {
        let identifier = i64::try_from(index).expect("case index") + 2;
        client.send(&json!({"jsonrpc":"2.0","id":identifier,"method":"workspace/willRenameFiles","params":{"files":[{"oldUri":uri(&dependency),"newUri":uri(&directory.path().join(destination))}]}}))?;
        let response = client.receive(identifier);
        assert!(response.get("error").is_some(), "{destination}: {response}");
        assert_eq!(fs::read_to_string(&dependency)?, source);
    }

    client.send(&json!({"jsonrpc":"2.0","id":10,"method":"shutdown"}))?;
    assert_eq!(client.receive(10)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn tokens_classify_native_roles_in_each_position_encoding() -> io::Result<()> {
    let legend: Value = serde_json::from_str(include_str!("fixtures/tokens/legend.json"))?;
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/tokens/cases.json"))?;

    for encoding in ["utf-8", "utf-16", "utf-32"] {
        let directory = tempfile::tempdir()?;

        fs::write(
            directory.path().join("instar.toml"),
            include_str!("fixtures/hover/instar.toml"),
        )?;

        fs::write(
            directory.path().join("definitions.luau"),
            include_str!("fixtures/hover/definitions.luau"),
        )?;

        fs::write(
            directory.path().join("documentation.json"),
            include_str!("fixtures/hover/documentation.json"),
        )?;

        let source = include_str!("fixtures/tokens/input.luau")
            .replace("\r\n", "\n")
            .replace('\n', "\r\n");

        let document = uri(&directory.path().join("tokens.luau"));
        let mut client = Client::new(directory.path())?;
        client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"general":{"positionEncodings":[encoding]},"textDocument":{"semanticTokens":{"requests":{"full":true},"tokenTypes":legend["tokenTypes"],"tokenModifiers":legend["tokenModifiers"],"formats":["relative"]}}}}}))?;
        let response = client.receive(1);
        let provider = &response["result"]["capabilities"]["semanticTokensProvider"];
        assert_eq!(provider["legend"], legend);
        assert_eq!(provider["full"], true);
        assert!(provider.get("range").is_none());
        client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
        client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/semanticTokens/full","params":{"textDocument":{"uri":document}}}))?;
        let response = client.receive(2);
        assert!(response.get("error").is_none(), "{response}");
        assert!(response["result"].get("resultId").is_none());

        assert_tokens(
            &source,
            encoding,
            &response["result"]["data"],
            &legend,
            &cases,
        );

        client.send(&json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}))?;
        assert_eq!(client.receive(3)["result"], Value::Null);
        client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
        assert!(client.wait()?.success());
    }

    Ok(())
}

#[test]
fn tokens_apply_deltas_render_ranges_and_forget_closed_documents() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/hover/instar.toml"),
    )?;

    fs::write(
        directory.path().join("definitions.luau"),
        include_str!("fixtures/hover/definitions.luau"),
    )?;

    fs::write(
        directory.path().join("documentation.json"),
        include_str!("fixtures/hover/documentation.json"),
    )?;

    let source = include_str!("fixtures/tokens/input.luau");
    let updated = include_str!("fixtures/tokens/updated.luau");
    let legend: Value = serde_json::from_str(include_str!("fixtures/tokens/legend.json"))?;
    let document = "untitled:tokens";
    let mut client = Client::new(directory.path())?;
    client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"workspaceFolders":[{"uri":uri(directory.path()),"name":"tokens"}],"capabilities":{"textDocument":{"semanticTokens":{"requests":{"full":{"delta":true},"range":true},"tokenTypes":legend["tokenTypes"],"tokenModifiers":legend["tokenModifiers"],"formats":["relative"]}}}}}))?;
    let response = client.receive(1);

    assert_eq!(
        response["result"]["capabilities"]["semanticTokensProvider"]["full"]["delta"],
        true
    );

    assert_eq!(
        response["result"]["capabilities"]["semanticTokensProvider"]["range"],
        true
    );

    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/semanticTokens/full","params":{"textDocument":{"uri":document}}}))?;
    let full = client.receive(2);
    assert!(full.get("error").is_none(), "{full}");

    let mut data = full["result"]["data"]
        .as_array()
        .expect("full tokens")
        .clone();

    client.send(&json!({"jsonrpc":"2.0","id":3,"method":"textDocument/semanticTokens/full/delta","params":{"textDocument":{"uri":document},"previousResultId":full["result"]["resultId"]}}))?;
    let unchanged = client.receive(3);
    assert_eq!(unchanged["result"]["edits"], json!([]), "{unchanged}");
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":2},"contentChanges":[{"text":updated}]}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":4,"method":"textDocument/semanticTokens/full/delta","params":{"textDocument":{"uri":document},"previousResultId":unchanged["result"]["resultId"]}}))?;
    let changed = client.receive(4);
    assert!(changed.get("error").is_none(), "{changed}");
    let edits = changed["result"]["edits"].as_array().expect("token delta");
    assert_ne!(edits.as_slice(), [] as [Value; 0]);

    for edit in edits.iter().rev() {
        let start =
            usize::try_from(edit["start"].as_u64().expect("delta start")).expect("start index");

        let count = usize::try_from(edit["deleteCount"].as_u64().expect("delta deletion"))
            .expect("deletion length");

        assert!(start + count <= data.len());

        data.splice(
            start..start + count,
            edit["data"].as_array().cloned().unwrap_or_default(),
        );
    }

    client.send(&json!({"jsonrpc":"2.0","id":5,"method":"textDocument/semanticTokens/full","params":{"textDocument":{"uri":document}}}))?;
    let current = client.receive(5);
    assert_eq!(json!(data), current["result"]["data"], "{current}");
    let start = updated.find("object.value +=").expect("range start");
    let end = start + "object.value += adjust(20)".len();

    let range =
        json!({"start":position(updated, start, "utf-16"),"end":position(updated, end, "utf-16")});

    client.send(&json!({"jsonrpc":"2.0","id":6,"method":"textDocument/semanticTokens/range","params":{"textDocument":{"uri":document},"range":range}}))?;
    let response = client.receive(6);
    assert!(response["result"].get("resultId").is_none());
    let tokens = decode_tokens(&response["result"]["data"], &legend);
    assert_ne!(tokens.as_slice(), [] as [Value; 0]);

    for token in tokens {
        assert_eq!(token["line"], range["start"]["line"]);

        assert!(
            token["character"].as_u64().expect("column")
                >= range["start"]["character"].as_u64().expect("start")
        );

        assert!(
            token["character"].as_u64().expect("column")
                + token["length"].as_u64().expect("length")
                <= range["end"]["character"].as_u64().expect("end")
        );
    }

    client.send(&json!({"jsonrpc":"2.0","id":7,"method":"textDocument/semanticTokens/full/delta","params":{"textDocument":{"uri":document},"previousResultId":"unknown"}}))?;

    assert_eq!(
        client.receive(7)["result"]["data"],
        current["result"]["data"]
    );

    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":document}}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":updated}}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":8,"method":"textDocument/semanticTokens/full/delta","params":{"textDocument":{"uri":document},"previousResultId":current["result"]["resultId"]}}))?;

    assert_eq!(
        client.receive(8)["result"]["data"],
        current["result"]["data"]
    );

    client.send(&json!({"jsonrpc":"2.0","id":9,"method":"shutdown"}))?;
    assert_eq!(client.receive(9)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn tokens_negotiate_supported_legends_and_request_formats() -> io::Result<()> {
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/tokens/negotiation.json"))?;

    for case in cases {
        let directory = tempfile::tempdir()?;

        fs::write(
            directory.path().join("instar.toml"),
            include_str!("fixtures/diagnostics/instar.toml"),
        )?;

        let document = uri(&directory.path().join("tokens.luau"));
        let mut client = Client::new(directory.path())?;
        client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"textDocument":{"semanticTokens":{"requests":{"full":true},"tokenTypes":case["types"],"tokenModifiers":[],"formats":case["formats"]}}}}}))?;
        let response = client.receive(1);
        let provider = &response["result"]["capabilities"]["semanticTokensProvider"];
        let supported = case["supported"].as_bool().expect("negotiated support");

        if supported {
            assert_eq!(provider["legend"]["tokenTypes"], json!(["variable"]));
            assert_eq!(provider["legend"]["tokenModifiers"], json!([]));
            assert_eq!(provider["full"], true);
        } else {
            assert!(provider.is_null(), "{response}");
        }

        client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":include_str!("fixtures/hover/input.luau")}}}))?;
        client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/semanticTokens/full","params":{"textDocument":{"uri":document}}}))?;
        let response = client.receive(2);

        if supported {
            let data = response["result"]["data"]
                .as_array()
                .expect("negotiated tokens");

            assert_ne!(data.as_slice(), [] as [Value; 0]);
            assert_eq!(data.len() % 5, 0);

            assert!(
                data.as_chunks::<5>()
                    .0
                    .iter()
                    .all(|token| token[3] == 0 && token[4] == 0)
            );

            assert!(response["result"].get("resultId").is_none());
        } else {
            assert!(response.get("error").is_some(), "{response}");
        }

        client.send(&json!({"jsonrpc":"2.0","id":3,"method":"textDocument/semanticTokens/full/delta","params":{"textDocument":{"uri":document},"previousResultId":"unknown"}}))?;
        assert!(client.receive(3).get("error").is_some());
        client.send(&json!({"jsonrpc":"2.0","id":4,"method":"shutdown"}))?;
        assert_eq!(client.receive(4)["result"], Value::Null);
        client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
        assert!(client.wait()?.success());
    }

    Ok(())
}

#[test]
fn hints_infer_binding_types_in_negotiated_ranges() -> io::Result<()> {
    let expected: Vec<Value> = serde_json::from_str(include_str!("fixtures/hints/expected.json"))?;
    let source = include_str!("fixtures/hints/input.luau").replace('\n', "\r\n");

    for encoding in ["utf-8", "utf-16", "utf-32"] {
        let directory = tempfile::tempdir()?;

        fs::write(
            directory.path().join("instar.toml"),
            include_str!("fixtures/diagnostics/instar.toml"),
        )?;

        let path = directory.path().join("hints.luau");
        fs::write(&path, &source)?;
        let document = uri(&path);
        let mut client = Client::new(directory.path())?;
        client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"general":{"positionEncodings":[encoding]},"textDocument":{"inlayHint":{}}}}}))?;
        let response = client.receive(1);
        assert!(response["result"]["capabilities"]["inlayHintProvider"].is_object());

        assert!(
            response["result"]["capabilities"]["inlayHintProvider"]["resolveProvider"].is_null()
        );

        client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
        let end = position(&source, source.len(), encoding);
        client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/inlayHint","params":{"textDocument":{"uri":document},"range":{"start":{"line":0,"character":0},"end":end}}}))?;
        let response = client.receive(2);
        assert!(response.get("error").is_none(), "{response}");
        let hints = response["result"].as_array().expect("inlay hints");
        assert_eq!(hints.len(), expected.len(), "{hints:?}");

        for (hint, expected) in hints.iter().zip(&expected) {
            let name = expected["name"].as_str().expect("binding name");
            let offset = source.find(name).expect("binding") + name.len();
            assert_eq!(hint["position"], position(&source, offset, encoding));
            assert_eq!(hint["label"], expected["label"]);
            assert_eq!(hint["kind"], 1);

            assert!(
                hint["tooltip"]
                    .as_str()
                    .expect("eager tooltip")
                    .contains(name)
            );

            assert!(hint.get("data").is_none());
            assert!(hint.get("textEdits").is_none());
        }

        let count = source.find("count").expect("count binding") + "count".len();
        let point = position(&source, count, encoding);
        client.send(&json!({"jsonrpc":"2.0","id":3,"method":"textDocument/inlayHint","params":{"textDocument":{"uri":document},"range":{"start":point,"end":point}}}))?;
        let response = client.receive(3);
        assert_eq!(response["result"].as_array().expect("range hints").len(), 1);
        assert_eq!(response["result"][0]["position"], point);
        assert_eq!(response["result"][0]["label"], ": number");
        assert_eq!(fs::read_to_string(&path)?, source);
        client.send(&json!({"jsonrpc":"2.0","id":4,"method":"shutdown"}))?;
        assert_eq!(client.receive(4)["result"], Value::Null);
        client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
        assert!(client.wait()?.success());
    }

    Ok(())
}

#[test]
fn hints_resolve_selectively_and_reject_stale_identities() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/diagnostics/instar.toml"),
    )?;

    let source = include_str!("fixtures/hints/input.luau");
    let document = "untitled:hints";
    let mut client = Client::new(directory.path())?;
    client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"workspaceFolders":[{"uri":uri(directory.path()),"name":"hints"}],"capabilities":{"textDocument":{"inlayHint":{"resolveSupport":{"properties":["tooltip"]}}}}}}))?;

    assert_eq!(
        client.receive(1)["result"]["capabilities"]["inlayHintProvider"]["resolveProvider"],
        true
    );

    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
    let end = position(source, source.len(), "utf-16");
    let request = json!({"textDocument":{"uri":document},"range":{"start":{"line":0,"character":0},"end":end}});

    client.send(
        &json!({"jsonrpc":"2.0","id":2,"method":"textDocument/inlayHint","params":request}),
    )?;

    let response = client.receive(2);
    assert!(response.get("error").is_none(), "{response}");
    let hint = response["result"][0].clone();
    assert!(hint.get("tooltip").is_none());
    assert!(hint["data"].is_object());
    client.send(&json!({"jsonrpc":"2.0","id":3,"method":"inlayHint/resolve","params":hint}))?;
    let resolved = client.receive(3);

    assert!(
        resolved["result"]["tooltip"]
            .as_str()
            .expect("resolved tooltip")
            .contains("text: string")
    );

    assert_eq!(resolved["result"]["position"], hint["position"]);
    assert_eq!(resolved["result"]["label"], hint["label"]);
    assert!(resolved["result"].get("textEdits").is_none());
    let mut invalid = hint.clone();
    invalid["data"]["item"] = json!(99999);
    client.send(&json!({"jsonrpc":"2.0","id":4,"method":"inlayHint/resolve","params":invalid}))?;
    assert_eq!(client.receive(4)["error"]["code"], -32602);
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":2},"contentChanges":[{"text":include_str!("fixtures/hover/updated.luau")}]}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":5,"method":"inlayHint/resolve","params":hint}))?;
    assert_eq!(client.receive(5)["error"]["code"], -32801);
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":3},"contentChanges":[{"text":source}]}}))?;

    client.send(
        &json!({"jsonrpc":"2.0","id":6,"method":"textDocument/inlayHint","params":request}),
    )?;

    let current = client.receive(6)["result"][0].clone();
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":document}}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":7,"method":"inlayHint/resolve","params":current}))?;
    assert_eq!(client.receive(7)["error"]["code"], -32801);
    client.send(&json!({"jsonrpc":"2.0","id":8,"method":"shutdown"}))?;
    assert_eq!(client.receive(8)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn folding_uses_syntax_and_negotiated_client_limits() -> io::Result<()> {
    let expected: Vec<Value> = serde_json::from_str(include_str!("fixtures/structure/folds.json"))?;
    let source = include_str!("fixtures/structure/input.luau").replace('\n', "\r\n");

    for encoding in ["utf-8", "utf-16", "utf-32"] {
        for line_only in [false, true] {
            let directory = tempfile::tempdir()?;

            fs::write(
                directory.path().join("instar.toml"),
                include_str!("fixtures/symbols/instar.toml"),
            )?;

            let document = uri(&directory.path().join("structure.luau"));
            let mut client = Client::new(directory.path())?;
            client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"general":{"positionEncodings":[encoding]},"textDocument":{"foldingRange":{"lineFoldingOnly":line_only,"rangeLimit":7,"foldingRangeKind":{"valueSet":["comment"]}}}}}}))?;

            assert_eq!(
                client.receive(1)["result"]["capabilities"]["foldingRangeProvider"],
                true
            );

            client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
            client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
            client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/foldingRange","params":{"textDocument":{"uri":document}}}))?;
            let response = client.receive(2);
            assert!(response.get("error").is_none(), "{response}");
            let folds = response["result"].as_array().expect("folding ranges");
            assert_eq!(folds.len(), expected.len());

            for (fold, expected) in folds.iter().zip(&expected) {
                assert_eq!(fold["startLine"], expected["start"]);

                assert_eq!(
                    fold["endLine"],
                    expected[if line_only { "lines" } else { "end" }]
                );

                assert_eq!(fold["kind"], expected["kind"]);
                assert!(fold.get("startCharacter").is_none());
                assert_eq!(fold.get("endCharacter").is_none(), line_only);
            }

            if !line_only {
                let end = source.find("]]").expect("string closing delimiter") + 2;

                assert_eq!(
                    folds[5]["endCharacter"],
                    position(&source, end, encoding)["character"]
                );
            }

            client.send(&json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}))?;
            assert_eq!(client.receive(3)["result"], Value::Null);
            client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
            assert!(client.wait()?.success());
        }
    }

    Ok(())
}

#[test]
fn folding_obeys_small_limits_and_supported_kinds() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/symbols/instar.toml"),
    )?;

    for limit in [0, 2] {
        let document = uri(&directory.path().join("structure.luau"));
        let mut client = Client::new(directory.path())?;
        client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"textDocument":{"foldingRange":{"rangeLimit":limit,"foldingRangeKind":{"valueSet":[]}}}}}}))?;
        assert!(client.receive(1).get("error").is_none());
        client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":include_str!("fixtures/structure/input.luau")}}}))?;
        client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/foldingRange","params":{"textDocument":{"uri":document}}}))?;
        let response = client.receive(2);
        let folds = response["result"].as_array().expect("limited folds");
        assert_eq!(folds.len(), limit);
        assert!(folds.iter().all(|fold| fold.get("kind").is_none()));
        client.send(&json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}))?;
        assert_eq!(client.receive(3)["result"], Value::Null);
        client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
        assert!(client.wait()?.success());
    }

    Ok(())
}

#[test]
fn selections_nest_through_current_and_incomplete_syntax() -> io::Result<()> {
    let source = include_str!("fixtures/structure/input.luau").replace('\n', "\r\n");

    let coordinates = |point: &Value| {
        (
            point["line"].as_u64().expect("line"),
            point["character"].as_u64().expect("character"),
        )
    };

    for encoding in ["utf-8", "utf-16", "utf-32"] {
        let directory = tempfile::tempdir()?;

        fs::write(
            directory.path().join("instar.toml"),
            include_str!("fixtures/symbols/instar.toml"),
        )?;

        let document = "untitled:structure";
        let mut client = Client::new(directory.path())?;
        client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"workspaceFolders":[{"uri":uri(directory.path()),"name":"structure"}],"capabilities":{"general":{"positionEncodings":[encoding]}}}}))?;

        assert_eq!(
            client.receive(1)["result"]["capabilities"]["selectionRangeProvider"],
            true
        );

        client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
        let start = source.find("object").expect("binding");
        let point = position(&source, start + 1, encoding);
        let end = position(&source, source.len(), encoding);
        client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/selectionRange","params":{"textDocument":{"uri":document},"positions":[point,end]}}))?;
        let response = client.receive(2);
        assert!(response.get("error").is_none(), "{response}");
        let selections = response["result"].as_array().expect("selections");
        assert_eq!(selections.len(), 2);

        assert_eq!(
            selections[0]["range"],
            json!({"start":position(&source,start,encoding),"end":position(&source,start + "object".len(),encoding)})
        );

        for (selection, point) in selections.iter().zip([&point, &end]) {
            let mut current = selection;
            assert!(coordinates(&current["range"]["start"]) <= coordinates(point));
            assert!(coordinates(&current["range"]["end"]) >= coordinates(point));

            while let Some(parent) = current.get("parent") {
                assert!(
                    coordinates(&parent["range"]["start"])
                        <= coordinates(&current["range"]["start"])
                );

                assert!(
                    coordinates(&parent["range"]["end"]) >= coordinates(&current["range"]["end"])
                );

                assert_ne!(parent["range"], current["range"]);
                current = parent;
            }

            assert_eq!(
                current["range"],
                json!({"start":{"line":0,"character":0},"end":end})
            );
        }

        let incomplete = include_str!("fixtures/structure/incomplete.luau");
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":2},"contentChanges":[{"text":incomplete}]}}))?;
        let start = incomplete.find("value").expect("field");
        client.send(&json!({"jsonrpc":"2.0","id":3,"method":"textDocument/selectionRange","params":{"textDocument":{"uri":document},"positions":[position(incomplete,start + 1,encoding)]}}))?;
        let response = client.receive(3);

        assert_eq!(
            response["result"][0]["range"],
            json!({"start":position(incomplete,start,encoding),"end":position(incomplete,start + "value".len(),encoding)})
        );

        client.send(&json!({"jsonrpc":"2.0","id":4,"method":"textDocument/foldingRange","params":{"textDocument":{"uri":document}}}))?;
        assert_eq!(client.receive(4)["result"][0]["startLine"], 0);
        client.send(&json!({"jsonrpc":"2.0","id":5,"method":"shutdown"}))?;
        assert_eq!(client.receive(5)["result"], Value::Null);
        client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
        assert!(client.wait()?.success());
    }

    Ok(())
}

#[test]
fn document_links_use_aliases_overlays_and_documentation() -> io::Result<()> {
    let source = include_str!("fixtures/links/input.luau").replace('\n', "\r\n");
    let expected: Vec<Value> = serde_json::from_str(include_str!("fixtures/links/expected.json"))?;

    for (encoding, tooltips) in [("utf-8", true), ("utf-16", false), ("utf-32", true)] {
        let directory = tempfile::tempdir()?;

        for (name, text) in [
            ("instar.toml", include_str!("fixtures/symbols/instar.toml")),
            (".luaurc", include_str!("fixtures/movement/.luaurc")),
            (
                "module.luau",
                include_str!("fixtures/completion/dependency.luau"),
            ),
            ("entry.luau", source.as_str()),
        ] {
            fs::write(directory.path().join(name), text)?;
        }

        let document = uri(&directory.path().join("entry.luau"));
        let mut client = Client::new(directory.path())?;
        client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"general":{"positionEncodings":[encoding]},"textDocument":{"documentLink":{"tooltipSupport":tooltips}}}}}))?;
        assert!(client.receive(1)["result"]["capabilities"]["documentLinkProvider"].is_object());
        client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
        client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/documentLink","params":{"textDocument":{"uri":document}}}))?;
        let response = client.receive(2);
        assert!(response.get("error").is_none(), "{response}");
        let links = response["result"].as_array().expect("document links");
        assert_eq!(links.len(), expected.len(), "{links:?}");

        for (link, expected) in links.iter().zip(&expected) {
            let text = expected["text"].as_str().expect("link source");
            let start = source.find(text).expect("link location");

            let target = expected["file"].as_str().map_or_else(
                || {
                    expected["address"]
                        .as_str()
                        .expect("documentation address")
                        .to_owned()
                },
                |name| uri(&directory.path().join(name)),
            );

            assert_eq!(link["target"], target);

            assert_eq!(
                link["range"],
                json!({"start":position(&source,start,encoding),"end":position(&source,start+text.len(),encoding)})
            );

            assert_eq!(link.get("tooltip").is_some(), tooltips);

            if tooltips {
                assert_eq!(link["tooltip"], target);
            }
        }

        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":2},"contentChanges":[{"text":include_str!("fixtures/links/updated.luau")}]}}))?;
        client.send(&json!({"jsonrpc":"2.0","id":3,"method":"textDocument/documentLink","params":{"textDocument":{"uri":document}}}))?;
        let response = client.receive(3);

        assert_eq!(
            response["result"].as_array().expect("updated links").len(),
            1
        );

        assert_eq!(
            response["result"][0]["target"],
            "https://example.com/updated"
        );

        let other = directory.path().join("other.luau");
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri(&other),"languageId":"luau","version":1,"text":include_str!("fixtures/completion/dependency.luau")}}}))?;
        client.send(&json!({"jsonrpc":"2.0","id":4,"method":"textDocument/documentLink","params":{"textDocument":{"uri":document}}}))?;
        let response = client.receive(4);

        assert_eq!(
            response["result"]
                .as_array()
                .expect("overlay target links")
                .len(),
            2
        );

        assert_eq!(response["result"][0]["target"], uri(&other));

        assert_eq!(
            fs::read_to_string(directory.path().join("entry.luau"))?,
            source
        );

        assert!(!other.exists());
        client.send(&json!({"jsonrpc":"2.0","id":5,"method":"shutdown"}))?;
        assert_eq!(client.receive(5)["result"], Value::Null);
        client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
        assert!(client.wait()?.success());
    }

    Ok(())
}

#[test]
fn document_links_require_mapped_context_agreement() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    for (name, text) in [
        (
            "instar.toml",
            include_str!("../../project/tests/fixtures/checking/roblox/instar.toml"),
        ),
        (
            "sourcemap.json",
            include_str!("../../project/tests/fixtures/checking/explicit/sourcemap.json"),
        ),
        (
            "number.luau",
            include_str!("fixtures/completion/dependency.luau"),
        ),
        (
            "string.luau",
            include_str!("../../project/tests/fixtures/checking/explicit/string.luau"),
        ),
    ] {
        fs::write(directory.path().join(name), text)?;
    }

    let mut client = Client::new(directory.path())?;

    client.send(
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}),
    )?;

    assert!(client.receive(1).get("error").is_none());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    roblox_assets(&mut client)?;
    let source = include_str!("fixtures/links/mapped.luau");
    let document = uri(&directory.path().join("caller.luau"));
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/documentLink","params":{"textDocument":{"uri":document}}}))?;
    let response = client.receive(2);
    assert!(response.get("error").is_none(), "{response}");
    let links = response["result"].as_array().expect("agreed links");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0]["range"]["start"]["line"], 1);
    let document = uri(&directory.path().join("number.luau"));
    assert_eq!(links[0]["target"], document);
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":3,"method":"textDocument/documentLink","params":{"textDocument":{"uri":document}}}))?;
    let response = client.receive(3);
    let links = response["result"].as_array().expect("unique context links");
    assert_eq!(links.len(), 2);
    assert!(links.iter().all(|link| link["target"] == document));
    client.send(&json!({"jsonrpc":"2.0","id":4,"method":"shutdown"}))?;
    assert_eq!(client.receive(4)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn pull_diagnostics_match_push_results_and_recover_from_changed_inputs() -> io::Result<()> {
    for encoding in ["utf-8", "utf-16", "utf-32"] {
        let directory = tempfile::tempdir()?;
        let configuration = directory.path().join("instar.toml");

        fs::write(
            &configuration,
            include_str!("fixtures/diagnostics/instar.toml"),
        )?;

        let path = directory.path().join("entry.luau");
        let source = include_str!("fixtures/diagnostics/unicode.luau").replace('\n', "\r\n");
        let valid = include_str!("fixtures/diagnostics/valid.luau");
        fs::write(&path, valid)?;

        let document = if encoding == "utf-32" {
            "untitled:diagnostics".to_owned()
        } else {
            uri(&path)
        };

        let mut push = Client::new(directory.path())?;
        push.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"general":{"positionEncodings":[encoding]}}}}))?;
        assert!(push.receive(1).get("result").is_some());
        push.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
        push.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
        let expected = push.diagnostics(&document, Some(1));

        assert!(
            expected
                .as_array()
                .expect("push findings")
                .iter()
                .any(|diagnostic| diagnostic["range"]["start"]
                    == position(&source, source.find("12").expect("invalid value"), encoding))
        );

        push.send(&json!({"jsonrpc":"2.0","id":2,"method":"shutdown"}))?;
        assert_eq!(push.receive(2)["result"], Value::Null);
        push.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
        assert!(push.wait()?.success());

        let mut client = Client::new(directory.path())?;
        client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"textDocument":{"diagnostic":{}},"general":{"positionEncodings":[encoding]}}}}))?;

        assert_eq!(
            client.receive(1)["result"]["capabilities"]["diagnosticProvider"],
            json!({"interFileDependencies":true,"workspaceDiagnostics":true})
        );

        client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
        client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/diagnostic","params":{"textDocument":{"uri":document}}}))?;
        let report = client.receive(2)["result"].clone();
        assert_eq!(report["kind"], "full");
        assert_eq!(report["items"], expected);
        assert!(report["resultId"].is_string());
        client.send(&json!({"jsonrpc":"2.0","id":3,"method":"textDocument/diagnostic","params":{"textDocument":{"uri":document},"previousResultId":report["resultId"]}}))?;

        assert_eq!(
            client.receive(3)["result"],
            json!({"kind":"unchanged","resultId":report["resultId"]})
        );

        client.send(&json!({"jsonrpc":"2.0","id":4,"method":"textDocument/diagnostic","params":{"textDocument":{"uri":document},"previousResultId":"unknown"}}))?;
        assert_eq!(client.receive(4)["result"], report);
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":2},"contentChanges":[{"text":valid}]}}))?;
        client.send(&json!({"jsonrpc":"2.0","id":5,"method":"textDocument/diagnostic","params":{"textDocument":{"uri":document},"previousResultId":report["resultId"]}}))?;
        let clean = client.receive(5)["result"].clone();
        assert_eq!(clean["kind"], "full");
        assert_eq!(clean["items"], json!([]));
        assert_ne!(clean["resultId"], report["resultId"]);

        fs::write(
            &configuration,
            include_str!("fixtures/diagnostics/broken.toml"),
        )?;

        client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":uri(&configuration),"type":2}]}}))?;
        client.send(&json!({"jsonrpc":"2.0","id":6,"method":"textDocument/diagnostic","params":{"textDocument":{"uri":document},"previousResultId":clean["resultId"]}}))?;
        assert_eq!(client.receive(6)["error"]["code"], -32803);

        fs::write(
            &configuration,
            include_str!("fixtures/diagnostics/instar.toml"),
        )?;

        client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":uri(&configuration),"type":2}]}}))?;
        client.send(&json!({"jsonrpc":"2.0","id":7,"method":"textDocument/diagnostic","params":{"textDocument":{"uri":document},"previousResultId":clean["resultId"]}}))?;
        assert_eq!(client.receive(7)["result"]["kind"], "unchanged");
        assert_eq!(fs::read_to_string(&path)?, valid);
        client.send(&json!({"jsonrpc":"2.0","id":8,"method":"shutdown"}))?;
        assert_eq!(client.receive(8)["result"], Value::Null);
        client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
        assert!(client.wait()?.success());
    }

    Ok(())
}

#[test]
fn workspace_pull_diagnostics_report_dependencies_versions_and_deleted_files() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/workspace/instar.toml"),
    )?;

    let path = directory.path().join("entry.luau");
    let dependency = directory.path().join("dependency.luau");
    let source = include_str!("fixtures/diagnostics/caller.luau");
    let invalid = include_str!("fixtures/diagnostics/invalid.luau");
    fs::write(&path, source)?;
    fs::write(&dependency, invalid)?;
    fs::write(directory.path().join("excluded.luau"), invalid)?;
    let document = uri(&path);
    let dependency_uri = uri(&dependency);
    let mut client = Client::new(directory.path())?;
    client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"textDocument":{"diagnostic":{}}},"workspaceFolders":[{"uri":uri(directory.path()),"name":"fixture"}]}}))?;
    assert!(client.receive(1).get("result").is_some());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"workspace/diagnostic","params":{"previousResultIds":[]}}))?;
    let response = client.receive(2);

    let reports = response["result"]["items"]
        .as_array()
        .expect("workspace reports");

    assert_eq!(reports.len(), 2, "{response}");

    assert!(
        reports
            .iter()
            .any(|report| report["uri"] == document && report["version"] == 1)
    );

    assert!(reports.iter().any(|report| report["uri"] == dependency_uri
        && report["version"].is_null()
        && !report["items"].as_array().expect("findings").is_empty()));

    let previous = reports
        .iter()
        .map(|report| json!({"uri":report["uri"],"value":report["resultId"]}))
        .collect::<Vec<_>>();

    client.send(&json!({"jsonrpc":"2.0","id":3,"method":"workspace/diagnostic","params":{"previousResultIds":previous}}))?;

    assert!(
        client.receive(3)["result"]["items"]
            .as_array()
            .expect("unchanged reports")
            .iter()
            .all(|report| report["kind"] == "unchanged")
    );

    fs::remove_file(&dependency)?;
    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":dependency_uri,"type":3}]}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":4,"method":"workspace/diagnostic","params":{"previousResultIds":previous}}))?;
    let response = client.receive(4);

    let reports = response["result"]["items"]
        .as_array()
        .expect("changed reports");

    assert!(
        reports.iter().any(|report| report["uri"] == dependency_uri
            && report["kind"] == "full"
            && report["items"] == json!([])),
        "{response}"
    );

    assert!(
        reports.iter().any(|report| report["uri"] == document
            && report["kind"] == "full"
            && !report["items"]
                .as_array()
                .expect("missing dependency")
                .is_empty()),
        "{response}"
    );

    fs::write(&dependency, include_str!("fixtures/diagnostics/valid.luau"))?;
    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":dependency_uri,"type":1}]}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":5,"method":"workspace/diagnostic","params":{"previousResultIds":[]}}))?;

    assert!(
        client.receive(5)["result"]["items"]
            .as_array()
            .expect("recovered reports")
            .iter()
            .all(|report| report["items"] == json!([]))
    );

    assert_eq!(fs::read_to_string(&path)?, source);
    client.send(&json!({"jsonrpc":"2.0","id":6,"method":"shutdown"}))?;
    assert_eq!(client.receive(6)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn pull_diagnostics_negotiate_refresh_without_duplicate_publication() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/diagnostics/instar.toml"),
    )?;

    let document = uri(&directory.path().join("entry.luau"));
    let mut client = Client::new(directory.path())?;
    client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"textDocument":{"diagnostic":{}},"workspace":{"diagnostics":{"refreshSupport":true}}}}}))?;
    assert!(client.receive(1).get("result").is_some());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":include_str!("fixtures/diagnostics/invalid.luau")}}}))?;

    let refresh = client
        .messages
        .recv_timeout(Duration::from_secs(10))
        .expect("diagnostic refresh")
        .expect("refresh request");

    assert_eq!(refresh["method"], "workspace/diagnostic/refresh");
    client.send(&json!({"jsonrpc":"2.0","id":refresh["id"],"result":null}))?;
    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/diagnostic","params":{"textDocument":{"uri":document}}}))?;

    let report = client.message(Instant::now());

    assert_eq!(report["id"], 2, "{report}");
    assert_eq!(report["result"]["kind"], "full");
    client.send(&json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}))?;
    assert_eq!(client.receive(3)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn hierarchy_resolves_native_calls_across_workspace_sources() -> io::Result<()> {
    for encoding in ["utf-8", "utf-16", "utf-32"] {
        let directory = tempfile::tempdir()?;

        for (name, source) in [
            (
                "instar.toml",
                include_str!("fixtures/diagnostics/instar.toml"),
            ),
            (
                "dependency.luau",
                include_str!("fixtures/hierarchy/dependency.luau"),
            ),
            (
                "consumer.luau",
                include_str!("fixtures/hierarchy/consumer.luau"),
            ),
        ] {
            fs::write(directory.path().join(name), source)?;
        }

        let source = include_str!("fixtures/hierarchy/input.luau").replace('\n', "\r\n");
        let path = directory.path().join("input.luau");
        fs::write(&path, &source)?;
        let document = uri(&path);
        let mut client = Client::new(directory.path())?;
        client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"workspaceFolders":[{"uri":uri(directory.path()),"name":"hierarchy"}],"capabilities":{"general":{"positionEncodings":[encoding]}}}}))?;
        let initialized = client.receive(1);

        assert_eq!(
            initialized["result"]["capabilities"]["callHierarchyProvider"],
            true
        );

        assert_eq!(
            initialized["result"]["capabilities"]["implementationProvider"],
            true
        );

        client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
        let point = position(&source, source.find("caller").expect("caller"), encoding);
        client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/prepareCallHierarchy","params":{"textDocument":{"uri":document},"position":point}}))?;
        let prepared = client.receive(2);
        assert!(prepared.get("error").is_none(), "{prepared}");

        assert_eq!(
            prepared["result"].as_array().expect("prepared calls").len(),
            1,
            "{prepared}"
        );

        let caller = prepared["result"][0].clone();
        assert_eq!(caller["name"], "caller");
        assert_eq!(caller["selectionRange"]["start"], point);
        client.send(&json!({"jsonrpc":"2.0","id":3,"method":"callHierarchy/outgoingCalls","params":{"item":caller}}))?;
        let outgoing = client.receive(3);
        let calls = outgoing["result"].as_array().expect("outgoing calls");
        assert_eq!(calls.len(), 2, "{outgoing}");

        let target = calls
            .iter()
            .find(|call| call["to"]["name"] == "target")
            .expect("aliased call");

        assert_eq!(
            target["to"]["uri"],
            uri(&directory.path().join("dependency.luau"))
        );

        assert_eq!(
            target["fromRanges"].as_array().expect("call sites").len(),
            2
        );

        let offset = source.find("alias()").expect("first call");

        assert_eq!(
            target["fromRanges"][0],
            json!({"start":position(&source, offset, encoding),"end":position(&source, offset + "alias".len(), encoding)})
        );

        client.send(&json!({"jsonrpc":"2.0","id":4,"method":"callHierarchy/incomingCalls","params":{"item":target["to"]}}))?;
        let incoming = client.receive(4);
        let calls = incoming["result"].as_array().expect("incoming calls");
        assert_eq!(calls.len(), 3, "{incoming}");

        assert!(calls.iter().any(|call| call["from"]["name"] == "caller"
            && call["fromRanges"].as_array().expect("ranges").len() == 2));

        assert!(calls.iter().any(|call| call["from"]["name"] == "nested"));
        assert!(calls.iter().any(|call| call["from"]["name"] == "consumer"));
        client.send(&json!({"jsonrpc":"2.0","id":5,"method":"textDocument/implementation","params":{"textDocument":{"uri":document},"position":position(&source, offset, encoding)}}))?;
        let implementation = client.receive(5);

        assert_eq!(
            implementation["result"],
            json!([{"uri":target["to"]["uri"],"range":target["to"]["selectionRange"]}])
        );

        let mut invalid = caller.clone();
        invalid["uri"] = json!("untitled:forged");
        client.send(&json!({"jsonrpc":"2.0","id":6,"method":"callHierarchy/outgoingCalls","params":{"item":invalid}}))?;
        assert_eq!(client.receive(6)["error"]["code"], -32602);
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":2},"contentChanges":[{"text":include_str!("fixtures/hierarchy/dependency.luau")}]}}))?;
        client.send(&json!({"jsonrpc":"2.0","id":7,"method":"callHierarchy/outgoingCalls","params":{"item":caller}}))?;
        assert_eq!(client.receive(7)["error"]["code"], -32801);
        assert_eq!(fs::read_to_string(&path)?, source);
        client.send(&json!({"jsonrpc":"2.0","id":8,"method":"shutdown"}))?;
        assert_eq!(client.receive(8)["result"], Value::Null);
        client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
        assert!(client.wait()?.success());
    }

    Ok(())
}

#[test]
fn hierarchy_exposes_only_native_nominal_inheritance() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    for (name, source) in [
        (
            "instar.toml",
            include_str!("fixtures/hierarchy/instar.toml"),
        ),
        (
            "definitions.luau",
            include_str!("fixtures/hierarchy/definitions.luau"),
        ),
    ] {
        fs::write(directory.path().join(name), source)?;
    }

    let source = include_str!("fixtures/hierarchy/types.luau");
    let document = "untitled:types";
    let mut client = Client::new(directory.path())?;

    client.send(
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}),
    )?;

    assert_eq!(
        client.receive(1)["result"]["capabilities"]["typeHierarchyProvider"],
        true
    );

    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;

    let point = position(
        source,
        source.rfind("subject").expect("class instance"),
        "utf-16",
    );

    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/prepareTypeHierarchy","params":{"textDocument":{"uri":document},"position":point}}))?;
    let prepared = client.receive(2);
    assert!(prepared.get("error").is_none(), "{prepared}");

    assert_eq!(
        prepared["result"].as_array().expect("nominal types").len(),
        1,
        "{prepared}"
    );

    let child = prepared["result"][0].clone();
    assert_eq!(child["name"], "Child");

    assert!(
        child["uri"]
            .as_str()
            .expect("declaration URI")
            .starts_with("instar:")
    );

    client.send(&json!({"jsonrpc":"2.0","id":3,"method":"typeHierarchy/supertypes","params":{"item":child}}))?;
    let parents = client.receive(3);

    assert_eq!(
        parents["result"]
            .as_array()
            .unwrap_or_else(|| panic!("parents: {parents}"))
            .len(),
        1
    );

    assert_eq!(parents["result"][0]["name"], "Base");

    client.send(
        &json!({"jsonrpc":"2.0","id":4,"method":"typeHierarchy/subtypes","params":{"item":child}}),
    )?;

    let children = client.receive(4);
    assert_eq!(children["result"].as_array().expect("children").len(), 1);
    assert_eq!(children["result"][0]["name"], "Grandchild");
    client.send(&json!({"jsonrpc":"2.0","id":5,"method":"textDocument/implementation","params":{"textDocument":{"uri":document},"position":point}}))?;

    assert_eq!(
        client.receive(5)["result"],
        json!([{"uri":children["result"][0]["uri"],"range":children["result"][0]["selectionRange"]}])
    );

    client.send(&json!({"jsonrpc":"2.0","id":6,"method":"textDocument/prepareTypeHierarchy","params":{"textDocument":{"uri":document},"position":position(source, source.rfind("structural").expect("structural type"), "utf-16")}}))?;
    assert_eq!(client.receive(6)["result"], json!([]));
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":document}}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;

    client.send(
        &json!({"jsonrpc":"2.0","id":7,"method":"typeHierarchy/subtypes","params":{"item":child}}),
    )?;

    assert_eq!(client.receive(7)["error"]["code"], -32801);
    client.send(&json!({"jsonrpc":"2.0","id":8,"method":"shutdown"}))?;
    assert_eq!(client.receive(8)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn diagnostics_preserve_native_metadata_and_recover_from_incomplete_syntax() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    for (name, source) in [
        (
            "instar.toml",
            include_str!("fixtures/diagnostics/instar.toml"),
        ),
        (".luaurc", include_str!("fixtures/diagnostics/.luaurc")),
    ] {
        fs::write(directory.path().join(name), source)?;
    }

    let path = directory.path().join("entry.luau");
    let valid = include_str!("fixtures/diagnostics/valid.luau");
    fs::write(&path, valid)?;
    let document = uri(&path);
    let source = include_str!("fixtures/diagnostics/metadata.luau");
    let mut client = Client::new(directory.path())?;
    client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"textDocument":{"diagnostic":{}}}}}))?;
    assert!(client.receive(1).get("error").is_none());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/diagnostic","params":{"textDocument":{"uri":document}}}))?;
    let report = client.receive(2);

    let findings = report["result"]["items"]
        .as_array()
        .expect("native diagnostics");

    for (code, tag) in [(7, 1), (8, 1), (22, 2)] {
        let finding = findings
            .iter()
            .find(|finding| finding["source"] == "Luau" && finding["code"] == code)
            .unwrap_or_else(|| panic!("missing native warning {code}: {report}"));

        assert_eq!(finding["tags"], json!([tag]), "{finding}");
        assert_eq!(finding["codeDescription"]["href"], "https://luau.org/lint/");
        assert_eq!(finding["severity"], 2);
    }

    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":2},"contentChanges":[{"text":include_str!("fixtures/diagnostics/incomplete.luau")}]}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":3,"method":"textDocument/diagnostic","params":{"textDocument":{"uri":document},"previousResultId":report["result"]["resultId"]}}))?;
    let incomplete = client.receive(3);
    assert_eq!(incomplete["result"]["kind"], "full", "{incomplete}");

    let findings = incomplete["result"]["items"]
        .as_array()
        .expect("syntax findings");

    assert!(
        findings.iter().any(|finding| finding["source"] == "Luau"
            && finding["severity"] == 1
            && finding["code"].is_number()),
        "{incomplete}"
    );

    for (index, finding) in findings.iter().enumerate() {
        assert!(!findings[..index].contains(finding), "{incomplete}");
    }

    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":3},"contentChanges":[{"text":valid}]}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":4,"method":"textDocument/diagnostic","params":{"textDocument":{"uri":document},"previousResultId":incomplete["result"]["resultId"]}}))?;
    let recovered = client.receive(4);
    assert_eq!(recovered["result"]["kind"], "full");
    assert_eq!(recovered["result"]["items"], json!([]));
    assert_eq!(fs::read_to_string(&path)?, valid);
    client.send(&json!({"jsonrpc":"2.0","id":5,"method":"shutdown"}))?;
    assert_eq!(client.receive(5)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[cfg(windows)]
#[test]
fn diagnostics_recover_after_native_worker_termination() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/diagnostics/instar.toml"),
    )?;

    let document = uri(&directory.path().join("entry.luau"));
    let mut client = Client::with_arguments(directory.path(), &["--jobs", "1"])?;

    client.send(
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}),
    )?;

    assert!(client.receive(1).get("error").is_none());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":include_str!("fixtures/diagnostics/invalid.luau")}}}))?;
    let initial = client.diagnostics(&document, Some(1));

    assert!(
        initial
            .as_array()
            .expect("type findings")
            .iter()
            .any(|finding| finding["source"] == "Luau")
    );

    let terminated = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &format!(
            "$ErrorActionPreference = 'Stop'; $workers = @(Get-CimInstance Win32_Process -Filter 'ParentProcessId = {}' | Where-Object {{ $_.Name -eq 'instar.exe' -and $_.CommandLine -match 'worker analysis$' }}); if ($workers.Count -ne 1) {{ throw 'Expected one owned native worker' }}; $process = Get-Process -Id $workers[0].ProcessId; Stop-Process -InputObject $process -Force; $process.WaitForExit(); Write-Output $workers.Count",
            client.child.id()
        )])
        .output()?;

    assert!(
        terminated.status.success(),
        "{}",
        String::from_utf8_lossy(&terminated.stderr)
    );

    assert_eq!(String::from_utf8_lossy(&terminated.stdout).trim(), "1");
    let valid = include_str!("fixtures/diagnostics/valid.luau");
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":2},"contentChanges":[{"text":valid}]}}))?;
    assert_eq!(client.diagnostics(&document, Some(2)), json!([]));

    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document,"version":3},"contentChanges":[{"text":valid}]}}))?;
    assert_eq!(client.diagnostics(&document, Some(3)), json!([]));
    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"shutdown"}))?;
    assert_eq!(client.receive(2)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn diagnostics_preserve_mapped_contexts_and_follow_source_map_changes() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let sourcemap = directory.path().join("sourcemap.json");

    for (name, source) in [
        (
            "instar.toml",
            include_str!("../../project/tests/fixtures/checking/roblox/instar.toml"),
        ),
        (
            ".luaurc",
            include_str!("../../project/tests/fixtures/checking/imported/strict.json"),
        ),
        (
            "sourcemap.json",
            include_str!("../../project/tests/fixtures/checking/explicit/sourcemap.json"),
        ),
        (
            "number.luau",
            include_str!("fixtures/completion/dependency.luau"),
        ),
        (
            "string.luau",
            include_str!("../../project/tests/fixtures/checking/explicit/string.luau"),
        ),
    ] {
        fs::write(directory.path().join(name), source)?;
    }

    let source = include_str!("../../project/tests/fixtures/checking/explicit/caller.luau");
    let document = uri(&directory.path().join("caller.luau"));
    let mut client = Client::new(directory.path())?;
    client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"textDocument":{"diagnostic":{}}}}}))?;
    assert!(client.receive(1).get("error").is_none());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    contextual_assets(&mut client)?;

    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/diagnostic","params":{"textDocument":{"uri":document}}}))?;
    let report = client.receive(2);
    assert!(report.get("error").is_none(), "{report}");

    let findings = report["result"]["items"]
        .as_array()
        .expect("contextual diagnostics");

    assert!(
        findings.iter().any(|finding| finding["source"] == "Luau"
            && finding["severity"] == 1
            && finding["range"]["start"]["line"] == 1),
        "{report}"
    );

    let point = position(
        source,
        source.find("Target").expect("mapped target"),
        "utf-16",
    );

    client.send(&json!({"jsonrpc":"2.0","id":3,"method":"textDocument/definition","params":{"textDocument":{"uri":document},"position":point}}))?;
    let definitions = client.receive(3);
    let targets = definitions["result"].as_array().expect("mapped targets");
    assert_eq!(targets.len(), 2, "{definitions}");

    for name in ["number.luau", "string.luau"] {
        assert!(
            targets
                .iter()
                .any(|target| target["uri"] == uri(&directory.path().join(name)))
        );
    }

    fs::write(
        &sourcemap,
        include_str!("fixtures/diagnostics/sourcemap.json"),
    )?;

    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":uri(&sourcemap),"type":2}]}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":4,"method":"textDocument/diagnostic","params":{"textDocument":{"uri":document},"previousResultId":report["result"]["resultId"]}}))?;
    let updated = client.receive(4);
    assert_eq!(updated["result"]["kind"], "full", "{updated}");
    assert_eq!(updated["result"]["items"], json!([]));
    client.send(&json!({"jsonrpc":"2.0","id":5,"method":"textDocument/definition","params":{"textDocument":{"uri":document},"position":point}}))?;
    let definitions = client.receive(5);

    assert_eq!(
        definitions["result"]
            .as_array()
            .expect("unique target")
            .len(),
        1
    );

    assert_eq!(
        definitions["result"][0]["uri"],
        uri(&directory.path().join("number.luau"))
    );

    fs::write(
        &sourcemap,
        include_str!("../../project/tests/fixtures/checking/explicit/sourcemap.json"),
    )?;

    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":uri(&sourcemap),"type":2}]}}))?;
    client.send(&json!({"jsonrpc":"2.0","id":6,"method":"textDocument/diagnostic","params":{"textDocument":{"uri":document}}}))?;

    assert_eq!(
        client.receive(6)["result"]["items"],
        report["result"]["items"]
    );

    client.send(&json!({"jsonrpc":"2.0","id":7,"method":"shutdown"}))?;
    assert_eq!(client.receive(7)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    Ok(())
}

#[test]
fn diagnostic_selection_agrees_across_push_and_pulls_and_clears_live_results() -> io::Result<()> {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/selection/diagnostics.json"))?;

    let source = include_str!("fixtures/selection/diagnostics.luau");
    let directory = tempfile::tempdir()?;
    let configuration = directory.path().join("instar.toml");
    let document = uri(&directory.path().join("entry.luau"));
    fs::write(directory.path().join("entry.luau"), source)?;
    let mut expected = Vec::new();

    for pull in [false, true] {
        fs::write(
            &configuration,
            cases[0]["configuration"].as_str().expect("configuration"),
        )?;

        let mut client = Client::new(directory.path())?;

        let capabilities = if pull {
            json!({"textDocument":{"diagnostic":{}}})
        } else {
            json!({})
        };

        client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":capabilities,"workspaceFolders":[{"uri":uri(directory.path()),"name":"selection"}]}}))?;
        assert!(client.receive(1).get("error").is_none());
        client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
        client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;

        for (index, case) in cases.iter().enumerate() {
            if index > 0 {
                fs::write(
                    &configuration,
                    case["configuration"].as_str().expect("configuration"),
                )?;

                client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":uri(&configuration),"type":2}]}}))?;
            }

            let findings = if pull {
                client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/diagnostic","params":{"textDocument":{"uri":document}}}))?;
                let response = client.receive(2);
                assert!(response.get("error").is_none(), "{response}");

                response["result"]["items"].clone()
            } else {
                client.diagnostics(&document, Some(1))
            };

            let diagnostics = findings.as_array().expect("diagnostics");

            assert_eq!(
                diagnostics
                    .iter()
                    .any(|finding| finding["source"] == "Luau"),
                case["checking"].as_bool().expect("checking"),
                "{case}: {findings}"
            );

            assert_eq!(
                diagnostics
                    .iter()
                    .any(|finding| finding["code"] == "divide_by_zero"),
                case["linting"].as_bool().expect("linting"),
                "{case}: {findings}"
            );

            if pull {
                assert_eq!(findings, expected[index], "{case}");
                client.send(&json!({"jsonrpc":"2.0","id":3,"method":"workspace/diagnostic","params":{"previousResultIds":[]}}))?;
                let response = client.receive(3);

                let reports = response["result"]["items"]
                    .as_array()
                    .expect("workspace reports");

                assert_eq!(reports.len(), 1, "{response}");
                assert_eq!(reports[0]["uri"], document);
                assert_eq!(reports[0]["items"], findings);
            } else {
                expected.push(findings);
            }
        }

        client.send(&json!({"jsonrpc":"2.0","id":4,"method":"shutdown"}))?;
        assert_eq!(client.receive(4)["result"], Value::Null);
        client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
        assert!(client.wait()?.success());
    }

    directory.close()
}

#[test]
fn automatic_import_selection_refreshes_and_rejects_stale_edits() -> io::Result<()> {
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/selection/imports.json"))?;
    let directory = tempfile::tempdir()?;
    let configuration = directory.path().join("instar.toml");

    fs::write(
        &configuration,
        cases[0]["configuration"].as_str().expect("configuration"),
    )?;

    fs::write(
        directory.path().join("dependency.luau"),
        include_str!("fixtures/completion/dependency.luau"),
    )?;

    let source = include_str!("fixtures/completion/automatic.luau");
    let document = uri(&directory.path().join("entry.luau"));
    let mut client = Client::new(directory.path())?;
    client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"textDocument":{"completion":{"completionItem":{"resolveSupport":{"properties":["additionalTextEdits"]}}}}},"workspaceFolders":[{"uri":uri(directory.path()),"name":"selection"}]}}))?;
    assert!(client.receive(1).get("error").is_none());
    client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document,"languageId":"luau","version":1,"text":source}}}))?;
    let mut previous = None;

    for (index, case) in cases.iter().enumerate() {
        if index > 0 {
            fs::write(
                &configuration,
                case["configuration"].as_str().expect("configuration"),
            )?;

            client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":uri(&configuration),"type":2}]}}))?;

            if let Some(candidate) = previous.take() {
                client.send(&json!({"jsonrpc":"2.0","id":3,"method":"completionItem/resolve","params":candidate}))?;
                assert_eq!(client.receive(3)["error"]["code"], -32801);
            }
        }

        client.send(&json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":document},"position":{"line":1,"character":17}}}))?;
        let response = client.receive(2);
        assert!(response.get("error").is_none(), "{response}");

        let candidate = response["result"]["items"]
            .as_array()
            .expect("items")
            .iter()
            .find(|item| item["label"] == "dependency")
            .cloned();

        assert_eq!(
            candidate.is_some(),
            case["available"].as_bool().expect("availability"),
            "{case}: {response}"
        );

        previous = candidate;
    }

    client.send(&json!({"jsonrpc":"2.0","id":4,"method":"shutdown"}))?;
    assert_eq!(client.receive(4)["result"], Value::Null);
    client.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    assert!(client.wait()?.success());

    directory.close()
}
