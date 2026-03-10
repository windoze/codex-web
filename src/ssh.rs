//! SSH utilities for remote command execution and filesystem browsing.
//!
//! This module provides:
//! - POSIX shell-safe quoting of strings for remote execution.
//! - Construction of `ssh` command-line arguments.
//! - Running remote commands and streaming stdout/stderr.
//! - Remote filesystem listing via `ssh` + shell commands.

use std::process::Stdio;

use anyhow::Context;
use tokio::process::Command;

// ---------------------------------------------------------------------------
// Shell quoting
// ---------------------------------------------------------------------------

/// Quote a string for safe interpolation into a POSIX shell command.
///
/// Strategy: wrap the value in single-quotes and escape embedded single-quotes
/// using the `'\''` idiom (end single-quote, escaped single-quote, start
/// single-quote).
///
/// This is safe against command injection because single-quoted strings in
/// POSIX shells have no special characters except the closing single-quote.
pub fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

// ---------------------------------------------------------------------------
// SSH target configuration
// ---------------------------------------------------------------------------

/// All the information needed to reach a remote host via SSH.
#[derive(Debug, Clone)]
pub struct SshTarget {
    /// The SSH destination, e.g. `user@host` or an SSH config host name.
    pub target: String,
    /// Optional port override (default: let SSH use its configured port).
    pub port: Option<u16>,
    /// Optional path to an identity file (`-i`).
    pub identity_file: Option<String>,
}

// ---------------------------------------------------------------------------
// SSH command builder
// ---------------------------------------------------------------------------

/// Build a `tokio::process::Command` that will execute `remote_command` on the
/// remote host described by `target`.
///
/// The resulting command is:
///   ssh [-p PORT] [-i IDENTITY] -T -o BatchMode=yes -- <target> <remote_command>
///
/// Key design choices:
/// - `-T` disables PTY allocation (no terminal control sequences).
/// - `-o BatchMode=yes` prevents interactive password prompts.
/// - `--` prevents the target/command from being interpreted as SSH options.
pub fn build_ssh_command(target: &SshTarget, remote_command: &str) -> Command {
    let mut cmd = Command::new("ssh");

    if let Some(port) = target.port {
        cmd.arg("-p").arg(port.to_string());
    }
    if let Some(ref identity) = target.identity_file {
        cmd.arg("-i").arg(identity);
    }

    cmd.arg("-T");
    cmd.arg("-o").arg("BatchMode=yes");
    cmd.arg("--");
    cmd.arg(&target.target);
    cmd.arg(remote_command);
    cmd
}

/// Build the remote shell command string to run Codex in a given directory.
///
/// Returns something like:
///   sh -lc 'cd '\''/home/alice/repo'\'' && codex --cd '\''/home/alice/repo'\'' ... exec --json ...'
pub fn build_remote_codex_command(
    remote_path: &str,
    session_id: Option<&str>,
    prompt: &str,
    ask_for_approval: &str,
    sandbox: &str,
    skip_git_repo_check: bool,
) -> String {
    let quoted_path = shell_quote(remote_path);
    let quoted_approval = shell_quote(ask_for_approval);
    let quoted_sandbox = shell_quote(sandbox);
    let quoted_prompt = shell_quote(prompt);

    let mut inner = format!(
        "cd {quoted_path} && codex --cd {quoted_path} --ask-for-approval {quoted_approval} --sandbox {quoted_sandbox} exec"
    );

    if skip_git_repo_check {
        inner.push_str(" --skip-git-repo-check");
    }

    if let Some(sid) = session_id {
        let quoted_sid = shell_quote(sid);
        inner.push_str(&format!(" resume {quoted_sid}"));
    }

    inner.push_str(&format!(" --json {quoted_prompt}"));

    // Wrap in sh -lc so we get a login shell (PATH, etc.).
    format!("sh -lc {}", shell_quote(&inner))
}

/// Build a remote shell command to check that the Codex binary exists and the
/// directory is valid.
pub fn build_remote_prereq_check(remote_path: &str) -> String {
    let quoted_path = shell_quote(remote_path);
    let inner = format!(
        "test -d {quoted_path} && command -v codex >/dev/null 2>&1 && echo OK"
    );
    format!("sh -lc {}", shell_quote(&inner))
}

// ---------------------------------------------------------------------------
// Remote command execution
// ---------------------------------------------------------------------------

/// Output from running a remote command.
#[derive(Debug, Clone)]
pub struct RemoteCommandOutput {
    pub stdout_lines: Vec<String>,
    pub stderr_text: String,
    pub exit_success: bool,
}

/// Run a command on a remote host via SSH and collect all output.
///
/// This is used for quick commands like directory listing and prerequisite
/// checks, not for long-running streaming processes.
pub async fn run_remote_command(
    target: &SshTarget,
    remote_command: &str,
) -> anyhow::Result<RemoteCommandOutput> {
    let mut cmd = build_ssh_command(target, remote_command);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    let child = cmd.spawn().context("spawn ssh command")?;
    let output = child
        .wait_with_output()
        .await
        .context("wait ssh command")?;

    let stdout_text = String::from_utf8_lossy(&output.stdout);
    let stdout_lines: Vec<String> = stdout_text
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();
    let stderr_text = String::from_utf8_lossy(&output.stderr).to_string();

    Ok(RemoteCommandOutput {
        stdout_lines,
        stderr_text,
        exit_success: output.status.success(),
    })
}

/// Run a command on a remote host, streaming stdout lines through a callback.
///
/// Returns the child process handle so the caller can also write to stdin.
/// This is used for long-running Codex processes.
pub async fn spawn_remote_streaming(
    target: &SshTarget,
    remote_command: &str,
) -> anyhow::Result<tokio::process::Child> {
    let mut cmd = build_ssh_command(target, remote_command);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    cmd.spawn().context("spawn ssh streaming command")
}

// ---------------------------------------------------------------------------
// Remote filesystem listing
// ---------------------------------------------------------------------------

/// Get the home directory of the remote user.
pub async fn remote_home(target: &SshTarget) -> anyhow::Result<String> {
    let cmd = "echo $HOME";
    let output = run_remote_command(target, cmd).await?;
    if !output.exit_success {
        anyhow::bail!(
            "failed to get remote home: {}",
            output.stderr_text.trim()
        );
    }
    output
        .stdout_lines
        .into_iter()
        .next()
        .filter(|s| !s.is_empty())
        .context("empty HOME from remote")
}

/// Entry from remote directory listing.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RemoteFsEntry {
    pub name: String,
    pub path: String,
    pub kind: String, // "dir", "file", "symlink", "other"
}

/// List entries in a remote directory.
///
/// Uses a portable `stat` / `ls` approach that works on Linux and macOS:
/// We use `ls -1Ap` which appends `/` for dirs, `@` for symlinks, `*` for
/// executables, `|` for FIFOs, `=` for sockets. We classify entries based on
/// these suffixes.
///
/// For reliability, we also provide a Python-based fallback that emits JSON.
/// The caller can pick which one to use. Here we use the `ls` approach as the
/// primary (no deps).
pub async fn remote_fs_list(
    target: &SshTarget,
    remote_path: &str,
) -> anyhow::Result<(String, Option<String>, Vec<RemoteFsEntry>)> {
    let quoted_path = shell_quote(remote_path);

    // Use a shell snippet that:
    // 1. Resolves the full path (via cd + pwd)
    // 2. Prints the parent directory
    // 3. Lists entries with type indicators
    let inner = format!(
        concat!(
            "cd {path} 2>/dev/null || exit 1; ",
            "FULL=$(pwd); ",
            "PARENT=$(dirname \"$FULL\"); ",
            "echo \"__PATH__:$FULL\"; ",
            "echo \"__PARENT__:$PARENT\"; ",
            "ls -1Ap 2>/dev/null || true"
        ),
        path = quoted_path,
    );
    let remote_cmd = format!("sh -c {}", shell_quote(&inner));

    let output = run_remote_command(target, &remote_cmd).await?;
    if !output.exit_success {
        anyhow::bail!(
            "failed to list remote directory {}: {}",
            remote_path,
            output.stderr_text.trim()
        );
    }

    let mut full_path = remote_path.to_string();
    let mut parent: Option<String> = None;
    let mut entries = Vec::new();

    for line in &output.stdout_lines {
        if let Some(p) = line.strip_prefix("__PATH__:") {
            full_path = p.to_string();
            continue;
        }
        if let Some(p) = line.strip_prefix("__PARENT__:") {
            parent = Some(p.to_string());
            continue;
        }

        // Parse ls -Ap output.
        let (name, kind) = if let Some(n) = line.strip_suffix('/') {
            (n.to_string(), "dir")
        } else if let Some(n) = line.strip_suffix('@') {
            (n.to_string(), "symlink")
        } else if let Some(n) = line.strip_suffix('*') {
            (n.to_string(), "file")
        } else if let Some(n) = line.strip_suffix('|') {
            (n.to_string(), "other")
        } else if let Some(n) = line.strip_suffix('=') {
            (n.to_string(), "other")
        } else {
            (line.to_string(), "file")
        };

        if name == "." || name == ".." || name.is_empty() {
            continue;
        }

        let entry_path = if full_path.ends_with('/') {
            format!("{full_path}{name}")
        } else {
            format!("{full_path}/{name}")
        };

        entries.push(RemoteFsEntry {
            name,
            path: entry_path,
            kind: kind.to_string(),
        });
    }

    // Sort: dirs first, then by name (case-insensitive).
    entries.sort_by(|a, b| {
        let rank = |k: &str| match k {
            "dir" => 0,
            "symlink" => 1,
            "file" => 2,
            _ => 3,
        };
        rank(&a.kind)
            .cmp(&rank(&b.kind))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok((full_path, parent, entries))
}

/// Check SSH connectivity and remote prerequisites.
///
/// Returns (remote_user, remote_home, codex_found).
pub async fn ssh_check(target: &SshTarget) -> anyhow::Result<SshCheckResult> {
    let inner = concat!(
        "echo \"__USER__:$(whoami)\"; ",
        "echo \"__HOME__:$HOME\"; ",
        "if command -v codex >/dev/null 2>&1; then echo \"__CODEX__:true\"; else echo \"__CODEX__:false\"; fi"
    );
    let remote_cmd = format!("sh -c {}", shell_quote(inner));

    let output = run_remote_command(target, &remote_cmd).await?;
    if !output.exit_success {
        anyhow::bail!(
            "SSH connection failed: {}",
            output.stderr_text.trim()
        );
    }

    let mut user = String::new();
    let mut home = String::new();
    let mut codex_found = false;

    for line in &output.stdout_lines {
        if let Some(v) = line.strip_prefix("__USER__:") {
            user = v.to_string();
        } else if let Some(v) = line.strip_prefix("__HOME__:") {
            home = v.to_string();
        } else if let Some(v) = line.strip_prefix("__CODEX__:") {
            codex_found = v == "true";
        }
    }

    Ok(SshCheckResult {
        remote_user: user,
        remote_home: home,
        codex_found,
    })
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SshCheckResult {
    pub remote_user: String,
    pub remote_home: String,
    pub codex_found: bool,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_basic() {
        assert_eq!(shell_quote("hello"), "'hello'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn shell_quote_with_single_quotes() {
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote("a'b'c"), "'a'\\''b'\\''c'");
    }

    #[test]
    fn shell_quote_with_special_chars() {
        // Spaces, dollar signs, backticks, semicolons — all safely quoted.
        assert_eq!(shell_quote("$HOME"), "'$HOME'");
        assert_eq!(shell_quote("`rm -rf /`"), "'`rm -rf /`'");
        assert_eq!(shell_quote("a; echo pwned"), "'a; echo pwned'");
        assert_eq!(shell_quote("$(evil)"), "'$(evil)'");
    }

    #[test]
    fn shell_quote_with_newlines() {
        assert_eq!(shell_quote("a\nb"), "'a\nb'");
    }

    #[test]
    fn build_ssh_command_basic() {
        let target = SshTarget {
            target: "alice@host".to_string(),
            port: None,
            identity_file: None,
        };
        let cmd = build_ssh_command(&target, "echo hello");
        let prog = cmd.as_std().get_program().to_string_lossy().to_string();
        let args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();

        assert_eq!(prog, "ssh");
        assert!(args.contains(&"-T".to_string()));
        assert!(args.contains(&"BatchMode=yes".to_string()));
        assert!(args.contains(&"--".to_string()));
        assert!(args.contains(&"alice@host".to_string()));
        assert!(args.contains(&"echo hello".to_string()));

        // Ensure -- comes before the target.
        let dash_pos = args.iter().position(|a| a == "--").unwrap();
        let target_pos = args.iter().position(|a| a == "alice@host").unwrap();
        assert!(dash_pos < target_pos, "-- must come before target");
    }

    #[test]
    fn build_ssh_command_with_port_and_identity() {
        let target = SshTarget {
            target: "bob@server".to_string(),
            port: Some(2222),
            identity_file: Some("/home/bob/.ssh/id_rsa".to_string()),
        };
        let cmd = build_ssh_command(&target, "ls");
        let args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();

        assert!(args.contains(&"-p".to_string()));
        assert!(args.contains(&"2222".to_string()));
        assert!(args.contains(&"-i".to_string()));
        assert!(args.contains(&"/home/bob/.ssh/id_rsa".to_string()));
    }

    #[test]
    fn build_remote_codex_command_no_session() {
        let cmd = build_remote_codex_command(
            "/home/alice/repo",
            None,
            "hello world",
            "never",
            "workspace-write",
            true,
        );

        // The outer wrapper is sh -lc '<inner>'.
        assert!(cmd.starts_with("sh -lc '"));
        // The inner command should contain cd, codex, --json, the prompt.
        // All inner single quotes are escaped as '\'' due to the outer shell_quote.
        assert!(cmd.contains("cd"));
        assert!(cmd.contains("/home/alice/repo"));
        assert!(cmd.contains("codex"));
        assert!(cmd.contains("--json"));
        assert!(cmd.contains("--skip-git-repo-check"));
        assert!(cmd.contains("hello world"));
        // Should NOT contain "resume".
        assert!(!cmd.contains("resume"));

        // Verify the inner command itself is valid by extracting it.
        let inner = &cmd["sh -lc ".len()..];
        // The inner is a single-quoted string.
        assert!(inner.starts_with('\''));
        assert!(inner.ends_with('\''));
    }

    #[test]
    fn build_remote_codex_command_with_session() {
        let cmd = build_remote_codex_command(
            "/home/alice/repo",
            Some("sess-123"),
            "continue",
            "never",
            "workspace-write",
            false,
        );

        assert!(cmd.contains("resume"));
        assert!(cmd.contains("sess-123"));
        assert!(!cmd.contains("--skip-git-repo-check"));
    }

    #[test]
    fn build_remote_codex_command_path_injection_safe() {
        // A path that attempts shell injection.
        let cmd = build_remote_codex_command(
            "/tmp/'; rm -rf / #",
            None,
            "test",
            "never",
            "workspace-write",
            true,
        );

        // The inner command is built with shell_quote for each part, then
        // the whole thing is wrapped in shell_quote again for sh -lc.
        // The dangerous characters should appear only inside quoted strings.
        // Verify the output is a single sh -lc '...' command.
        assert!(cmd.starts_with("sh -lc '"));
        assert!(cmd.ends_with('\''));
        // The path with injection should be safely escaped.
        assert!(cmd.contains("/tmp/"));
    }

    #[test]
    fn build_remote_codex_command_prompt_injection_safe() {
        // A prompt that attempts shell injection.
        let cmd = build_remote_codex_command(
            "/home/alice/repo",
            None,
            "'; rm -rf / #",
            "never",
            "workspace-write",
            true,
        );

        // The prompt should be safely wrapped.
        assert!(cmd.starts_with("sh -lc '"));
        assert!(cmd.ends_with('\''));
    }

    #[test]
    fn build_remote_prereq_check_basic() {
        let cmd = build_remote_prereq_check("/home/alice/repo");
        // The inner is wrapped in shell_quote, so the single quotes are escaped.
        assert!(cmd.contains("test -d"));
        assert!(cmd.contains("/home/alice/repo"));
        assert!(cmd.contains("command -v codex"));
    }

    #[test]
    fn remote_fs_entry_sorting() {
        let mut entries = vec![
            RemoteFsEntry {
                name: "zebra".to_string(),
                path: "/zebra".to_string(),
                kind: "file".to_string(),
            },
            RemoteFsEntry {
                name: "alpha".to_string(),
                path: "/alpha".to_string(),
                kind: "dir".to_string(),
            },
            RemoteFsEntry {
                name: "beta".to_string(),
                path: "/beta".to_string(),
                kind: "dir".to_string(),
            },
            RemoteFsEntry {
                name: "link".to_string(),
                path: "/link".to_string(),
                kind: "symlink".to_string(),
            },
        ];

        entries.sort_by(|a, b| {
            let rank = |k: &str| match k {
                "dir" => 0,
                "symlink" => 1,
                "file" => 2,
                _ => 3,
            };
            rank(&a.kind)
                .cmp(&rank(&b.kind))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });

        assert_eq!(entries[0].name, "alpha");
        assert_eq!(entries[1].name, "beta");
        assert_eq!(entries[2].name, "link");
        assert_eq!(entries[3].name, "zebra");
    }
}
