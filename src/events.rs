use serde_json::Value;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::db::{ConversationEvent, Db};

pub async fn emit(
    db: &Db,
    tx: &broadcast::Sender<ConversationEvent>,
    conversation_id: Uuid,
    event_type: &str,
    payload: &Value,
) -> anyhow::Result<ConversationEvent> {
    let event = db
        .append_event(conversation_id, event_type, payload)
        .await?;
    let _ = tx.send(event.clone());
    Ok(event)
}

