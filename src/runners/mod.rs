use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use tokio::sync::broadcast;
use uuid::Uuid;

use crate::db::{ConversationEvent, Db};
use crate::tool::ToolKind;

pub mod claude;
pub mod codex;

#[derive(Debug, Clone)]
pub struct RunnerTurnContext {
    pub db: Db,
    pub event_tx: broadcast::Sender<ConversationEvent>,
    pub conversation_id: Uuid,
    pub project_root: std::path::PathBuf,
    pub tool_session_id: Option<String>,
    pub prompt: String,
    pub ws_clients: Arc<AtomicUsize>,
    pub interaction_timeout_ms: i64,
    pub interaction_default_action: String,
}

#[derive(Debug, Clone)]
pub struct RunnerOutcome {
    pub tool_session_id: Option<String>,
}

pub trait Runner: Send + Sync {
    fn tool(&self) -> ToolKind;

    fn run_turn<'a>(
        &'a self,
        ctx: RunnerTurnContext,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<RunnerOutcome>> + Send + 'a>>;
}

#[derive(Clone)]
pub struct RunnerSet {
    codex: Arc<codex::CodexRunner>,
    claude: Arc<claude::ClaudeRunner>,
}

impl RunnerSet {
    pub fn new(
        codex_runtime: crate::codex::CodexRuntime,
        claude_runtime: crate::claude::ClaudeRuntime,
    ) -> Self {
        Self {
            codex: Arc::new(codex::CodexRunner { runtime: codex_runtime }),
            claude: Arc::new(claude::ClaudeRunner {
                runtime: claude_runtime,
            }),
        }
    }

    pub fn for_tool(&self, tool: ToolKind) -> Arc<dyn Runner> {
        match tool {
            ToolKind::Codex => self.codex.clone(),
            ToolKind::ClaudeCode => self.claude.clone(),
        }
    }
}
