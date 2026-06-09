// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0

//! Minimal, dependency-free MCP client for UAT scenarios.
//!
//! Drives the bundled `beamtalk-mcp` server over stdio JSON-RPC. Unlike LSP
//! (`Content-Length` framing), the MCP stdio transport is **newline-delimited**:
//! one JSON message per line. The server is launched with `--start`, which
//! spawns a `beamtalk repl` workspace in the background — so MCP scenarios need
//! a BEAM runtime (the `e2e` / `uat.yml` legs), and the bundle's `bin/` must be
//! on `PATH` because `--start` shells out to `beamtalk`.
//!
//! Like the LSP client, this stays dependency-free: hand-rolled framing,
//! substring assertions on the raw response, and a background reader thread with
//! `recv_timeout` so a stuck workspace fails the scenario instead of hanging.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Generous per-request timeout: `--start` boots a BEAM workspace on first use.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// A live `beamtalk-mcp --start` process with a line-reader thread.
pub struct McpClient {
    child: Child,
    stdin: ChildStdin,
    rx: mpsc::Receiver<String>,
    next_id: i64,
}

impl McpClient {
    /// Launch `beamtalk-mcp --start` in `cwd`, with `bin_dir` prepended to
    /// `PATH` so the server can find `beamtalk` to start the workspace.
    pub fn start(bin: &Path, cwd: &Path, bin_dir: &Path) -> Result<Self, String> {
        let existing = std::env::var("PATH").unwrap_or_default();
        let sep = if cfg!(windows) { ";" } else { ":" };
        let path = format!("{}{sep}{existing}", bin_dir.display());

        let mut child = Command::new(bin)
            .arg("--start")
            .current_dir(cwd)
            .env("PATH", path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("spawn `{} --start`: {e}", bin.display()))?;

        let stdin = child.stdin.take().ok_or("failed to capture mcp stdin")?;
        let stdout = child.stdout.take().ok_or("failed to capture mcp stdout")?;

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            // Newline-delimited: each line is one JSON-RPC message.
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(l) => {
                        if tx.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(McpClient {
            child,
            stdin,
            rx,
            next_id: 0,
        })
    }

    fn write_line(&mut self, payload: &str) -> Result<(), String> {
        writeln!(self.stdin, "{payload}")
            .and_then(|()| self.stdin.flush())
            .map_err(|e| format!("write to mcp stdin: {e}"))
    }

    /// Send a notification (no `id`).
    pub fn notify(&mut self, method: &str, params_json: &str) -> Result<(), String> {
        self.write_line(&format!(
            r#"{{"jsonrpc":"2.0","method":"{method}","params":{params_json}}}"#
        ))
    }

    /// Send a request and return the raw text of the matching response line.
    pub fn request(&mut self, method: &str, params_json: &str) -> Result<String, String> {
        self.next_id += 1;
        let id = self.next_id;
        self.write_line(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{params_json}}}"#
        ))?;
        loop {
            match self.rx.recv_timeout(REQUEST_TIMEOUT) {
                Ok(msg) if message_has_id(&msg, id) => return Ok(msg),
                Ok(_) => continue,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(format!(
                        "timed out after {}s waiting for `{method}` response (workspace may have failed to start)",
                        REQUEST_TIMEOUT.as_secs()
                    ));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(format!("mcp server exited before answering `{method}`"));
                }
            }
        }
    }

    /// MCP `initialize` + `notifications/initialized` handshake.
    pub fn initialize(&mut self) -> Result<(), String> {
        let params = r#"{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"beamtalk-uat","version":"0"}}"#;
        self.request("initialize", params)?;
        self.notify("notifications/initialized", "{}")
    }

    /// Call a tool: `tools/call` with `{ name, arguments }`.
    pub fn call_tool(&mut self, name: &str, arguments_json: &str) -> Result<String, String> {
        let params = format!(r#"{{"name":"{name}","arguments":{arguments_json}}}"#);
        self.request("tools/call", &params)
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Whether a response line carries the top-level `"id":<id>` we sent, with a
/// digit boundary so id `2` doesn't match a response for id `20`.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_match_is_digit_boundary_aware() {
        assert!(message_has_id(r#"{"jsonrpc":"2.0","id":3,"result":{}}"#, 3));
        assert!(message_has_id(r#"{"id": 3,"result":{}}"#, 3));
        assert!(!message_has_id(r#"{"id":30,"result":{}}"#, 3));
        assert!(!message_has_id(r#"{"method":"notifications/message"}"#, 3));
    }
}
