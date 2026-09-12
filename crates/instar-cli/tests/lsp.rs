use std::{
    error::Error,
    fs,
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

use serde_json::{Value, json};
use tower_lsp_server::ls_types::Uri;

#[path = "../../instar-core/tests/support/roblox.rs"]
mod support;

type TestResult = Result<(), Box<dyn Error>>;

struct Client {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    progress: Vec<String>,
}

impl Drop for Client {
    fn drop(&mut self) {
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

impl Client {
    fn start() -> Result<Self, Box<dyn Error>> {
        Self::start_with(&json!({}))
    }

    fn start_with(capabilities: &Value) -> Result<Self, Box<dyn Error>> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_instar"))
            .arg("lsp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;

        let input = child.stdin.take().ok_or("missing input")?;
        let output = BufReader::new(child.stdout.take().ok_or("missing output")?);

        let mut client = Self {
            child,
            input,
            output,
            progress: Vec::new(),
        };

        client.send(
            &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":capabilities}}),
        )?;

        let initialized = client.response(1)?;

        assert_eq!(
            initialized["result"]["capabilities"]["textDocumentSync"]["change"],
            2
        );

        assert_eq!(
            initialized["result"]["capabilities"]["documentFormattingProvider"],
            true
        );

        assert!(initialized["result"]["capabilities"]["documentLinkProvider"].is_object());
        client.send(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;

        Ok(client)
    }

    fn send(&mut self, message: &Value) -> TestResult {
        let content = serde_json::to_vec(message)?;
        write!(self.input, "Content-Length: {}\r\n\r\n", content.len())?;
        self.input.write_all(&content)?;
        self.input.flush()?;

        Ok(())
    }

    fn receive(&mut self) -> Result<Value, Box<dyn Error>> {
        let mut length = None;

        loop {
            let mut line = String::new();

            if self.output.read_line(&mut line)? == 0 {
                return Err("server closed its output".into());
            }

            if line == "\r\n" {
                break;
            }

            if let Some(value) = line.strip_prefix("Content-Length: ") {
                length = Some(value.trim().parse::<usize>()?);
            }
        }

        let mut content = vec![0; length.ok_or("missing content length")?];
        self.output.read_exact(&mut content)?;

        Ok(serde_json::from_slice(&content)?)
    }

    fn response(&mut self, identifier: u64) -> Result<Value, Box<dyn Error>> {
        loop {
            let message = self.receive()?;

            if message["method"] == "window/workDoneProgress/create" {
                self.send(&json!({"jsonrpc":"2.0","id":message["id"],"result":null}))?;
            }

            if message["method"] == "$/progress"
                && let Some(kind) = message["params"]["value"]["kind"].as_str()
            {
                self.progress.push(kind.into());
            }

            if message["id"] == identifier && message.get("method").is_none() {
                return Ok(message);
            }

            assert_ne!(message["method"], "window/logMessage", "{message}");
        }
    }

    fn diagnostics(&mut self, uri: &str) -> Result<Value, Box<dyn Error>> {
        loop {
            let message = self.receive()?;
            assert_ne!(message["method"], "window/logMessage", "{message}");

            if message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == uri
            {
                return Ok(message["params"].clone());
            }
        }
    }

    fn open(&mut self, uri: &str, text: &str) -> TestResult {
        self.send(
            &json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                "textDocument":{"uri":uri,"languageId":"luau","version":1,"text":text}
            }}),
        )
    }

    fn change(&mut self, uri: &str, version: i32, text: &str) -> TestResult {
        self.send(
            &json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{
                "textDocument":{"uri":uri,"version":version},"contentChanges":[{"text":text}]
            }}),
        )
    }

    fn shutdown(&mut self) -> TestResult {
        self.send(&json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}))?;
        assert!(self.response(3)?["result"].is_null());
        self.send(&json!({"jsonrpc":"2.0","method":"exit"}))?;
        assert!(self.child.wait()?.success());

        Ok(())
    }

    fn query(
        &mut self,
        uri: &str,
        method: &str,
        line: u32,
        character: u32,
    ) -> Result<Value, Box<dyn Error>> {
        self.send(&json!({"jsonrpc":"2.0","id":20,"method":method,"params":{"textDocument":{"uri":uri},"position":{"line":line,"character":character},"context":{"includeDeclaration":true,"triggerKind":1,"isRetrigger":false}}}))?;
        let response = self.response(20)?;
        assert!(response.get("error").is_none(), "{response}");

        Ok(response["result"].clone())
    }

    fn request(&mut self, method: &str, parameters: Value) -> Result<Value, Box<dyn Error>> {
        let mut message = json!({"jsonrpc":"2.0","id":20,"method":method});
        message["params"] = parameters;
        self.send(&message)?;
        let response = self.response(20)?;
        assert!(response.get("error").is_none(), "{response}");

        Ok(response["result"].clone())
    }

    fn format(&mut self, uri: &str) -> Result<Value, Box<dyn Error>> {
        self.send(
            &json!({"jsonrpc":"2.0","id":2,"method":"textDocument/formatting","params":{
                "textDocument":{"uri":uri},"options":{"tabSize":4,"insertSpaces":false}
            }}),
        )?;

        self.response(2)
    }
}

fn has_errors(publication: &Value) -> bool {
    publication["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .any(|diagnostic| diagnostic["severity"] == 1)
}

#[test]
fn stdio_preserves_snapshots_positions_dependencies_and_shutdown() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    let path = root.join("café.luau");

    let uri = Uri::from_file_path(&path)
        .ok_or("document URI")?
        .to_string();

    fs::write(&path, "return 1\n")?;
    let mut client = Client::start()?;

    let invalid =
        "--!strict\nlocal marker = '😀'; local value: number = 'wrong'\nreturn marker, value\n";

    client.open(&uri, invalid)?;
    let publication = client.diagnostics(&uri)?;
    assert!(has_errors(&publication), "{publication}");
    assert_eq!(publication["version"], 1);

    let diagnostic = publication["diagnostics"]
        .as_array()
        .ok_or("diagnostics")?
        .iter()
        .find(|diagnostic| diagnostic["severity"] == 1)
        .ok_or("type error")?;

    let line = invalid.lines().nth(1).ok_or("source line")?;

    let column = line[..line.find("'wrong'").ok_or("expression")?]
        .encode_utf16()
        .count();

    assert_eq!(
        diagnostic["range"]["start"],
        json!({"line":1,"character":column})
    );

    assert!(client.format(&uri)?["result"].is_array());

    let valid = "--!strict\nlocal value:number=1\nreturn value";
    client.change(&uri, 2, valid)?;
    assert!(!has_errors(&client.diagnostics(&uri)?));
    let formatted = client.format(&uri)?;

    let replacement = formatted["result"][0]["newText"]
        .as_str()
        .ok_or("formatting edit")?;

    assert_eq!(
        replacement,
        "--!strict\nlocal value: number = 1\nreturn value\n"
    );

    assert_eq!(
        formatted["result"][0]["range"]["end"],
        json!({"line":2,"character":12})
    );

    assert_eq!(fs::read_to_string(&path)?, "return 1\n");
    client.change(&uri, 3, replacement)?;
    assert!(!has_errors(&client.diagnostics(&uri)?));
    assert!(client.format(&uri)?["result"].is_null());

    client.change(&uri, 2, invalid)?;
    let stale = client.receive()?;
    assert_eq!(stale["method"], "window/logMessage");

    assert!(
        stale["params"]["message"]
            .as_str()
            .ok_or("version error")?
            .contains("version must increase")
    );

    assert!(client.format(&uri)?["result"].is_null());
    assert_eq!(client.format("untitled:sample")?["error"]["code"], -32602);

    client.change(&uri, 4, "local = =")?;
    assert!(has_errors(&client.diagnostics(&uri)?));
    assert!(client.format(&uri)?["error"].is_object());
    client.change(&uri, 5, replacement)?;
    assert!(!has_errors(&client.diagnostics(&uri)?));

    let dependency = root.join("dependency.luau");

    let dependency_uri = Uri::from_file_path(&dependency)
        .ok_or("dependency URI")?
        .to_string();

    fs::write(&dependency, "return 1")?;

    let entry_uri = Uri::from_file_path(root.join("entry.luau"))
        .ok_or("entry URI")?
        .to_string();

    client.open(
        &entry_uri,
        "--!strict\nlocal value: number = require('./dependency')\nreturn value",
    )?;

    assert!(!has_errors(&client.diagnostics(&entry_uri)?));
    client.open(&dependency_uri, "return 'wrong'")?;
    assert!(has_errors(&client.diagnostics(&entry_uri)?));
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":dependency_uri}}}))?;
    assert!(!has_errors(&client.diagnostics(&entry_uri)?));

    fs::write(&dependency, "return 'wrong'")?;
    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":dependency_uri,"type":2}]}}))?;
    assert!(has_errors(&client.diagnostics(&entry_uri)?));
    fs::write(&dependency, "return 1")?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didSave","params":{"textDocument":{"uri":entry_uri}}}))?;
    assert!(!has_errors(&client.diagnostics(&entry_uri)?));

    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":uri}}}))?;
    assert_eq!(client.diagnostics(&uri)?["diagnostics"], json!([]));
    client.shutdown()?;

    Ok(())
}

#[test]
fn configuration_changes_refresh_diagnostics_and_formatting() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    let configuration = root.join("instar.toml");
    fs::write(&configuration, "[format]\nindentation.style = 'spaces'\n")?;

    let uri = Uri::from_file_path(root.join("main.luau"))
        .ok_or("document URI")?
        .to_string();

    let mut client = Client::start()?;
    client.open(&uri, "do print('hello') end")?;
    assert!(!has_errors(&client.diagnostics(&uri)?));

    assert_eq!(
        client.format(&uri)?["result"][0]["newText"],
        "do\n    print(\"hello\")\nend\n"
    );

    fs::write(&configuration, "[format")?;
    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeConfiguration","params":{"settings":{}}}))?;
    assert!(has_errors(&client.diagnostics(&uri)?));
    assert!(client.format(&uri)?["error"].is_object());

    fs::write(
        &configuration,
        "[format]\nindentation.style = 'spaces'\nindentation.width = 2\n",
    )?;

    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeConfiguration","params":{"settings":{}}}))?;
    assert!(!has_errors(&client.diagnostics(&uri)?));

    assert_eq!(
        client.format(&uri)?["result"][0]["newText"],
        "do\n  print(\"hello\")\nend\n"
    );

    client.shutdown()
}

#[test]
fn editor_queries_use_inferred_types_and_bindings() -> TestResult {
    let directory = tempfile::tempdir()?;

    let uri = Uri::from_file_path(directory.path().join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let mut client = Client::start()?;
    client.open(&uri, "--!strict\nlocal value: number = 1\nlocal function increase(amount: number): number\nreturn amount + value\nend\nreturn increase(value)")?;
    assert!(!has_errors(&client.diagnostics(&uri)?));
    let hover = client.query(&uri, "textDocument/hover", 5, 17)?;

    assert!(
        hover["contents"]["value"]
            .as_str()
            .ok_or("hover")?
            .contains("number"),
        "{hover}"
    );

    for method in ["textDocument/definition", "textDocument/declaration"] {
        let target = client.query(&uri, method, 5, 17)?;
        assert_eq!(target["uri"], uri);
        assert_eq!(target["range"]["start"], json!({"line":1,"character":6}));
    }

    let references = client.query(&uri, "textDocument/references", 5, 17)?;

    assert_eq!(
        references.as_array().ok_or("references")?.len(),
        3,
        "{references}"
    );

    let completions = client.query(&uri, "textDocument/completion", 5, 16)?;

    assert!(
        completions
            .as_array()
            .ok_or("completions")?
            .iter()
            .any(|item| item["label"] == "value"),
        "{completions}"
    );

    let signature = client.query(&uri, "textDocument/signatureHelp", 5, 17)?;

    assert!(
        signature["signatures"][0]["label"]
            .as_str()
            .ok_or("signature")?
            .contains("number")
    );

    let symbols = client.query(&uri, "textDocument/documentSymbol", 0, 0)?;

    assert!(
        symbols
            .as_array()
            .ok_or("symbols")?
            .iter()
            .any(|symbol| symbol["name"] == "increase")
    );

    let tokens = client.query(&uri, "textDocument/semanticTokens/full", 0, 0)?;
    assert_ne!(tokens["data"].as_array().ok_or("tokens")?.len(), 0);

    client.shutdown()
}

#[test]
fn workspace_navigation_and_rename_include_closed_dependencies() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    let root_uri = Uri::from_file_path(root).ok_or("root URI")?.to_string();
    let dependency = root.join("dependency.luau");
    fs::write(&dependency, "return {Value = 1}")?;

    let dependency_uri = Uri::from_file_path(&dependency)
        .ok_or("dependency URI")?
        .to_string();

    let uri = Uri::from_file_path(root.join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let mut client = Client::start()?;
    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWorkspaceFolders","params":{"event":{"added":[{"uri":root_uri,"name":"workspace"}],"removed":[]}}}))?;

    client.open(
        &uri,
        "--!strict\nlocal dependency = require('./dependency')\nreturn dependency.Value",
    )?;

    assert!(!has_errors(&client.diagnostics(&uri)?));
    let target = client.query(&uri, "textDocument/definition", 2, 20)?;
    assert_eq!(target["uri"], dependency_uri, "{target}");
    assert_eq!(target["range"]["start"], json!({"line":0,"character":8}));
    let target = client.query(&uri, "textDocument/definition", 1, 32)?;
    assert_eq!(target["uri"], dependency_uri);
    let references = client.query(&uri, "textDocument/references", 2, 20)?;

    assert_eq!(
        references.as_array().ok_or("references")?.len(),
        2,
        "{references}"
    );

    assert!(
        client
            .query(&uri, "textDocument/prepareRename", 2, 20)?
            .is_object()
    );

    client.send(&json!({"jsonrpc":"2.0","id":20,"method":"textDocument/rename","params":{"textDocument":{"uri":uri},"position":{"line":2,"character":20},"newName":"Amount"}}))?;
    let renamed = client.response(20)?;

    assert_eq!(
        renamed["result"]["documentChanges"]
            .as_array()
            .ok_or_else(|| format!("rename: {renamed}"))?
            .len(),
        2,
        "{renamed}"
    );

    assert_eq!(fs::read_to_string(&dependency)?, "return {Value = 1}");

    client.send(
        &json!({"jsonrpc":"2.0","id":20,"method":"workspace/symbol","params":{"query":"Value"}}),
    )?;

    let symbols = client.response(20)?;

    assert!(
        symbols["result"]
            .as_array()
            .ok_or("workspace symbols")?
            .iter()
            .any(|symbol| symbol["name"] == "Value")
    );

    client.shutdown()
}

#[test]
fn queued_edits_preserve_each_changed_dependency() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();

    let uri = Uri::from_file_path(root.join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let left = Uri::from_file_path(root.join("left.luau"))
        .ok_or("URI")?
        .to_string();

    let right = Uri::from_file_path(root.join("right.luau"))
        .ok_or("URI")?
        .to_string();

    let mut client = Client::start()?;
    client.open(&left, "return 1")?;
    client.diagnostics(&left)?;
    client.open(&right, "return 1")?;
    client.diagnostics(&right)?;
    client.open(&uri, "--!strict\nlocal left: number = require('./left')\nlocal right: number = require('./right')\nreturn left, right")?;
    assert!(!has_errors(&client.diagnostics(&uri)?));
    client.change(&left, 2, "return 'wrong'")?;
    client.change(&right, 2, "return 'wrong'")?;
    client.send(&json!({"jsonrpc":"2.0","id":20,"method":"textDocument/hover","params":{"textDocument":{"uri":uri},"position":{"line":3,"character":8}}}))?;
    let mut errors = 0;
    let mut answered = false;
    let mut published = false;

    while !answered || !published {
        let message = client.receive()?;
        assert_ne!(message["method"], "window/logMessage", "{message}");
        answered |= message["id"] == 20;

        published |= message["method"] == "textDocument/publishDiagnostics"
            && message["params"]["uri"] == right
            && message["params"]["version"] == 2;

        if message["method"] == "textDocument/publishDiagnostics" && message["params"]["uri"] == uri
        {
            errors = message["params"]["diagnostics"]
                .as_array()
                .ok_or("diagnostics")?
                .iter()
                .filter(|diagnostic| diagnostic["severity"] == 1)
                .count();
        }
    }

    assert_eq!(errors, 2);

    client.shutdown()
}

#[test]
fn file_renames_preserve_unsaved_documents() -> TestResult {
    let directory = tempfile::tempdir()?;

    let original = Uri::from_file_path(directory.path().join("original.luau"))
        .ok_or("URI")?
        .to_string();

    let renamed = Uri::from_file_path(directory.path().join("renamed.luau"))
        .ok_or("URI")?
        .to_string();

    let mut client = Client::start()?;

    client.open(
        &original,
        "--!strict\nlocal value: number = 'wrong'\nreturn value",
    )?;

    assert!(has_errors(&client.diagnostics(&original)?));
    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didRenameFiles","params":{"files":[{"oldUri":original,"newUri":renamed}]}}))?;
    assert_eq!(client.diagnostics(&original)?["diagnostics"], json!([]));
    assert!(has_errors(&client.diagnostics(&renamed)?));
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":original}}}))?;
    let hover = client.query(&renamed, "textDocument/hover", 2, 8)?;

    assert!(
        hover["contents"]["value"]
            .as_str()
            .ok_or("hover")?
            .contains("number")
    );

    client.shutdown()
}

#[test]
fn type_navigation_and_related_diagnostics_keep_ranges() -> TestResult {
    let directory = tempfile::tempdir()?;

    let uri = Uri::from_file_path(directory.path().join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let mut client = Client::start()?;

    client.open(
        &uri,
        "--!strict\ntype Item = {value: number}\nlocal item: Item = {value = 1}\nreturn item",
    )?;

    assert!(!has_errors(&client.diagnostics(&uri)?));
    let target = client.query(&uri, "textDocument/typeDefinition", 3, 9)?;
    assert_eq!(target["uri"], uri, "{target}");
    assert_eq!(target["range"]["start"]["line"], 1, "{target}");

    client.change(
        &uri,
        2,
        "--!strict\ntype Item = number\ntype Item = string\nreturn 1",
    )?;

    let publication = client.diagnostics(&uri)?;
    assert!(has_errors(&publication));

    assert!(
        publication["diagnostics"]
            .as_array()
            .ok_or("diagnostics")?
            .iter()
            .any(|diagnostic| diagnostic["relatedInformation"]
                .as_array()
                .is_some_and(|related| !related.is_empty())),
        "{publication}"
    );

    client.shutdown()
}

#[test]
fn requests_cancel_while_analysis_is_running() -> TestResult {
    let directory = tempfile::tempdir()?;
    fs::write(directory.path().join("instar.toml"), "[roblox]\n")?;
    support::configure(directory.path())?;

    let uri = Uri::from_file_path(directory.path().join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let mut client = Client::start()?;
    client.open(&uri, "local value = 1\nreturn value")?;
    client.send(&json!({"jsonrpc":"2.0","id":20,"method":"textDocument/hover","params":{"textDocument":{"uri":uri},"position":{"line":1,"character":8}}}))?;
    client.send(&json!({"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":20}}))?;
    let mut cancelled = false;
    let mut diagnosed = false;

    while !cancelled || !diagnosed {
        let message = client.receive()?;

        if message["id"] == 20 {
            assert_eq!(message["error"]["code"], -32800, "{message}");
            cancelled = true;
        }

        if message["method"] == "textDocument/publishDiagnostics" {
            diagnosed = true;
        }
    }

    assert!(client.query(&uri, "textDocument/hover", 1, 8)?.is_object());

    client.shutdown()
}

#[test]
fn registrations_documentation_actions_and_rename_validation() -> TestResult {
    let directory = tempfile::tempdir()?;

    let uri = Uri::from_file_path(directory.path().join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let mut client = Client::start_with(
        &json!({"workspace":{"didChangeWatchedFiles":{"dynamicRegistration":true},"didChangeConfiguration":{"dynamicRegistration":true}}}),
    )?;

    let registration = client.receive()?;
    assert_eq!(registration["method"], "client/registerCapability");

    let registrations = registration["params"]["registrations"]
        .as_array()
        .ok_or("registrations")?;

    assert!(
        registrations
            .iter()
            .any(|registration| registration["method"] == "workspace/didChangeWatchedFiles")
    );

    assert!(
        registrations
            .iter()
            .any(|registration| registration["method"] == "workspace/didChangeConfiguration")
    );

    client.send(&json!({"jsonrpc":"2.0","id":registration["id"],"result":null}))?;
    client.open(&uri, "--- Adds a number.\nlocal function increase(amount:number):number return amount+1 end\nlocal value=increase(1)\nreturn value")?;
    assert!(!has_errors(&client.diagnostics(&uri)?));
    let hover = client.query(&uri, "textDocument/hover", 2, 17)?;

    assert!(
        hover["contents"]["value"]
            .as_str()
            .ok_or("hover")?
            .contains("Adds a number."),
        "{hover}"
    );

    let signature = client.query(&uri, "textDocument/signatureHelp", 2, 22)?;

    assert_eq!(
        signature["signatures"][0]["parameters"][0]["label"],
        "amount: number"
    );

    client.send(&json!({"jsonrpc":"2.0","id":20,"method":"textDocument/codeAction","params":{"textDocument":{"uri":uri},"range":{"start":{"line":0,"character":0},"end":{"line":3,"character":12}},"context":{"diagnostics":[],"only":["source.format"]}}}))?;
    let actions = client.response(20)?;

    assert!(
        actions["result"][0]["edit"]["changes"][&uri].is_array(),
        "{actions}"
    );

    for name in ["return", "not valid", "increase"] {
        client.send(&json!({"jsonrpc":"2.0","id":20,"method":"textDocument/rename","params":{"textDocument":{"uri":uri},"position":{"line":3,"character":8},"newName":name}}))?;
        assert_eq!(client.response(20)?["error"]["code"], -32602);
    }

    client.shutdown()
}

#[test]
fn roblox_navigation_and_edits_share_physical_source_identity() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();

    fs::write(
        root.join("instar.toml"),
        "[roblox]\nsourcemap = 'sourcemap.json'\n",
    )?;

    fs::write(
        root.join("sourcemap.json"),
        r#"{"name":"Library","className":"Folder","children":[{"name":"Dependency","className":"ModuleScript","filePaths":["dependency.luau"]},{"name":"Main","className":"ModuleScript","filePaths":["main.luau"]}]}"#,
    )?;

    fs::write(root.join("dependency.luau"), "return {Value = 1}")?;
    support::configure(root)?;

    let uri = Uri::from_file_path(root.join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let dependency = Uri::from_file_path(root.join("dependency.luau"))
        .ok_or("URI")?
        .to_string();

    let mut client = Client::start()?;
    client.open(&uri, "--!strict\nlocal dependency = require(script.Parent.Dependency)\nlocal value: number = dependency.Value\nreturn value")?;
    assert!(!has_errors(&client.diagnostics(&uri)?));
    let target = client.query(&uri, "textDocument/definition", 2, 35)?;
    assert_eq!(target["uri"], dependency, "{target}");
    let links = client.query(&uri, "textDocument/documentLink", 0, 0)?;
    assert_eq!(links.as_array().ok_or("links")?.len(), 1, "{links}");
    assert_eq!(links[0]["target"], dependency);

    assert_eq!(
        links[0]["range"],
        json!({"start":{"line":1,"character":27},"end":{"line":1,"character":51}})
    );

    client.open(&dependency, "return {Value = 1}")?;
    client.diagnostics(&dependency)?;
    let references = client.query(&dependency, "textDocument/references", 0, 10)?;

    assert_eq!(
        references.as_array().ok_or("references")?.len(),
        2,
        "{references}"
    );

    client.change(&dependency, 2, "return {Value = 'wrong'}")?;
    assert!(has_errors(&client.diagnostics(&uri)?));
    client.change(&dependency, 3, "return {Value = 1}")?;
    assert!(!has_errors(&client.diagnostics(&uri)?));

    client.shutdown()
}

#[test]
fn imported_type_navigation_and_references_resolve_aliases() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();

    fs::write(
        root.join("dependency.luau"),
        "export type Item = number\nreturn {}",
    )?;

    let uri = Uri::from_file_path(root.join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let dependency = Uri::from_file_path(root.join("dependency.luau"))
        .ok_or("URI")?
        .to_string();

    let mut client = Client::start()?;
    client.open(&uri, "--!strict\nlocal dependency = require('./dependency')\nlocal value: dependency.Item = 1\nreturn value")?;
    assert!(!has_errors(&client.diagnostics(&uri)?));
    let target = client.query(&uri, "textDocument/definition", 2, 26)?;
    assert_eq!(target["uri"], dependency, "{target}");
    assert_eq!(target["range"]["start"], json!({"line":0,"character":12}));
    let prefix = client.query(&uri, "textDocument/definition", 2, 16)?;
    assert_eq!(prefix["range"]["start"], json!({"line":1,"character":6}));
    let references = client.query(&dependency, "textDocument/references", 0, 14)?;

    assert_eq!(
        references.as_array().ok_or("references")?.len(),
        2,
        "{references}"
    );

    client.shutdown()
}

#[test]
fn hints_follow_configuration_and_completion_resolves_details() -> TestResult {
    let directory = tempfile::tempdir()?;

    let uri = Uri::from_file_path(directory.path().join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let mut client = Client::start()?;
    client.open(&uri, "local function increase(amount: number)\nreturn amount + 1\nend\nlocal result = increase(1)\nreturn result")?;
    client.diagnostics(&uri)?;
    let parameters = json!({"textDocument":{"uri":uri},"range":{"start":{"line":0,"character":0},"end":{"line":4,"character":13}}});

    assert_eq!(
        client.request("textDocument/inlayHint", parameters.clone())?,
        json!([])
    );

    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeConfiguration","params":{"settings":{"inlayHints":{"variableTypes":true,"functionReturnTypes":true,"parameterNames":"all"}}}}))?;
    client.diagnostics(&uri)?;
    let hints = client.request("textDocument/inlayHint", parameters)?;

    assert!(
        hints
            .as_array()
            .ok_or("hints")?
            .iter()
            .any(|hint| hint["label"] == ": number"),
        "{hints}"
    );

    assert!(
        hints
            .as_array()
            .ok_or("hints")?
            .iter()
            .any(|hint| hint["label"] == "amount:"),
        "{hints}"
    );

    let completions = client.query(&uri, "textDocument/completion", 4, 13)?;

    let item = completions
        .as_array()
        .ok_or("completions")?
        .iter()
        .find(|item| item["label"] == "result")
        .ok_or("completion")?;

    assert!(item.get("detail").is_none(), "{item}");
    let resolved = client.request("completionItem/resolve", item.clone())?;
    assert_eq!(resolved["detail"], "number");

    client.shutdown()
}

#[test]
fn range_formatting_preserves_unselected_text_and_pull_checks_closed_files() -> TestResult {
    let directory = tempfile::tempdir()?;

    let uri = Uri::from_file_path(directory.path().join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let mut client = Client::start()?;

    client.open(
        &uri,
        "local outside=1\nlocal   selected=2\nreturn outside+selected",
    )?;

    client.diagnostics(&uri)?;
    let range = json!({"start":{"line":1,"character":0},"end":{"line":1,"character":18}});
    let edits = client.request("textDocument/rangeFormatting", json!({"textDocument":{"uri":uri},"range":range,"options":{"tabSize":4,"insertSpaces":true}}))?;
    assert_eq!(edits[0]["range"], range);
    assert_eq!(edits[0]["newText"], "local selected = 2");

    client.change(
        &uri,
        2,
        &format!(
            "local outside=1\n{}\nreturn outside+selected",
            edits[0]["newText"].as_str().ok_or("edit")?
        ),
    )?;

    assert!(!has_errors(&client.diagnostics(&uri)?));

    fs::write(
        directory.path().join("closed.luau"),
        "--!strict\nlocal value: number = 'wrong'\nreturn value",
    )?;

    let closed = Uri::from_file_path(directory.path().join("closed.luau"))
        .ok_or("URI")?
        .to_string();

    let report = client.request(
        "textDocument/diagnostic",
        json!({"textDocument":{"uri":closed}}),
    )?;

    assert_eq!(report["kind"], "full");

    let unchanged = client.request(
        "textDocument/diagnostic",
        json!({"textDocument":{"uri":closed},"previousResultId":report["resultId"]}),
    )?;

    assert_eq!(
        unchanged["kind"], "unchanged",
        "first: {report}; next: {unchanged}"
    );

    assert!(
        report["items"]
            .as_array()
            .ok_or("diagnostics")?
            .iter()
            .any(|item| item["severity"] == 1)
    );

    client.shutdown()
}

#[test]
fn range_formatting_handles_nested_statements_and_expressions() -> TestResult {
    let directory = tempfile::tempdir()?;

    let uri = Uri::from_file_path(directory.path().join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let mut client = Client::start()?;

    client.open(
        &uri,
        "local function run()\n    local selected = 1+2\n    return selected\nend\nreturn run",
    )?;

    client.diagnostics(&uri)?;

    for (start, expected) in [(0, "    local selected = 1 + 2"), (21, "1 + 2")] {
        let edits = client.request("textDocument/rangeFormatting", json!({"textDocument":{"uri":uri},"range":{"start":{"line":1,"character":start},"end":{"line":1,"character":24}},"options":{"tabSize":4,"insertSpaces":true}}))?;
        assert_eq!(edits[0]["newText"], expected, "{edits}");
    }

    client.shutdown()
}

#[test]
fn file_rename_edits_preserve_import_targets() -> TestResult {
    let directory = tempfile::tempdir()?;

    let root = Uri::from_file_path(directory.path())
        .ok_or("URI")?
        .to_string();

    let uri = Uri::from_file_path(directory.path().join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let old = Uri::from_file_path(directory.path().join("dependency.luau"))
        .ok_or("URI")?
        .to_string();

    let new = Uri::from_file_path(directory.path().join("renamed.luau"))
        .ok_or("URI")?
        .to_string();

    fs::write(directory.path().join("dependency.luau"), "return 1")?;
    let mut client = Client::start()?;
    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWorkspaceFolders","params":{"event":{"added":[{"uri":root,"name":"workspace"}],"removed":[]}}}))?;

    client.open(
        &uri,
        "local dependency = require('./dependency')\nreturn dependency",
    )?;

    client.diagnostics(&uri)?;
    let files = json!([{"oldUri":old,"newUri":new}]);
    let edit = client.request("workspace/willRenameFiles", json!({"files":files}))?;
    let document = &edit["documentChanges"][0];
    assert_eq!(document["textDocument"]["uri"], uri);
    assert_eq!(document["textDocument"]["version"], 1);
    assert_eq!(document["edits"][0]["newText"], "'./renamed'");

    fs::rename(
        directory.path().join("dependency.luau"),
        directory.path().join("renamed.luau"),
    )?;

    client.change(
        &uri,
        2,
        "local dependency = require('./renamed')\nreturn dependency",
    )?;

    client.send(
        &json!({"jsonrpc":"2.0","method":"workspace/didRenameFiles","params":{"files":files}}),
    )?;

    assert!(!has_errors(&client.diagnostics(&uri)?));
    let links = client.query(&uri, "textDocument/documentLink", 0, 0)?;
    assert_eq!(links[0]["target"], new);

    client.shutdown()
}

#[test]
fn implementation_navigation_follows_explicit_contract_returns() -> TestResult {
    let directory = tempfile::tempdir()?;

    let root = Uri::from_file_path(directory.path())
        .ok_or("URI")?
        .to_string();

    let uri = Uri::from_file_path(directory.path().join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let dependency = Uri::from_file_path(directory.path().join("context.luau"))
        .ok_or("URI")?
        .to_string();

    fs::write(
        directory.path().join("context.luau"),
        "export type Context = { read get_value: () -> number }\nlocal function create(): Context\nlocal function get_value() return 1 end\nreturn table.freeze({ get_value = get_value })\nend\nreturn create",
    )?;

    let mut client = Client::start()?;
    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWorkspaceFolders","params":{"event":{"added":[{"uri":root,"name":"workspace"}],"removed":[]}}}))?;
    client.open(&uri, "local shared = require('./context')\nlocal function use(context: shared.Context)\nreturn context.get_value()\nend\nreturn use")?;
    client.diagnostics(&uri)?;
    let implementations = client.query(&uri, "textDocument/implementation", 2, 19)?;

    assert!(
        implementations
            .as_array()
            .ok_or("implementations")?
            .iter()
            .any(|item| item["uri"] == dependency && item["range"]["start"]["line"] == 2),
        "{implementations}"
    );

    let implementations = client.query(&uri, "textDocument/implementation", 1, 38)?;

    assert!(
        implementations
            .as_array()
            .ok_or("implementations")?
            .iter()
            .any(|item| item["uri"] == dependency && item["range"]["start"]["line"] == 3),
        "{implementations}"
    );

    client.shutdown()
}

#[test]
fn extraction_returns_versioned_edits_and_indexing_reports_progress() -> TestResult {
    let directory = tempfile::tempdir()?;

    let uri = Uri::from_file_path(directory.path().join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let mut client = Client::start_with(&json!({"window":{"workDoneProgress":true}}))?;
    client.open(&uri, "local result = 1 + 2\nreturn result")?;
    client.diagnostics(&uri)?;
    let actions = client.request("textDocument/codeAction", json!({"textDocument":{"uri":uri},"range":{"start":{"line":0,"character":15},"end":{"line":0,"character":20}},"context":{"diagnostics":[],"only":["refactor.extract"]}}))?;
    assert_eq!(actions.as_array().ok_or("actions")?.len(), 1, "{actions}");
    let document = &actions[0]["edit"]["documentChanges"][0];
    assert_eq!(document["textDocument"]["version"], 1);

    let replacement = document["edits"][0]["newText"]
        .as_str()
        .ok_or("replacement")?;

    let insertion = document["edits"][1]["newText"]
        .as_str()
        .ok_or("insertion")?;

    client.change(
        &uri,
        2,
        &format!("{insertion}local result = {replacement}\nreturn result"),
    )?;

    assert!(!has_errors(&client.diagnostics(&uri)?));
    client.request("workspace/symbol", json!({"query":"result"}))?;
    assert!(client.progress.iter().any(|kind| kind == "begin"));
    assert!(client.progress.iter().any(|kind| kind == "end"));

    client.shutdown()
}

#[test]
fn annotated_properties_navigate_to_imported_declarations() -> TestResult {
    let directory = tempfile::tempdir()?;

    let dependency = Uri::from_file_path(directory.path().join("context.luau"))
        .ok_or("URI")?
        .to_string();

    let identifier = Uri::from_file_path(directory.path().join("spectate.luau"))
        .ok_or("URI")?
        .to_string();

    fs::write(
        directory.path().join("context.luau"),
        "export type Context = {\n    read get_player_index: () -> number,\n    value: number,\n}\nreturn {}",
    )?;

    let mut client = Client::start()?;
    client.open(&identifier, "--!strict\nconst shared = require('./context')\nconst function create(context: shared.Context)\n    const index = context.get_player_index()\n    return index + context.value\nend\nreturn create")?;
    assert!(!has_errors(&client.diagnostics(&identifier)?));

    for method in ["textDocument/definition", "textDocument/declaration"] {
        let target = client.query(&identifier, method, 3, 30)?;
        assert_eq!(target["uri"], dependency, "{target}");

        assert_eq!(
            target["range"],
            json!({"start":{"line":1,"character":9},"end":{"line":1,"character":25}})
        );

        let target = client.query(&identifier, method, 4, 29)?;
        assert_eq!(target["uri"], dependency, "{target}");

        assert_eq!(
            target["range"],
            json!({"start":{"line":2,"character":4},"end":{"line":2,"character":9}})
        );
    }

    client.shutdown()
}

#[test]
fn local_rename_does_not_read_unrelated_projects() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::create_dir(root.join("unrelated"))?;
    fs::write(root.join("unrelated/instar.toml"), "[")?;
    fs::write(root.join("unrelated/other.luau"), "return 1")?;
    let workspace = Uri::from_file_path(root).ok_or("workspace")?.to_string();

    let identifier = Uri::from_file_path(root.join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let mut client = Client::start()?;
    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWorkspaceFolders","params":{"event":{"added":[{"uri":workspace,"name":"workspace"}],"removed":[]}}}))?;
    client.open(&identifier, "local value = 1\nreturn value")?;
    assert!(!has_errors(&client.diagnostics(&identifier)?));

    for method in [
        "textDocument/prepareRename",
        "textDocument/hover",
        "textDocument/references",
    ] {
        let started = std::time::Instant::now();
        let response = client.query(&identifier, method, 1, 8)?;
        eprintln!("{method}: {:?}", started.elapsed());
        assert!(!response.is_null(), "{response}");
    }

    let started = std::time::Instant::now();
    client.send(&json!({"jsonrpc":"2.0","id":20,"method":"textDocument/rename","params":{"textDocument":{"uri":identifier},"position":{"line":1,"character":8},"newName":"amount"}}))?;
    let response = client.response(20)?;
    eprintln!("textDocument/rename: {:?}", started.elapsed());

    assert_eq!(
        response["result"]["documentChanges"][0]["edits"]
            .as_array()
            .ok_or("rename")?
            .len(),
        2,
        "{response}"
    );

    client.shutdown()
}

#[test]
fn hover_preserves_declarations_and_qualified_function_types() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();

    fs::write(
        root.join("ui.luau"),
        "export type Context = {scope: number}\nreturn {}",
    )?;

    fs::write(
        root.join("waypoints.luau"),
        "export type Settings = {enabled: boolean}\nreturn {}",
    )?;

    let identifier = Uri::from_file_path(root.join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let source = "local ui = require('./ui')\nlocal waypoints = require('./waypoints')\n--- Builds camera settings.\nconst function camera(context: ui.Context): waypoints.Settings\nreturn {enabled = context.scope > 0}\nend\nconst NAMESPACE: string = 'camera'\nlocal ui_context = {scope = 1, destroy = function() end}\ntype Box<T = string> = {value: T}\nfunction ui_context:show(value: number): number return self.scope + value end\nui_context:show(1)\ncamera(ui_context)\nreturn NAMESPACE, ui_context";
    let mut client = Client::start()?;
    client.open(&identifier, source)?;
    assert!(!has_errors(&client.diagnostics(&identifier)?));

    for (line, column) in [(3, 17), (11, 2)] {
        let hover = client.query(&identifier, "textDocument/hover", line, column)?;
        let description = hover["contents"]["value"].as_str().ok_or("function")?;

        assert!(
            description.contains("function camera(context: ui.Context): waypoints.Settings"),
            "{hover}"
        );

        assert!(!description.contains("read scope"), "{hover}");
        assert!(description.contains("Builds camera settings."), "{hover}");
    }

    for (line, column) in [(6, 8), (12, 8)] {
        let binding = client.query(&identifier, "textDocument/hover", line, column)?;

        assert_eq!(
            binding["contents"]["value"],
            "```luau\nconst NAMESPACE: string\n```"
        );
    }

    let binding = client.query(&identifier, "textDocument/hover", 7, 8)?;
    let description = binding["contents"]["value"].as_str().ok_or("table")?;

    assert!(
        description.starts_with("```luau\nlocal ui_context: {"),
        "{binding}"
    );

    assert!(
        description.contains("destroy:") && description.contains("scope:"),
        "{binding}"
    );

    let alias = client.query(&identifier, "textDocument/hover", 8, 6)?;

    assert_eq!(
        alias["range"],
        json!({"start":{"line":8,"character":5},"end":{"line":8,"character":8}})
    );

    assert!(
        alias["contents"]["value"]
            .as_str()
            .ok_or("alias")?
            .contains("type Box<T = string> ="),
        "{alias}"
    );

    let method = client.query(&identifier, "textDocument/hover", 10, 12)?;

    assert!(
        method["contents"]["value"]
            .as_str()
            .ok_or("method")?
            .contains("function ui_context:show(value: number): number"),
        "{method}"
    );

    client.shutdown()
}

#[test]
fn incremental_changes_use_sequential_unicode_ranges() -> TestResult {
    let directory = tempfile::tempdir()?;

    let identifier = Uri::from_file_path(directory.path().join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let mut client = Client::start()?;
    client.open(&identifier, "local value = \"😀\"\r\nreturn value")?;
    client.diagnostics(&identifier)?;
    client.send(&json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":identifier,"version":2},"contentChanges":[{"range":{"start":{"line":0,"character":15},"end":{"line":0,"character":17}},"text":"hello"},{"range":{"start":{"line":0,"character":14},"end":{"line":0,"character":21}},"text":"42"}]}}))?;
    assert!(!has_errors(&client.diagnostics(&identifier)?));
    let hover = client.query(&identifier, "textDocument/hover", 1, 9)?;

    assert!(
        hover["contents"]["value"]
            .as_str()
            .ok_or("hover")?
            .contains("number"),
        "{hover}"
    );

    client.shutdown()
}

#[test]
fn require_paths_complete_aliases_relative_paths_and_self() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::create_dir(root.join("packages"))?;
    fs::create_dir(root.join("nested"))?;

    fs::write(
        root.join("packages/Utilities.luau"),
        "return table.freeze({value = 1})",
    )?;

    fs::write(root.join("nested/Child.luau"), "return 2")?;

    fs::write(
        root.join("instar.toml"),
        "[aliases]\ncustomAlias = 'packages'\n",
    )?;

    let uri = Uri::from_file_path(root.join("nested/init.luau"))
        .ok_or("URI")?
        .to_string();

    let mut client = Client::start()?;
    client.open(&uri, "return require('')")?;
    client.diagnostics(&uri)?;
    let roots = client.query(&uri, "textDocument/completion", 0, 16)?;

    for label in ["./", "../", "@self/", "@customAlias/"] {
        assert!(
            roots
                .as_array()
                .ok_or("roots")?
                .iter()
                .any(|item| item["label"] == label),
            "{label}: {roots}"
        );
    }

    for (version, partial, expected) in [
        (2, "@customAlias/U", "@customAlias/Utilities"),
        (3, "./packages/U", "./packages/Utilities"),
        (4, "@self/Ch", "@self/Child"),
        (5, "../", "../"),
    ] {
        let text = format!("return require('{partial}')");
        client.change(&uri, version * 2, &text)?;
        client.diagnostics(&uri)?;

        let items = client.query(
            &uri,
            "textDocument/completion",
            0,
            u32::try_from(16 + partial.len())?,
        )?;

        if version == 5 {
            let folder = format!(
                "../{}/",
                root.file_name().ok_or("directory")?.to_string_lossy()
            );

            assert!(
                items
                    .as_array()
                    .ok_or("folders")?
                    .iter()
                    .any(|item| item["label"] == folder),
                "{items}"
            );

            continue;
        }

        let item = items
            .as_array()
            .ok_or("items")?
            .iter()
            .find(|item| item["label"] == expected)
            .ok_or_else(|| format!("missing {expected}: {items}"))?;

        assert_eq!(item["textEdit"]["newText"], expected);
        assert_eq!(item["textEdit"]["range"]["start"]["character"], 16);

        assert_eq!(
            item["textEdit"]["range"]["end"]["character"],
            16 + partial.len()
        );

        client.change(
            &uri,
            version * 2 + 1,
            &format!(
                "return require('{}')",
                item["textEdit"]["newText"].as_str().ok_or("text")?
            ),
        )?;

        assert!(!has_errors(&client.diagnostics(&uri)?));
    }

    client.shutdown()
}

#[test]
fn missing_modules_offer_import_actions_and_reuse_bindings() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();

    fs::write(
        root.join("Utilities.luau"),
        "return table.freeze({value = 1})",
    )?;

    let uri = Uri::from_file_path(root.join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let workspace = Uri::from_file_path(root).ok_or("URI")?.to_string();
    let mut client = Client::start()?;
    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWorkspaceFolders","params":{"event":{"added":[{"uri":workspace,"name":"workspace"}],"removed":[]}}}))?;
    client.open(&uri, "--!strict\nprint(Utilities)\nreturn Utilities")?;
    let diagnostics = client.diagnostics(&uri)?;
    let completions = client.query(&uri, "textDocument/completion", 2, 16)?;

    assert!(
        completions
            .as_array()
            .ok_or("completions")?
            .iter()
            .any(|item| item["label"] == "Utilities" && item["additionalTextEdits"].is_array()),
        "{completions}"
    );

    let actions = client.request("textDocument/codeAction", json!({"textDocument":{"uri":uri},"range":{"start":{"line":2,"character":7},"end":{"line":2,"character":16}},"context":{"diagnostics":diagnostics["diagnostics"],"only":["quickfix"]}}))?;

    let action = actions
        .as_array()
        .ok_or("actions")?
        .iter()
        .find(|action| {
            action["title"]
                .as_str()
                .is_some_and(|title| title.starts_with("Import 'Utilities'"))
        })
        .ok_or_else(|| format!("missing import fix: {actions}; {diagnostics}"))?;

    assert_eq!(
        action["edit"]["documentChanges"][0]["textDocument"]["version"],
        1
    );

    let declaration = action["edit"]["documentChanges"][0]["edits"][0]["newText"]
        .as_str()
        .ok_or("declaration")?;

    client.change(
        &uri,
        2,
        &format!("--!strict\n{declaration}print(Utilities)\nreturn Utilities"),
    )?;

    assert!(!has_errors(&client.diagnostics(&uri)?));

    client.change(
        &uri,
        3,
        "local existing = require('./Utilities')\nreturn Utilities",
    )?;

    client.diagnostics(&uri)?;
    let completions = client.query(&uri, "textDocument/completion", 1, 16)?;

    let item = completions
        .as_array()
        .ok_or("completions")?
        .iter()
        .find(|item| item["label"] == "Utilities")
        .ok_or("missing existing binding")?;

    assert_eq!(item["textEdit"]["newText"], "existing");
    assert!(item["additionalTextEdits"].is_null());

    client.shutdown()
}

#[test]
fn roblox_imports_offer_services_datamodel_paths_and_actions() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();

    fs::write(
        root.join("instar.toml"),
        "[roblox]\nsourcemap = 'sourcemap.json'\n",
    )?;

    fs::write(
        root.join("sourcemap.json"),
        r#"{"name":"Game","className":"DataModel","children":[{"name":"ReplicatedStorage","className":"ReplicatedStorage","children":[{"name":"Utilities","className":"ModuleScript","filePaths":["utilities.luau"]},{"name":"Main","className":"ModuleScript","filePaths":["main.luau"]}]}]}"#,
    )?;

    fs::write(
        root.join("utilities.luau"),
        "return table.freeze({value = 1})",
    )?;

    support::configure(root)?;

    let uri = Uri::from_file_path(root.join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let mut client = Client::start()?;
    client.open(&uri, "--!strict\nreturn Players")?;
    let diagnostics = client.diagnostics(&uri)?;
    let services = client.query(&uri, "textDocument/completion", 1, 14)?;

    let service = services
        .as_array()
        .ok_or("services")?
        .iter()
        .find(|item| item["label"] == "Players" && item["additionalTextEdits"].is_array())
        .ok_or_else(|| format!("missing Players: {services}"))?;

    assert_eq!(
        service["additionalTextEdits"][0]["newText"],
        "local Players = game:GetService(\"Players\")\n"
    );

    let actions = client.request("textDocument/codeAction", json!({"textDocument":{"uri":uri},"range":{"start":{"line":1,"character":7},"end":{"line":1,"character":14}},"context":{"diagnostics":diagnostics["diagnostics"],"only":["quickfix"]}}))?;

    assert!(
        actions
            .as_array()
            .ok_or("actions")?
            .iter()
            .any(|action| action["title"]
                .as_str()
                .is_some_and(|title| title.starts_with("Import 'Players'"))),
        "{actions}"
    );

    client.change(&uri, 2, "return Uti")?;
    client.diagnostics(&uri)?;
    let modules = client.query(&uri, "textDocument/completion", 0, 10)?;

    for expression in [
        "require(script.Parent.Utilities)",
        "require(game:GetService(\"ReplicatedStorage\").Utilities)",
        "require(\"@game/ReplicatedStorage/Utilities\")",
        "require(\"@self/../Utilities\")",
    ] {
        assert!(
            modules
                .as_array()
                .ok_or("modules")?
                .iter()
                .any(|item| item["label"] == "Utilities" && item["detail"] == expression),
            "{expression}: {modules}"
        );
    }

    for (version, partial, expected) in [
        (3, "@game/Rep", "@game/ReplicatedStorage/"),
        (
            5,
            "@game/ReplicatedStorage/Uti",
            "@game/ReplicatedStorage/Utilities",
        ),
        (7, "@self/../Uti", "@self/../Utilities"),
    ] {
        client.change(&uri, version, &format!("return require('{partial}')"))?;
        client.diagnostics(&uri)?;

        let items = client.query(
            &uri,
            "textDocument/completion",
            0,
            u32::try_from(16 + partial.len())?,
        )?;

        assert!(
            items
                .as_array()
                .ok_or("paths")?
                .iter()
                .any(|item| item["label"] == expected),
            "{expected}: {items}"
        );

        if !expected.ends_with('/') {
            client.change(
                &uri,
                version + 1,
                &format!(
                    "--!strict\nlocal value: number = require('{expected}').value\nreturn value"
                ),
            )?;

            assert!(!has_errors(&client.diagnostics(&uri)?));
        }
    }

    client.shutdown()
}

#[test]
fn indexed_modules_offer_imports_and_refresh_symbols() -> TestResult {
    let directory = tempfile::tempdir()?;

    let root = Uri::from_file_path(directory.path())
        .ok_or("URI")?
        .to_string();

    let identifier = Uri::from_file_path(directory.path().join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let dependency = Uri::from_file_path(directory.path().join("Utilities.luau"))
        .ok_or("URI")?
        .to_string();

    fs::write(
        directory.path().join("Utilities.luau"),
        "local function original() return 1 end\nreturn original",
    )?;

    let mut client = Client::start()?;
    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWorkspaceFolders","params":{"event":{"added":[{"uri":root,"name":"workspace"}],"removed":[]}}}))?;
    client.open(&identifier, "return Uti")?;
    client.diagnostics(&identifier)?;
    let completions = client.query(&identifier, "textDocument/completion", 0, 10)?;

    let import = completions
        .as_array()
        .ok_or("completions")?
        .iter()
        .find(|item| item["label"] == "Utilities")
        .ok_or("missing auto-import")?;

    assert_eq!(
        import["additionalTextEdits"][0]["newText"],
        "local Utilities = require(\"./Utilities\")\n"
    );

    client.send(
        &json!({"jsonrpc":"2.0","id":20,"method":"workspace/symbol","params":{"query":"original"}}),
    )?;

    assert_eq!(
        client.response(20)?["result"]
            .as_array()
            .ok_or("symbols")?
            .len(),
        1
    );

    client.open(
        &dependency,
        "local function replacement() return 2 end\nreturn replacement",
    )?;

    client.diagnostics(&dependency)?;

    client.send(
        &json!({"jsonrpc":"2.0","id":20,"method":"workspace/symbol","params":{"query":"original"}}),
    )?;

    assert_eq!(client.response(20)?["result"], json!([]));
    client.send(&json!({"jsonrpc":"2.0","id":20,"method":"workspace/symbol","params":{"query":"replacement"}}))?;

    assert_eq!(
        client.response(20)?["result"]
            .as_array()
            .ok_or("symbols")?
            .len(),
        1
    );

    client.shutdown()
}

#[test]
fn call_hierarchy_groups_calls_and_excludes_function_values() -> TestResult {
    let directory = tempfile::tempdir()?;

    let identifier = Uri::from_file_path(directory.path().join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let source = "local function target() return 1 end\nlocal function caller()\nreturn target() + target()\nend\nlocal value = target\nreturn caller, value";
    let mut client = Client::start()?;
    client.open(&identifier, source)?;
    client.diagnostics(&identifier)?;
    let prepared = client.query(&identifier, "textDocument/prepareCallHierarchy", 0, 18)?;
    assert_eq!(prepared[0]["name"], "target", "{prepared}");
    client.send(&json!({"jsonrpc":"2.0","id":20,"method":"callHierarchy/incomingCalls","params":{"item":prepared[0]}}))?;
    let incoming = client.response(20)?;

    assert_eq!(
        incoming["result"].as_array().ok_or("incoming")?.len(),
        1,
        "{incoming}"
    );

    assert_eq!(incoming["result"][0]["from"]["name"], "caller");

    assert_eq!(
        incoming["result"][0]["fromRanges"]
            .as_array()
            .ok_or("ranges")?
            .len(),
        2
    );

    client.send(&json!({"jsonrpc":"2.0","id":20,"method":"callHierarchy/outgoingCalls","params":{"item":incoming["result"][0]["from"]}}))?;
    let outgoing = client.response(20)?;
    assert_eq!(outgoing["result"][0]["to"]["name"], "target", "{outgoing}");

    assert_eq!(
        outgoing["result"][0]["fromRanges"]
            .as_array()
            .ok_or("ranges")?
            .len(),
        2
    );

    client.shutdown()
}

#[test]
fn call_hierarchy_resolves_table_functions_across_files() -> TestResult {
    let directory = tempfile::tempdir()?;

    let root = Uri::from_file_path(directory.path())
        .ok_or("URI")?
        .to_string();

    let identifier = Uri::from_file_path(directory.path().join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let dependency = Uri::from_file_path(directory.path().join("dependency.luau"))
        .ok_or("URI")?
        .to_string();

    fs::write(
        directory.path().join("dependency.luau"),
        "return { target = function() return 1 end }",
    )?;

    let mut client = Client::start()?;
    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWorkspaceFolders","params":{"event":{"added":[{"uri":root,"name":"workspace"}],"removed":[]}}}))?;
    client.open(&identifier, "local module = require('./dependency')\nlocal function caller()\nreturn module.target()\nend\nreturn caller")?;
    client.diagnostics(&identifier)?;
    let prepared = client.query(&identifier, "textDocument/prepareCallHierarchy", 2, 16)?;
    assert_eq!(prepared[0]["name"], "target", "{prepared}");
    assert_eq!(prepared[0]["uri"], dependency);
    client.send(&json!({"jsonrpc":"2.0","id":20,"method":"callHierarchy/incomingCalls","params":{"item":prepared[0]}}))?;
    let incoming = client.response(20)?;

    assert_eq!(
        incoming["result"][0]["from"]["name"], "caller",
        "{incoming}"
    );

    client.open(
        &dependency,
        "return { replacement = function() return 2 end }",
    )?;

    client.diagnostics(&dependency)?;
    client.send(&json!({"jsonrpc":"2.0","id":20,"method":"callHierarchy/incomingCalls","params":{"item":prepared[0]}}))?;
    assert_eq!(client.response(20)?["result"], json!([]));

    client.shutdown()
}

#[test]
fn imports_preserve_directives_comments_and_existing_bindings() -> TestResult {
    let directory = tempfile::tempdir()?;

    let root = Uri::from_file_path(directory.path())
        .ok_or("URI")?
        .to_string();

    let identifier = Uri::from_file_path(directory.path().join("main.luau"))
        .ok_or("URI")?
        .to_string();

    fs::write(directory.path().join("Utilities.luau"), "return {}")?;
    let mut client = Client::start()?;
    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWorkspaceFolders","params":{"event":{"added":[{"uri":root,"name":"workspace"}],"removed":[]}}}))?;
    client.open(&identifier, "--!strict\r\n--- Documentation\r\nreturn Uti")?;
    client.diagnostics(&identifier)?;
    let completions = client.query(&identifier, "textDocument/completion", 2, 10)?;

    let import = completions
        .as_array()
        .ok_or("completions")?
        .iter()
        .find(|item| item["label"] == "Utilities")
        .ok_or("missing auto-import")?;

    assert_eq!(
        import["additionalTextEdits"][0]["range"]["start"]["line"],
        1
    );

    assert_eq!(
        import["additionalTextEdits"][0]["newText"],
        "local Utilities = require(\"./Utilities\")\r\n"
    );

    client.change(&identifier, 2, "local Utilities = {}\nreturn Uti")?;
    client.diagnostics(&identifier)?;
    let completions = client.query(&identifier, "textDocument/completion", 1, 10)?;

    assert!(
        completions
            .as_array()
            .ok_or("completions")?
            .iter()
            .all(|item| item.get("additionalTextEdits").is_none()),
        "{completions}"
    );

    fs::create_dir(directory.path().join("Utilities"))?;
    fs::write(directory.path().join("Utilities/init.luau"), "return {}")?;

    let created = Uri::from_file_path(directory.path().join("Utilities/init.luau"))
        .ok_or("URI")?
        .to_string();

    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":created,"type":1}]}}))?;
    client.change(&identifier, 3, "return Uti")?;

    while client.diagnostics(&identifier)?["version"] != 3 {}

    let completions = client.query(&identifier, "textDocument/completion", 0, 10)?;

    assert!(
        completions
            .as_array()
            .ok_or("completions")?
            .iter()
            .all(|item| item["label"] != "Utilities"),
        "{completions}"
    );

    client.shutdown()
}

#[test]
fn semantic_tokens_distinguish_parameters_methods_and_constants() -> TestResult {
    let directory = tempfile::tempdir()?;

    let identifier = Uri::from_file_path(directory.path().join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let mut client = Client::start()?;
    client.open(&identifier, "const fixed = 1\nlocal object = {}\nfunction object:method(parameter: number)\nreturn parameter + fixed\nend\nreturn object")?;
    client.diagnostics(&identifier)?;
    let tokens = client.query(&identifier, "textDocument/semanticTokens/full", 0, 0)?;
    let values = tokens["data"].as_array().ok_or("tokens")?;

    assert!(
        values.as_chunks::<5>().0.iter().any(|token| token[3] == 5),
        "{tokens}"
    );

    assert!(
        values.as_chunks::<5>().0.iter().any(|token| token[3] == 6),
        "{tokens}"
    );

    assert!(
        values
            .as_chunks::<5>()
            .0
            .iter()
            .any(|token| token[4].as_u64().is_some_and(|value| value & 2 != 0)),
        "{tokens}"
    );

    client.shutdown()
}

#[test]
fn unused_binding_actions_preserve_document_versions() -> TestResult {
    let directory = tempfile::tempdir()?;

    let identifier = Uri::from_file_path(directory.path().join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let mut client = Client::start()?;
    client.open(&identifier, "local unused = 1\nreturn 1")?;
    let diagnostics = client.diagnostics(&identifier)?;
    client.send(&json!({"jsonrpc":"2.0","id":20,"method":"textDocument/codeAction","params":{"textDocument":{"uri":identifier},"range":{"start":{"line":0,"character":0},"end":{"line":1,"character":8}},"context":{"diagnostics":diagnostics["diagnostics"],"only":["quickfix"]}}}))?;
    let actions = client.response(20)?;

    assert_eq!(
        actions["result"].as_array().ok_or("actions")?.len(),
        1,
        "{actions}"
    );

    let change = &actions["result"][0]["edit"]["documentChanges"][0];
    assert_eq!(change["textDocument"]["version"], 1);
    assert_eq!(change["edits"][0]["newText"], "_unused");
    client.change(&identifier, 2, "local _unused = 1\nreturn 1")?;
    let diagnostics = client.diagnostics(&identifier)?;

    assert!(
        diagnostics["diagnostics"]
            .as_array()
            .ok_or("diagnostics")?
            .iter()
            .all(|diagnostic| diagnostic["code"] != "LocalUnused")
    );

    client.shutdown()
}

#[test]
fn colors_offer_literal_preserving_presentations() -> TestResult {
    let directory = tempfile::tempdir()?;

    let identifier = Uri::from_file_path(directory.path().join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let source = "local first = Color3.fromRGB(300, -20, 0)\nlocal second = Color3.fromHex('#0f0')\nlocal third = Color3.new(0, 0, 1)\nlocal function custom(Color3) return Color3.new(1, 1, 1) end\nreturn first, second, third";
    let mut client = Client::start()?;
    client.open(&identifier, source)?;
    client.diagnostics(&identifier)?;
    let colors = client.query(&identifier, "textDocument/documentColor", 0, 0)?;
    assert_eq!(colors.as_array().ok_or("colors")?.len(), 3, "{colors}");

    assert_eq!(
        colors[0]["color"],
        json!({"red":1.0,"green":0.0,"blue":0.0,"alpha":1.0})
    );

    assert_eq!(colors[1]["color"]["green"], 1.0);
    assert_eq!(colors[2]["color"]["blue"], 1.0);
    client.send(&json!({"jsonrpc":"2.0","id":20,"method":"textDocument/colorPresentation","params":{"textDocument":{"uri":identifier},"range":colors[0]["range"],"color":colors[0]["color"]}}))?;
    let presentations = client.response(20)?;

    assert_eq!(
        presentations["result"]
            .as_array()
            .ok_or("presentations")?
            .len(),
        3
    );

    assert_eq!(
        presentations["result"][1]["textEdit"]["newText"],
        "Color3.fromRGB(255, 0, 0)"
    );

    assert_eq!(
        presentations["result"][2]["label"],
        "Color3.fromHex(\"#FF0000\")"
    );

    client.shutdown()
}

#[test]
fn structured_comments_and_document_structure() -> TestResult {
    let directory = tempfile::tempdir()?;

    let identifier = Uri::from_file_path(directory.path().join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let source = "--- Adds a number.\n--- @param amount number -- Amount\n--- @return number -- Result\n--- @within Numbers\nlocal function increase(amount: number): number\nreturn amount + 1\nend\nlocal result = increase(1)\nreturn result";
    let mut client = Client::start()?;
    client.open(&identifier, source)?;
    assert!(!has_errors(&client.diagnostics(&identifier)?));
    let hover = client.query(&identifier, "textDocument/hover", 7, 19)?;
    let documentation = hover["contents"]["value"].as_str().ok_or("hover")?;

    assert!(
        documentation.contains("**Parameters**")
            && documentation.contains("`amount` number -- Amount"),
        "{hover}"
    );

    assert!(
        documentation.contains("**Returns**") && !documentation.contains("@within"),
        "{hover}"
    );

    let highlights = client.query(&identifier, "textDocument/documentHighlight", 5, 9)?;

    assert_eq!(
        highlights.as_array().ok_or("highlights")?.len(),
        2,
        "{highlights}"
    );

    let folds = client.query(&identifier, "textDocument/foldingRange", 0, 0)?;

    assert!(
        folds
            .as_array()
            .ok_or("folds")?
            .iter()
            .any(|range| range["startLine"] == 4 && range["endLine"] == 6),
        "{folds}"
    );

    client.send(&json!({"jsonrpc":"2.0","id":20,"method":"textDocument/selectionRange","params":{"textDocument":{"uri":identifier},"positions":[{"line":5,"character":9}]}}))?;
    let selections = client.response(20)?;

    assert_eq!(
        selections["result"][0]["range"],
        json!({"start":{"line":5,"character":7},"end":{"line":5,"character":13}})
    );

    assert!(
        selections["result"][0]["parent"].is_object(),
        "{selections}"
    );

    client.shutdown()
}

#[test]
fn roblox_hover_includes_examples_and_reference_links() -> TestResult {
    let directory = tempfile::tempdir()?;
    fs::write(directory.path().join("instar.toml"), "[roblox]\n")?;
    support::configure(directory.path())?;

    let identifier = Uri::from_file_path(directory.path().join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let mut client = Client::start()?;

    client.open(
        &identifier,
        "local model = Instance.new('Model')\nreturn model",
    )?;

    assert!(!has_errors(&client.diagnostics(&identifier)?));
    let hover = client.query(&identifier, "textDocument/hover", 1, 8)?;
    let documentation = hover["contents"]["value"].as_str().ok_or("hover")?;

    assert!(
        documentation.contains(
            "[Learn More](https://create.roblox.com/docs/reference/engine/classes/Model)"
        ),
        "{hover}"
    );

    assert!(
        documentation.contains("```luau\nlocal function groupObjects(objectTable)"),
        "{hover}"
    );

    assert!(
        documentation.contains("groupObjects(objects)\n```"),
        "{hover}"
    );

    client.shutdown()
}

#[test]
fn return_annotations_show_class_documentation() -> TestResult {
    let directory = tempfile::tempdir()?;
    fs::write(directory.path().join("instar.toml"), "[roblox]\n")?;
    support::configure(directory.path())?;

    let identifier = Uri::from_file_path(directory.path().join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let source = "const function resolve_mount_root(): (BasePlayerGui | Folder)?\nreturn nil\nend";
    let mut client = Client::start()?;
    client.open(&identifier, source)?;
    assert!(!has_errors(&client.diagnostics(&identifier)?));

    for (name, documentation) in [
        ("Folder", "A simple container"),
        ("BasePlayerGui", "is an abstract class"),
    ] {
        let column = u32::try_from(source.find(name).ok_or("annotation")?)?;
        let hover = client.query(&identifier, "textDocument/hover", 0, column)?;
        let contents = hover["contents"]["value"].as_str().ok_or("hover")?;

        assert!(
            contents.contains(name) && contents.contains(documentation),
            "{hover}"
        );

        assert!(!contents.contains("resolve_mount_root"), "{hover}");

        assert_eq!(
            hover["range"]["start"],
            json!({"line":0,"character":column})
        );
    }

    client.shutdown()
}

#[test]
fn document_links_resolve_imports_with_unicode_ranges() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::write(root.join("mód ü.luau"), "return {}")?;
    fs::write(root.join("instar.toml"), "[aliases]\npackages = './'\n")?;

    let identifier = Uri::from_file_path(root.join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let target = Uri::from_file_path(root.join("mód ü.luau"))
        .ok_or("target")?
        .to_string();

    let source = "local marker = '😀'; local fluid = require('@packages/mód ü')\nlocal missing = require('./missing')\nlocal text = '@packages/mód ü'\nlocal function custom(require) return require('@packages/mód ü') end\nreturn fluid";
    let mut client = Client::start()?;
    client.open(&identifier, source)?;
    client.diagnostics(&identifier)?;
    let links = client.query(&identifier, "textDocument/documentLink", 0, 0)?;
    assert_eq!(links.as_array().ok_or("links")?.len(), 1, "{links}");
    assert_eq!(links[0]["target"], target);

    let start = source[..source.find("'@packages").ok_or("range")?]
        .encode_utf16()
        .count();

    assert_eq!(
        links[0]["range"],
        json!({"start":{"line":0,"character":start},"end":{"line":0,"character":start + "'@packages/mód ü'".encode_utf16().count()}})
    );

    fs::remove_file(root.join("mód ü.luau"))?;
    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didDeleteFiles","params":{"files":[{"uri":target}]}}))?;
    client.diagnostics(&identifier)?;
    let links = client.query(&identifier, "textDocument/documentLink", 0, 0)?;
    assert_eq!(links, json!([]));

    client.shutdown()
}

#[test]
fn resolved_imports_receive_namespace_tokens() -> TestResult {
    let directory = tempfile::tempdir()?;
    fs::write(directory.path().join("dependency.luau"), "return {}")?;

    fs::write(
        directory.path().join("instar.toml"),
        "[aliases]\npackages = './'\n",
    )?;

    let identifier = Uri::from_file_path(directory.path().join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let mut client =
        Client::start_with(&json!({"workspace":{"semanticTokens":{"refreshSupport":true}}}))?;

    client.open(&identifier, "local dependency = require('./dependency')\nlocal package = require('@packages/dependency')\nlocal missing = require('./missing')\nlocal text = './dependency'\nreturn dependency")?;
    client.diagnostics(&identifier)?;
    let refresh = client.receive()?;
    assert_eq!(refresh["method"], "workspace/semanticTokens/refresh");
    client.send(&json!({"jsonrpc":"2.0","id":refresh["id"],"result":null}))?;
    let tokens = client.query(&identifier, "textDocument/semanticTokens/full", 0, 0)?;
    let data = tokens["data"].as_array().ok_or("tokens")?;

    let imports = data
        .as_chunks::<5>()
        .0
        .iter()
        .filter(|token| token[3] == 4)
        .collect::<Vec<_>>();

    assert_eq!(imports.len(), 2, "{tokens}");
    assert_eq!(imports[0][2], "'./dependency'".len());
    assert_eq!(imports[1][2], "'@packages/dependency'".len());
    fs::remove_file(directory.path().join("dependency.luau"))?;

    let dependency = Uri::from_file_path(directory.path().join("dependency.luau"))
        .ok_or("dependency")?
        .to_string();

    client.send(&json!({"jsonrpc":"2.0","method":"workspace/didDeleteFiles","params":{"files":[{"uri":dependency}]}}))?;
    client.diagnostics(&identifier)?;
    let refresh = client.receive()?;
    assert_eq!(refresh["method"], "workspace/semanticTokens/refresh");
    client.send(&json!({"jsonrpc":"2.0","id":refresh["id"],"result":null}))?;
    let tokens = client.query(&identifier, "textDocument/semanticTokens/full", 0, 0)?;

    assert!(
        tokens["data"]
            .as_array()
            .ok_or("tokens")?
            .as_chunks::<5>()
            .0
            .iter()
            .all(|token| token[3] != 4),
        "{tokens}"
    );

    client.shutdown()
}

#[test]
fn syntax_diagnostics_accept_unicode_errors() -> TestResult {
    let directory = tempfile::tempdir()?;

    let uri = Uri::from_file_path(directory.path().join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let mut client = Client::start()?;
    client.open(&uri, "local 😀 = 1")?;
    assert!(has_errors(&client.diagnostics(&uri)?));

    client.shutdown()
}

#[test]
fn editor_requests_reject_results_from_older_snapshots() -> TestResult {
    let directory = tempfile::tempdir()?;
    fs::write(directory.path().join("instar.toml"), "[roblox]\n")?;
    support::configure(directory.path())?;

    let uri = Uri::from_file_path(directory.path().join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let mut client = Client::start()?;
    client.open(&uri, "local value = 1\nreturn value")?;
    client.send(&json!({"jsonrpc":"2.0","id":20,"method":"textDocument/hover","params":{"textDocument":{"uri":uri},"position":{"line":1,"character":8}}}))?;
    client.change(&uri, 2, "local value = 'changed'\nreturn value")?;
    assert_eq!(client.response(20)?["error"]["code"], -32801);
    assert_eq!(client.diagnostics(&uri)?["version"], 2);
    let hover = client.query(&uri, "textDocument/hover", 1, 8)?;

    assert!(
        hover["contents"]["value"]
            .as_str()
            .ok_or("hover")?
            .contains("string")
    );

    client.shutdown()
}

#[test]
fn method_definitions_and_symbols_select_the_method_name() -> TestResult {
    let directory = tempfile::tempdir()?;

    let uri = Uri::from_file_path(directory.path().join("main.luau"))
        .ok_or("URI")?
        .to_string();

    let mut client = Client::start()?;
    client.open(&uri, "local library = {}\nfunction library.run(amount: number): number\nreturn amount\nend\nreturn library.run(1)")?;
    assert!(!has_errors(&client.diagnostics(&uri)?));
    let target = client.query(&uri, "textDocument/definition", 4, 16)?;

    assert_eq!(
        target["range"]["start"],
        json!({"line":1,"character":17}),
        "{target}"
    );

    let references = client.query(&uri, "textDocument/references", 4, 16)?;

    assert_eq!(
        references.as_array().ok_or("references")?.len(),
        2,
        "{references}"
    );

    let symbols = client.query(&uri, "textDocument/documentSymbol", 0, 0)?;

    assert!(
        symbols
            .as_array()
            .ok_or("symbols")?
            .iter()
            .any(|symbol| symbol["name"] == "run" && symbol["kind"] == 12)
    );

    let annotation = client.query(&uri, "textDocument/hover", 1, 30)?;

    assert!(
        annotation["contents"]["value"]
            .as_str()
            .ok_or("annotation hover")?
            .contains("number")
    );

    assert_eq!(
        annotation["range"]["start"],
        json!({"line":1,"character":29})
    );

    let declaration = client.query(&uri, "textDocument/hover", 0, 7)?;

    assert!(
        declaration["contents"]["value"]
            .as_str()
            .ok_or("declaration hover")?
            .contains("run"),
        "{declaration}"
    );

    client.shutdown()
}

#[test]
fn diagnostics_measure_open_and_edits() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::write(root.join("instar.toml"), "[roblox]\n")?;
    support::configure(root)?;

    let uri = Uri::from_file_path(root.join("main.luau"))
        .ok_or("document URI")?
        .to_string();

    let mut client = Client::start()?;
    let started = std::time::Instant::now();
    client.open(&uri, "--!strict\nlocal value: number = 1\nreturn value")?;
    assert!(!has_errors(&client.diagnostics(&uri)?));
    eprintln!("diagnostics cold open: {:?}", started.elapsed());

    for (version, value) in [(2, "'wrong'"), (3, "2"), (4, "'wrong'"), (5, "3")] {
        let started = std::time::Instant::now();

        client.change(
            &uri,
            version,
            &format!("--!strict\nlocal value: number = {value}\nreturn value"),
        )?;

        assert_eq!(has_errors(&client.diagnostics(&uri)?), value == "'wrong'");
        eprintln!("diagnostics edit {version}: {:?}", started.elapsed());
    }

    client.change(
        &uri,
        6,
        "--!strict\nlocal part = Instance.new('Part')\nreturn part.Size",
    )?;

    assert!(!has_errors(&client.diagnostics(&uri)?));
    let hover = client.query(&uri, "textDocument/hover", 2, 14)?;

    assert!(
        hover["contents"]["value"]
            .as_str()
            .ok_or("Roblox hover")?
            .contains("Vector3"),
        "{hover}"
    );

    let completion = client.query(&uri, "textDocument/completion", 2, 12)?;

    assert!(
        completion
            .as_array()
            .ok_or("Roblox completion")?
            .iter()
            .any(|entry| entry["label"] == "Anchored"),
        "{completion}"
    );

    client.shutdown()
}

#[test]
fn eof_without_shutdown_fails_without_protocol_output() {
    assert_cmd::Command::new(env!("CARGO_BIN_EXE_instar"))
        .arg("lsp")
        .write_stdin("")
        .assert()
        .failure()
        .stdout("")
        .stderr("");
}
