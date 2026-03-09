use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolKind {
    Codex,
    ClaudeCode,
}

impl ToolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ToolKind::Codex => "codex",
            ToolKind::ClaudeCode => "claude-code",
        }
    }
}

impl Default for ToolKind {
    fn default() -> Self {
        ToolKind::Codex
    }
}

impl fmt::Display for ToolKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ToolKind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "codex" => Ok(ToolKind::Codex),
            "claude-code" => Ok(ToolKind::ClaudeCode),
            other => Err(anyhow::anyhow!("unknown tool: {other}")),
        }
    }
}

