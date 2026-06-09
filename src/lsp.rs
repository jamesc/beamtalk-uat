// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0

//! Minimal, dependency-free LSP client for UAT scenarios.
//!
//! Drives the bundled `beamtalk-lsp` binary over stdio JSON-RPC (LSP
//! `Content-Length`-framed messages). The language server runs standalone in
//! its in-process AST mode — no workspace or BEAM runtime — so `lsp` scenarios
//! exercise editor capabilities (`documentSymbol`, `hover`, `completion`,
//! `definition`, …) on any platform without Erlang.
//!
//! The harness stays dependency-free, so this hand-rolls the framing and uses
//! **substring** assertions on the raw response rather than a JSON parser.
//! Assertions should target value substrings (e.g. `"self"`, `Extends:`) that
//! don't depend on the server's key spacing.
//!
//! A background reader thread parses framed messages off the server's stdout
//! onto a channel; requests block on `recv_timeout` so a hung server fails the
//! scenario with a clear timeout instead of wedging the whole run.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// How long to wait for a single response before declaring the server hung.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(20);

/// A live `beamtalk-lsp` process with a framed-message reader thread.
pub struct LspClient {
    child: Child,
    stdin: ChildStdin,
    rx: mpsc::Receiver<String>,
    next_id: i64,
}

impl LspClient {
    /// Spawn the language server at `bin` and start the reader thread.
    pub fn start(bin: &Path) -> Result<Self, String> {
        let mut child = Command::new(bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("spawn `{}`: {e}", bin.display()))?;

        let stdin = child.stdin.take().ok_or("failed to capture lsp stdin")?;
        let stdout = child.stdout.take().ok_or("failed to capture lsp stdout")?;

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Some(msg) = read_message(&mut reader) {
                if tx.send(msg).is_err() {
                    break;
                }
            }
        });

        Ok(LspClient {
            child,
            stdin,
            rx,
            next_id: 0,
        })
    }

    fn write_frame(&mut self, payload: &str) -> Result<(), String> {
        write!(
            self.stdin,
            "Content-Length: {}\r\n\r\n{}",
            payload.len(),
            payload
        )
        .and_then(|()| self.stdin.flush())
        .map_err(|e| format!("write to lsp stdin: {e}"))
    }

    /// Send a notification (no `id`, no response expected).
    pub fn notify(&mut self, method: &str, params_json: &str) -> Result<(), String> {
        let payload = format!(r#"{{"jsonrpc":"2.0","method":"{method}","params":{params_json}}}"#);
        self.write_frame(&payload)
    }

    /// Send a request and return the raw text of the matching response message.
    pub fn request(&mut self, method: &str, params_json: &str) -> Result<String, String> {
        self.next_id += 1;
        let id = self.next_id;
        let payload =
            format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{params_json}}}"#);
        self.write_frame(&payload)?;

        // Read messages until we see the response carrying our id, skipping any
        // notifications (diagnostics, log messages) the server interleaves.
        loop {
            match self.rx.recv_timeout(RESPONSE_TIMEOUT) {
                Ok(msg) if message_has_id(&msg, id) => return Ok(msg),
                Ok(_) => continue,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(format!(
                        "timed out after {}s waiting for `{method}` response",
                        RESPONSE_TIMEOUT.as_secs()
                    ));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(format!("lsp server exited before answering `{method}`"));
                }
            }
        }
    }

    /// `initialize` + `initialized` handshake with the given root URI.
    pub fn initialize(&mut self, root_uri: &str) -> Result<(), String> {
        let params = format!(
            r#"{{"processId":null,"rootUri":"{}","capabilities":{{}}}}"#,
            json_escape(root_uri)
        );
        self.request("initialize", &params)?;
        self.notify("initialized", "{}")
    }

    /// `textDocument/didOpen` for a document with the given URI and text.
    pub fn did_open(&mut self, uri: &str, text: &str) -> Result<(), String> {
        let params = format!(
            r#"{{"textDocument":{{"uri":"{}","languageId":"beamtalk","version":1,"text":"{}"}}}}"#,
            json_escape(uri),
            json_escape(text)
        );
        self.notify("textDocument/didOpen", &params)
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        // The server runs until shutdown/exit; just kill it.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Read one `Content-Length`-framed JSON message; `None` on EOF.
fn read_message<R: BufRead>(reader: &mut R) -> Option<String> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).ok()?;
        if n == 0 {
            return None; // EOF
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break; // end of headers
        }
        if let Some(rest) = trimmed
            .strip_prefix("Content-Length:")
            .or_else(|| trimmed.strip_prefix("content-length:"))
        {
            content_length = rest.trim().parse().ok();
        }
    }
    let len = content_length?;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Whether a response message carries the top-level `"id":<id>` we sent
/// (tolerating optional whitespace after the colon, which some servers emit).
///
/// The match is digit-boundary-aware so request id `2` does not spuriously match
/// a response for id `20`.
fn message_has_id(msg: &str, id: i64) -> bool {
    let id_s = id.to_string();
    for prefix in ["\"id\":", "\"id\": "] {
        let mut from = 0;
        while let Some(pos) = msg[from..].find(prefix) {
            let after = from + pos + prefix.len();
            if msg[after..].starts_with(&id_s) {
                let next = msg[after + id_s.len()..].chars().next();
                if !matches!(next, Some(c) if c.is_ascii_digit()) {
                    return true;
                }
            }
            from = after;
        }
    }
    false
}

/// Escape a string for embedding inside a JSON string literal.
pub fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_escape_handles_specials() {
        assert_eq!(json_escape("a\"b\\c"), "a\\\"b\\\\c");
        assert_eq!(json_escape("line1\nline2"), "line1\\nline2");
        assert_eq!(json_escape("tab\there"), "tab\\there");
    }

    #[test]
    fn message_id_match_tolerates_spacing() {
        assert!(message_has_id(r#"{"jsonrpc":"2.0","id":2,"result":[]}"#, 2));
        assert!(message_has_id(
            r#"{"jsonrpc":"2.0","id": 2,"result":[]}"#,
            2
        ));
        assert!(!message_has_id(
            r#"{"jsonrpc":"2.0","id":20,"result":[]}"#,
            2
        ));
        assert!(!message_has_id(
            r#"{"method":"textDocument/publishDiagnostics"}"#,
            2
        ));
    }

    #[test]
    fn read_message_parses_framed_payload() {
        let raw = "Content-Length: 13\r\n\r\n{\"hello\":1}\r\n";
        let mut reader = std::io::BufReader::new(raw.as_bytes());
        let msg = read_message(&mut reader).unwrap();
        assert_eq!(msg, "{\"hello\":1}\r\n");
    }
}
