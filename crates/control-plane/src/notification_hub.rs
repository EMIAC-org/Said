use std::{collections::HashMap, sync::Arc};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{RwLock, mpsc};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopNotification {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub payload: Value,
}

struct AccountRoom {
    senders: Vec<mpsc::UnboundedSender<DesktopNotification>>,
}

#[derive(Default)]
pub struct NotificationHub {
    rooms: RwLock<HashMap<Uuid, AccountRoom>>,
}

impl NotificationHub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub async fn join(&self, account_id: Uuid) -> mpsc::UnboundedReceiver<DesktopNotification> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut rooms = self.rooms.write().await;
        rooms
            .entry(account_id)
            .or_insert_with(|| AccountRoom {
                senders: Vec::new(),
            })
            .senders
            .push(tx);
        rx
    }

    pub async fn emit(&self, account_id: Uuid, notification: DesktopNotification) {
        let mut rooms = self.rooms.write().await;
        let Some(room) = rooms.get_mut(&account_id) else {
            return;
        };
        room.senders
            .retain(|sender| sender.send(notification.clone()).is_ok());
        if room.senders.is_empty() {
            rooms.remove(&account_id);
        }
    }
}
