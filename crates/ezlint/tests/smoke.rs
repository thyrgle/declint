//! End-to-end smoke test: spawns the `ezlint` binary in LSP mode and drives
//! a real session over stdio — initialize, didOpen (with a tab), incremental
//! didChange (fixing it), shutdown.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

const CONFIG: &str = "\
version: 1
languages: [plaintext]
rules:
  - id: no-tabs
    pattern: '\\t+'
    message: \"Use spaces, found '{match}'\"
    severity: warning
  - id: loud-todo
    pattern: 'TODO(?<bang>!*)'
    message: fallback
    callback: |
      return function(c)
        if c.captures.bang == \"\" then
          return nil
        end
        return { severity = \"error\", message = \"loud TODO: \" .. #c.captures.bang .. \" bangs\" }
      end
scopes:
  - id: shell
    start: '^```sh$'
    end: '^```$'
    rules:
      - id: no-sudo
        pattern: '\\bsudo\\b'
        message: \"Don't use sudo in scripts\"
        severity: error
";

/// A tab on line 0 (global rule), a loud TODO on line 2 (callback rule:
/// bangs -> violation, quiet -> allowed), and a shell fence around a
/// `sudo` on line 4 (scoped rule).
const SOURCE: &str = "def x = 1;\t# note\n# quiet TODO\n# loud TODO!!!\n```sh\nsudo ls -la /\n```\n";

fn spawn_server(config_path: &str) -> (Child, ChildStdin, Receiver<String>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ezlint"))
        .args(["serve", config_path])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("failed to spawn ezlint");

    let stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");

    let (tx, rx) = channel::<String>();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut content_length: Option<usize> = None;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                if let Some(value) = line.strip_prefix("Content-Length: ") {
                    content_length = value.parse().ok();
                }
            }
            let Some(len) = content_length else { return };
            let mut body = vec![0u8; len];
            if reader.read_exact(&mut body).is_err() {
                return;
            }
            if tx
                .send(String::from_utf8_lossy(&body).into_owned())
                .is_err()
            {
                return;
            }
        }
    });

    (child, stdin, rx)
}

fn send(stdin: &mut ChildStdin, value: &serde_json::Value) {
    let body = value.to_string();
    write!(stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
    stdin.flush().unwrap();
}

fn next_message(rx: &Receiver<String>) -> serde_json::Value {
    let started = Instant::now();
    loop {
        let text = match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(text) => text,
            Err(RecvTimeoutError::Timeout) => panic!("timed out waiting for a server message"),
            Err(RecvTimeoutError::Disconnected) => panic!("server closed its output"),
        };
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        if value.get("id").is_some() || value.get("method").is_some() {
            return value;
        }
        assert!(started.elapsed() < Duration::from_secs(60));
    }
}

fn notification(method: &str, params: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"jsonrpc": "2.0", "method": method, "params": params})
}

fn request(id: i64, method: &str, params: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn wait_for_diagnostics(rx: &Receiver<String>, version: Option<i64>) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for diagnostics"
        );
        let msg = next_message(rx);
        if msg["method"] == "textDocument/publishDiagnostics"
            && msg["params"]["version"].as_i64() == version
        {
            return msg["params"]["diagnostics"].clone();
        }
    }
}

fn write_config() -> String {
    let path = std::env::temp_dir().join(format!("ezlint-smoke-{}.yaml", std::process::id()));
    std::fs::write(&path, CONFIG).expect("write config");
    path.to_string_lossy().into_owned()
}

#[test]
fn ezlint_server_smoke() {
    let config_path = write_config();
    let (mut child, mut stdin, rx) = spawn_server(&config_path);

    // initialize -> UTF-16 position encoding negotiated.
    send(
        &mut stdin,
        &request(1, "initialize", serde_json::json!({"capabilities": {}})),
    );
    let initialized = next_message(&rx);
    assert_eq!(initialized["id"], 1);
    assert_eq!(
        initialized["result"]["capabilities"]["positionEncoding"],
        "utf-16"
    );
    send(
        &mut stdin,
        &notification("initialized", serde_json::json!({})),
    );

    // didOpen -> two diagnostics: the tab (warning, global) and the sudo
    // (error, inside the shell-fence scope).
    send(
        &mut stdin,
        &notification(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": "file:///test.txt",
                    "languageId": "plaintext",
                    "version": 0,
                    "text": SOURCE,
                }
            }),
        ),
    );
    let diags = wait_for_diagnostics(&rx, Some(0));
    let diags = diags.as_array().unwrap();
    assert_eq!(diags.len(), 3, "tab + loud TODO + sudo must be flagged");

    let tab = diags
        .iter()
        .find(|d| d["code"] == "no-tabs")
        .expect("no-tabs diagnostic");
    assert_eq!(tab["severity"], 2, "warning");
    assert_eq!(tab["source"], "ezlint");
    assert!(
        tab["message"].to_string().contains("Use spaces"),
        "{}",
        tab["message"]
    );
    assert_eq!(tab["range"]["start"]["line"], 0);
    assert_eq!(tab["range"]["start"]["character"], 10);

    // The Lua callback: the quiet TODO on line 1 is allowed, the loud
    // one on line 2 produces a callback-computed message.
    let todo = diags
        .iter()
        .find(|d| d["code"] == "loud-todo")
        .expect("loud-todo diagnostic");
    assert_eq!(todo["severity"], 1, "callback overrode to error");
    assert_eq!(todo["message"], "loud TODO: 3 bangs");
    assert_eq!(todo["range"]["start"]["line"], 2);

    let sudo = diags
        .iter()
        .find(|d| d["code"] == "no-sudo")
        .expect("no-sudo diagnostic");
    assert_eq!(sudo["severity"], 1, "error");
    assert_eq!(sudo["range"]["start"]["line"], 4);
    assert_eq!(sudo["range"]["start"]["character"], 0);

    // Incremental didChange: replace the tab (line 0, char 10..11) with a
    // space -> the global violation heals; the scoped one remains.
    send(
        &mut stdin,
        &notification(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": { "uri": "file:///test.txt", "version": 1 },
                "contentChanges": [{
                    "range": {
                        "start": { "line": 0, "character": 10 },
                        "end": { "line": 0, "character": 11 },
                    },
                    "text": " ",
                }],
            }),
        ),
    );
    let diags = wait_for_diagnostics(&rx, Some(1));
    let diags = diags.as_array().unwrap();
    assert_eq!(diags.len(), 2, "the scoped sudo and the loud TODO survive");
    let codes: Vec<&str> = diags
        .iter()
        .map(|d| d["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, ["loud-todo", "no-sudo"]);

    // A document whose languageId does not match the config's
    // `languages` gets an empty publish, even with violations present.
    send(
        &mut stdin,
        &notification(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": "file:///other.py",
                    "languageId": "python",
                    "version": 100,
                    "text": SOURCE,
                }
            }),
        ),
    );
    let diags = wait_for_diagnostics(&rx, Some(100));
    assert_eq!(
        diags.as_array().unwrap().len(),
        0,
        "python is not in languages: [plaintext]"
    );

    // Shutdown handshake.
    send(&mut stdin, &request(2, "shutdown", serde_json::Value::Null));
    let shutdown = next_message(&rx);
    assert_eq!(shutdown["id"], 2);
    send(&mut stdin, &notification("exit", serde_json::Value::Null));

    drop(stdin);
    let status = child.wait().expect("wait");
    assert!(status.success(), "server exited with {status}");

    std::fs::remove_file(&config_path).ok();
}
