use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use directories::BaseDirs;

#[derive(Debug, Parser)]
#[command(name = "codex-web", version, about = "Local web UI + daemon for Codex sessions")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the local codex-web daemon (HTTP + WebSocket)
    Serve(ServeArgs),
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
}

#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub db_path: PathBuf,
    pub static_dir: Option<PathBuf>,
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

