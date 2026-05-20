//! In-memory meeting room manager for real-time transcript streaming.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Message types sent over WebSocket to clients
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum OutboundMessage {
    #[serde(rename = "participant_joined")]
    ParticipantJoined {
        account_id: String,
        email: String,
        display_name: String,
    },

    #[serde(rename = "participant_left")]
    ParticipantLeft { account_id: String },

    #[serde(rename = "transcript_chunk")]
    TranscriptChunk {
        speaker_id: String,
        speaker_name: String,
        text: String,
        timestamp_ms: i64,
        chunk_index: i32,
    },

    #[serde(rename = "task_detected")]
    TaskDetected {
        task_id: String,
        title: String,
        assignee_id: Option<String>,
        assignee_name: Option<String>,
    },

    #[serde(rename = "summary_updated")]
    SummaryUpdated { summary: String },

    #[serde(rename = "meeting_ended")]
    MeetingEnded { meeting_id: String },
}

/// Message types received from WebSocket clients
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum InboundMessage {
    #[serde(rename = "transcript_chunk")]
    TranscriptChunk { text: String, timestamp_ms: i64 },

    #[serde(rename = "ping")]
    Ping,
}

/// Tracks the current open slot (1-5 minute time window) for AI processing.
struct SlotState {
    slot_index: i32,
    start_ms: i64,
    chunk_count: i32,
    word_count: i32,
    speaker_ids: HashSet<Uuid>,
    /// Timestamp of the last ingested chunk — used for silence detection.
    last_chunk_ms: i64,
    /// Database row ID for the open `meeting_slots` row.
    db_id: Uuid,
}

/// A connected participant in a room
struct Participant {
    account_id: Uuid,
    email: String,
    display_name: String,
    sender: mpsc::UnboundedSender<OutboundMessage>,
}

/// A single meeting room
struct MeetingRoom {
    meeting_id: Uuid,
    participants: HashMap<Uuid, Participant>,
    chunk_counter: i32,
    /// The currently open slot for AI processing, if any.
    current_slot: Option<SlotState>,
    /// Running slot index counter for this meeting.
    slot_counter: i32,
}

impl MeetingRoom {
    fn new(meeting_id: Uuid) -> Self {
        Self {
            meeting_id,
            participants: HashMap::new(),
            chunk_counter: 0,
            current_slot: None,
            slot_counter: 0,
        }
    }

    /// Broadcast a message to all participants EXCEPT the sender
    fn broadcast_except(&self, exclude: Uuid, msg: &OutboundMessage) {
        for (id, p) in &self.participants {
            if *id != exclude {
                let _ = p.sender.send(msg.clone());
            }
        }
    }

    /// Broadcast to ALL participants including sender
    fn broadcast_all(&self, msg: &OutboundMessage) {
        for p in self.participants.values() {
            let _ = p.sender.send(msg.clone());
        }
    }

    fn next_chunk_index(&mut self) -> i32 {
        self.chunk_counter += 1;
        self.chunk_counter
    }

    fn next_slot_index(&mut self) -> i32 {
        self.slot_counter += 1;
        self.slot_counter
    }
}

/// The global meeting hub — holds all active rooms
pub struct MeetingHub {
    rooms: RwLock<HashMap<Uuid, MeetingRoom>>,
}

/// Max slot duration in milliseconds (5 minutes).
const SLOT_MAX_MS: i64 = 300_000;

/// Minimum slot duration before silence can close it (1 minute).
const SLOT_MIN_MS: i64 = 60_000;

/// Silence threshold in milliseconds (3 seconds without a chunk).
const SILENCE_THRESHOLD_MS: i64 = 3_000;

/// How often the silence checker runs (5 seconds).
const SILENCE_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

impl MeetingHub {
    pub fn new(db: PgPool) -> Arc<Self> {
        let hub = Arc::new(Self {
            rooms: RwLock::new(HashMap::new()),
        });
        hub.start_silence_checker(db);
        hub
    }

    /// Add a participant to a meeting room. Returns a receiver for outbound messages.
    pub async fn join(
        &self,
        meeting_id: Uuid,
        account_id: Uuid,
        email: String,
        display_name: String,
        db: &PgPool,
    ) -> mpsc::UnboundedReceiver<OutboundMessage> {
        let (tx, rx) = mpsc::unbounded_channel();

        let mut rooms = self.rooms.write().await;
        let room = rooms
            .entry(meeting_id)
            .or_insert_with(|| MeetingRoom::new(meeting_id));

        // Broadcast join to existing participants
        let join_msg = OutboundMessage::ParticipantJoined {
            account_id: account_id.to_string(),
            email: email.clone(),
            display_name: display_name.clone(),
        };
        room.broadcast_all(&join_msg);

        room.participants.insert(
            account_id,
            Participant {
                account_id,
                email,
                display_name,
                sender: tx,
            },
        );

        // Update participant status in DB
        let _ = sqlx::query(
            "UPDATE meeting_participants SET status = 'joined', joined_at = now()
             WHERE meeting_id = $1 AND account_id = $2",
        )
        .bind(meeting_id)
        .bind(account_id)
        .execute(db)
        .await;

        info!(
            "[hub] {account_id} joined meeting {meeting_id} ({} participants)",
            room.participants.len()
        );

        rx
    }

    /// Remove a participant from a meeting room
    pub async fn leave(&self, meeting_id: Uuid, account_id: Uuid, db: &PgPool) {
        let mut rooms = self.rooms.write().await;

        if let Some(room) = rooms.get_mut(&meeting_id) {
            room.participants.remove(&account_id);

            let leave_msg = OutboundMessage::ParticipantLeft {
                account_id: account_id.to_string(),
            };
            room.broadcast_all(&leave_msg);

            info!(
                "[hub] {account_id} left meeting {meeting_id} ({} remaining)",
                room.participants.len()
            );

            // Clean up empty rooms
            if room.participants.is_empty() {
                rooms.remove(&meeting_id);
                debug!("[hub] room {meeting_id} removed (empty)");
            }
        }

        // Update participant status in DB
        let _ = sqlx::query(
            "UPDATE meeting_participants SET status = 'left', left_at = now()
             WHERE meeting_id = $1 AND account_id = $2",
        )
        .bind(meeting_id)
        .bind(account_id)
        .execute(db)
        .await;
    }

    /// Ingest a transcript chunk from a participant — persist to DB, update
    /// the current slot, and broadcast to other participants.
    pub async fn ingest_chunk(
        &self,
        meeting_id: Uuid,
        speaker_id: Uuid,
        speaker_name: String,
        text: String,
        timestamp_ms: i64,
        db: &PgPool,
    ) -> Option<OutboundMessage> {
        let word_count = text.split_whitespace().count() as i32;

        // --- Slot that needs closing (determined inside the lock, closed outside) ---
        let mut slot_to_close: Option<(Uuid, SlotState)> = None;

        let chunk_index = {
            let mut rooms = self.rooms.write().await;
            let Some(room) = rooms.get_mut(&meeting_id) else {
                warn!("[hub] ingest_chunk for unknown room {meeting_id}");
                return None;
            };

            let idx = room.next_chunk_index();

            // --- Open a new slot if none exists ---
            if room.current_slot.is_none() {
                let slot_index = room.next_slot_index();
                match Self::create_slot_row(meeting_id, slot_index, timestamp_ms, db).await {
                    Ok(db_id) => {
                        room.current_slot = Some(SlotState {
                            slot_index,
                            start_ms: timestamp_ms,
                            chunk_count: 0,
                            word_count: 0,
                            speaker_ids: HashSet::new(),
                            last_chunk_ms: timestamp_ms,
                            db_id,
                        });
                        debug!("[hub] opened slot {slot_index} for meeting {meeting_id}");
                    }
                    Err(e) => {
                        error!("[hub] failed to create slot row: {e}");
                        // Continue without slot tracking — the chunk still gets persisted
                    }
                }
            }

            // --- Update the current slot ---
            if let Some(ref mut slot) = room.current_slot {
                slot.chunk_count += 1;
                slot.word_count += word_count;
                slot.speaker_ids.insert(speaker_id);
                slot.last_chunk_ms = timestamp_ms;

                // --- Check max-cap boundary (5 min) ---
                let elapsed = timestamp_ms - slot.start_ms;
                if elapsed >= SLOT_MAX_MS {
                    // Take the slot out; we'll close it outside the lock
                    slot_to_close = Some((meeting_id, room.current_slot.take().unwrap()));
                }
            }

            // Broadcast to all OTHER participants (not the speaker)
            let msg = OutboundMessage::TranscriptChunk {
                speaker_id: speaker_id.to_string(),
                speaker_name: speaker_name.clone(),
                text: text.clone(),
                timestamp_ms,
                chunk_index: idx,
            };
            room.broadcast_except(speaker_id, &msg);

            idx
        };

        let chunk_msg = OutboundMessage::TranscriptChunk {
            speaker_id: speaker_id.to_string(),
            speaker_name,
            text: text.clone(),
            timestamp_ms,
            chunk_index,
        };

        // --- Persist chunk to DB (outside of lock) ---
        let _ = sqlx::query(
            "INSERT INTO transcript_chunks (meeting_id, speaker_id, text, chunk_index, timestamp_ms)
             VALUES ($1, $2, $3, $4, $5)"
        )
        .bind(meeting_id)
        .bind(speaker_id)
        .bind(&text)
        .bind(chunk_index)
        .bind(timestamp_ms)
        .execute(db)
        .await;

        // --- Update last_activity_at on the meeting ---
        let _ = sqlx::query("UPDATE meetings SET last_activity_at = now() WHERE id = $1")
            .bind(meeting_id)
            .execute(db)
            .await;

        // --- Close the slot outside of the lock if boundary was hit ---
        if let Some((_mid, slot)) = slot_to_close {
            Self::close_slot_row(&slot, meeting_id, timestamp_ms, db).await;
            info!(
                "[hub] closed slot {} for meeting {meeting_id} (max cap, {} chunks, {} words)",
                slot.slot_index, slot.chunk_count, slot.word_count
            );
        }

        Some(chunk_msg)
    }

    /// Broadcast a task detection to all participants.
    pub async fn notify_task(
        &self,
        meeting_id: Uuid,
        task_id: Uuid,
        title: String,
        assignee_id: Option<Uuid>,
        assignee_name: Option<String>,
    ) {
        let rooms = self.rooms.read().await;
        let Some(room) = rooms.get(&meeting_id) else {
            return;
        };

        let msg = OutboundMessage::TaskDetected {
            task_id: task_id.to_string(),
            title,
            assignee_id: assignee_id.map(|id| id.to_string()),
            assignee_name,
        };

        room.broadcast_all(&msg);
    }

    /// Broadcast an updated summary to all participants
    pub async fn broadcast_summary(&self, meeting_id: Uuid, summary: String) {
        let rooms = self.rooms.read().await;
        let Some(room) = rooms.get(&meeting_id) else {
            return;
        };

        let msg = OutboundMessage::SummaryUpdated { summary };
        room.broadcast_all(&msg);
    }

    /// Get the list of currently connected participant IDs for a meeting
    pub async fn connected_participants(&self, meeting_id: Uuid) -> Vec<Uuid> {
        let rooms = self.rooms.read().await;
        rooms
            .get(&meeting_id)
            .map(|r| r.participants.keys().copied().collect())
            .unwrap_or_default()
    }

    // ── Slot lifecycle ─────────────────────────────────────────────────

    /// Close the current open slot for a meeting. Updates the DB row with
    /// final counts and resets `current_slot` to `None`.
    pub async fn close_current_slot(&self, meeting_id: Uuid, db: &PgPool) {
        let slot = {
            let mut rooms = self.rooms.write().await;
            let Some(room) = rooms.get_mut(&meeting_id) else {
                return;
            };
            room.current_slot.take()
        };

        if let Some(slot) = slot {
            let now_ms = chrono::Utc::now().timestamp_millis();
            Self::close_slot_row(&slot, meeting_id, now_ms, db).await;
            info!(
                "[hub] closed slot {} for meeting {meeting_id} (explicit, {} chunks, {} words)",
                slot.slot_index, slot.chunk_count, slot.word_count
            );
        }
    }

    /// Create a `meeting_slots` row for a newly opened slot. Returns the
    /// generated row UUID.
    async fn create_slot_row(
        meeting_id: Uuid,
        slot_index: i32,
        start_ms: i64,
        db: &PgPool,
    ) -> Result<Uuid, sqlx::Error> {
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO meeting_slots (meeting_id, slot_index, start_ms, ai_status)
             VALUES ($1, $2, $3, 'pending')
             RETURNING id",
        )
        .bind(meeting_id)
        .bind(slot_index)
        .bind(start_ms)
        .fetch_one(db)
        .await?;

        Ok(id)
    }

    /// Update the DB row when a slot closes.
    async fn close_slot_row(slot: &SlotState, meeting_id: Uuid, end_ms: i64, db: &PgPool) {
        let speaker_vec: Vec<Uuid> = slot.speaker_ids.iter().copied().collect();
        let result = sqlx::query(
            "UPDATE meeting_slots
             SET end_ms = $1, chunk_count = $2, word_count = $3, speaker_ids = $4
             WHERE id = $5 AND meeting_id = $6",
        )
        .bind(end_ms)
        .bind(slot.chunk_count)
        .bind(slot.word_count)
        .bind(&speaker_vec)
        .bind(slot.db_id)
        .bind(meeting_id)
        .execute(db)
        .await;

        if let Err(e) = result {
            error!("[hub] failed to close slot row {}: {e}", slot.db_id);
        }
    }

    // ── Silence checker ────────────────────────────────────────────────

    /// Spawn a background task that periodically checks for silence in each
    /// room. If no chunk has arrived for `SILENCE_THRESHOLD_MS` and the slot
    /// has been open for at least `SLOT_MIN_MS`, the slot is closed.
    fn start_silence_checker(self: &Arc<Self>, db: PgPool) {
        let hub = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(SILENCE_CHECK_INTERVAL);
            loop {
                interval.tick().await;

                let now_ms = chrono::Utc::now().timestamp_millis();

                // Collect slots that need closing while holding the write lock
                // as briefly as possible.
                let slots_to_close: Vec<(Uuid, SlotState)> = {
                    let mut rooms = hub.rooms.write().await;
                    let mut closing = Vec::new();

                    for room in rooms.values_mut() {
                        let should_close = room.current_slot.as_ref().is_some_and(|slot| {
                            let silence = now_ms - slot.last_chunk_ms;
                            let duration = now_ms - slot.start_ms;
                            silence > SILENCE_THRESHOLD_MS && duration >= SLOT_MIN_MS
                        });
                        if should_close {
                            let slot = room.current_slot.take().unwrap();
                            closing.push((room.meeting_id, slot));
                        }
                    }

                    closing
                };

                // Close each slot outside the lock
                for (meeting_id, slot) in &slots_to_close {
                    Self::close_slot_row(slot, *meeting_id, now_ms, &db).await;
                    info!(
                        "[hub] closed slot {} for meeting {meeting_id} (silence, {} chunks, {} words)",
                        slot.slot_index, slot.chunk_count, slot.word_count
                    );
                }
            }
        });
    }

    // ── Meeting end ────────────────────────────────────────────────────

    /// End a meeting: broadcast `MeetingEnded` to all participants, close any
    /// open slot, and remove the room.
    pub async fn broadcast_meeting_end(&self, meeting_id: Uuid, db: &PgPool) {
        let (participants_exist, slot) = {
            let mut rooms = self.rooms.write().await;
            let Some(room) = rooms.get_mut(&meeting_id) else {
                return;
            };

            // Broadcast to everyone before we tear down
            let msg = OutboundMessage::MeetingEnded {
                meeting_id: meeting_id.to_string(),
            };
            room.broadcast_all(&msg);

            // Take the open slot (if any) so we can close it outside the lock
            let slot = room.current_slot.take();
            let had_participants = !room.participants.is_empty();

            // Remove the room
            rooms.remove(&meeting_id);
            debug!("[hub] room {meeting_id} removed (meeting ended)");

            (had_participants, slot)
        };

        // Close the slot outside the lock
        if let Some(slot) = slot {
            let now_ms = chrono::Utc::now().timestamp_millis();
            Self::close_slot_row(&slot, meeting_id, now_ms, db).await;
            info!(
                "[hub] closed slot {} for meeting {meeting_id} (meeting ended, {} chunks, {} words)",
                slot.slot_index, slot.chunk_count, slot.word_count
            );
        }

        if participants_exist {
            info!("[hub] meeting {meeting_id} ended — all participants notified");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn assigned_task_is_broadcast_once_per_participant() {
        let hub = MeetingHub {
            rooms: RwLock::new(HashMap::new()),
        };
        let meeting_id = Uuid::new_v4();
        let assignee_id = Uuid::new_v4();
        let observer_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();
        let (assignee_tx, mut assignee_rx) = mpsc::unbounded_channel();
        let (observer_tx, mut observer_rx) = mpsc::unbounded_channel();

        let mut room = MeetingRoom::new(meeting_id);
        room.participants.insert(
            assignee_id,
            Participant {
                account_id: assignee_id,
                email: "assignee@example.com".to_string(),
                display_name: "Assignee".to_string(),
                sender: assignee_tx,
            },
        );
        room.participants.insert(
            observer_id,
            Participant {
                account_id: observer_id,
                email: "observer@example.com".to_string(),
                display_name: "Observer".to_string(),
                sender: observer_tx,
            },
        );
        hub.rooms.write().await.insert(meeting_id, room);

        hub.notify_task(
            meeting_id,
            task_id,
            "Follow up".to_string(),
            Some(assignee_id),
            Some("Assignee".to_string()),
        )
        .await;

        let assignee_msg = assignee_rx.try_recv().expect("assignee gets task");
        let observer_msg = observer_rx.try_recv().expect("observer gets task");

        for msg in [assignee_msg, observer_msg] {
            match msg {
                OutboundMessage::TaskDetected {
                    task_id: actual_task_id,
                    title,
                    assignee_id: actual_assignee_id,
                    assignee_name,
                } => {
                    assert_eq!(actual_task_id, task_id.to_string());
                    assert_eq!(title, "Follow up");
                    assert_eq!(actual_assignee_id, Some(assignee_id.to_string()));
                    assert_eq!(assignee_name, Some("Assignee".to_string()));
                }
                other => panic!("expected task_detected, got {other:?}"),
            }
        }

        assert!(
            assignee_rx.try_recv().is_err(),
            "assignee received duplicate task"
        );
        assert!(
            observer_rx.try_recv().is_err(),
            "observer received duplicate task"
        );
    }
}
