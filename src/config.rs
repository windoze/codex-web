use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use directories::BaseDirs;
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(
    name = "codex-web",
    version,
    about = "Local web UI + daemon for Codex sessions"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the local codex-web daemon (HTTP + WebSocket)
    Serve(ServeArgs),
    /// List/respond to pending interaction requests via the daemon HTTP API
    Interactions(InteractionsArgs),
}

#[derive(Debug, Args)]
pub struct ServeArgs {
    /// Address to listen on (host:port)
    #[arg(long, env = "CODEX_WEB_LISTEN", default_value = "127.0.0.1:8787")]
    pub listen: SocketAddr,

    /// Path to the SQLite DB file
    #[arg(long, env = "CODEX_WEB_DB_PATH")]
    pub db_path: Option<PathBuf>,

    /// Directory to serve as static web assets (optional; for production builds)
    #[arg(long, env = "CODEX_WEB_STATIC_DIR")]
    pub static_dir: Option<PathBuf>,

    /// Optional bearer token required for API requests (sent as `Authorization: Bearer <token>`)
    #[arg(long, env = "CODEX_WEB_AUTH_TOKEN")]
    pub auth_token: Option<String>,

    /// Default timeout (ms) for interaction requests before auto-responding
    #[arg(
        long,
        env = "CODEX_WEB_INTERACTION_TIMEOUT_MS",
        default_value_t = 30_000
    )]
    pub interaction_timeout_ms: i64,

    /// Default action to take when the user is away (e.g., "decline" or "accept")
    #[arg(
        long,
        env = "CODEX_WEB_INTERACTION_DEFAULT_ACTION",
        default_value = "decline"
    )]
    pub interaction_default_action: String,

    /// Codex CLI approval policy (passed to `codex --ask-for-approval`)
    #[arg(long, env = "CODEX_WEB_CODEX_APPROVAL_POLICY", default_value = "never")]
    pub codex_ask_for_approval: String,

    /// Codex CLI sandbox policy (passed to `codex --sandbox`)
    #[arg(
        long,
        env = "CODEX_WEB_CODEX_SANDBOX",
        default_value = "workspace-write"
    )]
    pub codex_sandbox: String,

    /// Claude Code binary to execute (or a bridge wrapper that implements the JSONL contract)
    #[arg(
        long,
        env = "CODEX_WEB_CLAUDE_CODE_BIN",
        default_value = "claude-code"
    )]
    pub claude_code_bin: String,

    /// Additional arguments passed to the Claude Code binary.
    ///
    /// This is a simple whitespace-delimited list (no shell parsing/quoting).
    #[arg(long, env = "CODEX_WEB_CLAUDE_CODE_ARGS", value_delimiter = ' ')]
    pub claude_code_args: Vec<String>,

    /// Maximum number of Codex turns to run concurrently across all conversations
    #[arg(long, env = "CODEX_WEB_MAX_CONCURRENT_RUNS", default_value_t = 2)]
    pub max_concurrent_runs: usize,

    /// Optional shell command to run when a Codex turn finishes (completed/failed/aborted).
    ///
    /// The command is executed on the same machine as the daemon, with `cwd` set to the project root.
    /// Environment variables are provided:
    /// - CODEX_WEB_CONVERSATION_ID
    /// - CODEX_WEB_PROJECT_ROOT
    /// - CODEX_WEB_RUN_STATUS
    /// - CODEX_WEB_CODEX_SESSION_ID (may be empty)
    #[arg(long, env = "CODEX_WEB_ON_TURN_FINISHED_COMMAND")]
    pub on_turn_finished_command: Option<String>,
}

#[derive(Debug, Args)]
pub struct InteractionsArgs {
    /// Base URL for the codex-web daemon
    #[arg(
        long,
        env = "CODEX_WEB_DAEMON",
        default_value = "http://127.0.0.1:8787"
    )]
    pub daemon: String,

    /// Bearer token for authenticating to the daemon API (if configured)
    #[arg(long, env = "CODEX_WEB_AUTH_TOKEN")]
    pub auth_token: Option<String>,

    #[command(subcommand)]
    pub command: InteractionsCommand,
}

#[derive(Debug, Subcommand)]
pub enum InteractionsCommand {
    /// List pending interaction requests
    List {
        /// Optional conversation id to filter by
        #[arg(long)]
        conversation_id: Option<Uuid>,
    },
    /// Respond to an interaction request
    Respond {
        /// Interaction request id
        interaction_id: Uuid,

        /// Action to take (e.g. "accept" or "decline")
        #[arg(long)]
        action: String,

        /// Optional free-form text input (used for elicitation requests)
        #[arg(long)]
        text: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub db_path: PathBuf,
    pub static_dir: Option<PathBuf>,
    pub auth_token: Option<String>,
    pub interaction_timeout_ms: i64,
    pub interaction_default_action: String,
    pub codex_ask_for_approval: String,
    pub codex_sandbox: String,
    pub claude_code_bin: String,
    pub claude_code_args: Vec<String>,
    pub max_concurrent_runs: usize,
    pub on_turn_finished_command: Option<String>,
}

impl Config {
    pub fn from_serve_args(args: ServeArgs) -> anyhow::Result<Self> {
        let db_path = match args.db_path {
            Some(path) => path,
            None => default_db_path().context("resolve default db path")?,
        };

        Ok(Self {
            listen: args.listen,
            db_path,
            static_dir: args.static_dir,
            auth_token: args.auth_token.filter(|t| !t.trim().is_empty()),
            interaction_timeout_ms: args.interaction_timeout_ms,
            interaction_default_action: args.interaction_default_action,
            codex_ask_for_approval: args.codex_ask_for_approval,
            codex_sandbox: args.codex_sandbox,
            claude_code_bin: args.claude_code_bin,
            claude_code_args: args.claude_code_args,
            max_concurrent_runs: args.max_concurrent_runs,
            on_turn_finished_command: args
                .on_turn_finished_command
                .filter(|cmd| !cmd.trim().is_empty()),
        })
    }

    pub fn db_dir(&self) -> anyhow::Result<&Path> {
        self.db_path
            .parent()
            .context("db_path has no parent directory")
    }
}

fn default_db_path() -> anyhow::Result<PathBuf> {
    let base = BaseDirs::new().context("unable to resolve user home directory")?;
    Ok(base.home_dir().join(".codex-web").join("codex-web.sqlite"))
}
