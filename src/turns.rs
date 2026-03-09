use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::watch;
use uuid::Uuid;

#[derive(Clone, Default)]
pub struct TurnManager {
    inner: Arc<Mutex<HashMap<Uuid, watch::Sender<bool>>>>,
}

impl TurnManager {
    pub fn register(&self, conversation_id: Uuid) -> watch::Receiver<bool> {
        let (tx, rx) = watch::channel(false);
        let mut inner = self
            .inner
            .lock()
            .expect("turn manager mutex poisoned");
        inner.insert(conversation_id, tx);
        rx
    }

    pub fn cancel(&self, conversation_id: Uuid) -> bool {
        let inner = self
            .inner
            .lock()
            .expect("turn manager mutex poisoned");
        let Some(tx) = inner.get(&conversation_id) else {
            return false;
        };
        let _ = tx.send(true);
        true
    }

    pub fn unregister(&self, conversation_id: Uuid) {
        let mut inner = self
            .inner
            .lock()
            .expect("turn manager mutex poisoned");
        inner.remove(&conversation_id);
    }
}

