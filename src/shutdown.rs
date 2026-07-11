use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::sync::Notify;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferKind {
    Download,
    Upload,
}

impl TransferKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Download => "download",
            Self::Upload => "upload",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ActiveTransfer {
    pub transfer_id: Uuid,
    pub request_id: String,
    pub kind: TransferKind,
    pub fields: BTreeMap<String, Value>,
    forced: bool,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct TransferTotals {
    pub completed: u64,
    pub aborted: u64,
}

#[derive(Debug, Default)]
pub struct ShutdownState {
    draining: AtomicBool,
    active: Mutex<BTreeMap<Uuid, ActiveTransfer>>,
    completed: AtomicU64,
    aborted: AtomicU64,
    changed: Notify,
    drain_started: Notify,
}

impl ShutdownState {
    pub fn begin_draining(&self) -> bool {
        if self
            .draining
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            self.drain_started.notify_waiters();
            true
        } else {
            false
        }
    }

    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }

    pub async fn draining_notified(&self) {
        if self.is_draining() {
            return;
        }
        let notified = self.drain_started.notified();
        if self.is_draining() {
            return;
        }
        notified.await;
    }

    pub fn start_transfer(
        self: &Arc<Self>,
        kind: TransferKind,
        request_id: String,
        fields: BTreeMap<String, Value>,
    ) -> TransferGuard {
        let transfer_id = Uuid::new_v4();
        self.active
            .lock()
            .expect("transfer tracker poisoned")
            .insert(
                transfer_id,
                ActiveTransfer {
                    transfer_id,
                    request_id,
                    kind,
                    fields,
                    forced: false,
                },
            );
        self.changed.notify_waiters();
        TransferGuard {
            state: self.clone(),
            transfer_id,
        }
    }

    pub fn active_count(&self) -> usize {
        self.active.lock().expect("transfer tracker poisoned").len()
    }

    pub fn mark_active_forced(&self) -> Vec<ActiveTransfer> {
        let mut active = self.active.lock().expect("transfer tracker poisoned");
        for transfer in active.values_mut() {
            transfer.forced = true;
        }
        active.values().cloned().collect()
    }

    pub fn totals(&self) -> TransferTotals {
        TransferTotals {
            completed: self.completed.load(Ordering::SeqCst),
            aborted: self.aborted.load(Ordering::SeqCst),
        }
    }

    pub async fn wait_for_no_transfers(&self) {
        loop {
            let changed = self.changed.notified();
            if self.active_count() == 0 {
                return;
            }
            changed.await;
        }
    }

    fn set_field(&self, transfer_id: Uuid, key: &str, value: Value) {
        if let Some(transfer) = self
            .active
            .lock()
            .expect("transfer tracker poisoned")
            .get_mut(&transfer_id)
        {
            transfer.fields.insert(key.to_string(), value);
        }
    }

    fn finish(&self, transfer_id: Uuid) {
        let transfer = self
            .active
            .lock()
            .expect("transfer tracker poisoned")
            .remove(&transfer_id);
        if let Some(transfer) = transfer {
            if transfer.forced {
                self.aborted.fetch_add(1, Ordering::SeqCst);
            } else {
                self.completed.fetch_add(1, Ordering::SeqCst);
            }
            self.changed.notify_waiters();
        }
    }
}

pub struct TransferGuard {
    state: Arc<ShutdownState>,
    transfer_id: Uuid,
}

impl TransferGuard {
    pub fn set_field(&self, key: &str, value: impl Into<Value>) {
        self.state.set_field(self.transfer_id, key, value.into());
    }
}

impl Drop for TransferGuard {
    fn drop(&mut self) {
        self.state.finish(self.transfer_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tracks_completed_and_forced_transfers() {
        let state = Arc::new(ShutdownState::default());
        let completed = state.start_transfer(TransferKind::Download, "one".into(), BTreeMap::new());
        drop(completed);
        let forced = state.start_transfer(TransferKind::Upload, "two".into(), BTreeMap::new());
        assert_eq!(state.mark_active_forced().len(), 1);
        drop(forced);
        state.wait_for_no_transfers().await;
        let totals = state.totals();
        assert_eq!(totals.completed, 1);
        assert_eq!(totals.aborted, 1);
    }
}
