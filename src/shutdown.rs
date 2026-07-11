//! Graceful shutdown coordination for active uploads and downloads.
//!
//! Transfer guards let UDS stop accepting new work while allowing in-flight
//! traffic to finish until the configured deadline expires.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::sync::Notify;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Network transfer categories tracked during graceful shutdown.
pub enum TransferKind {
    /// Represents the item case in UDS.
    Download,

    /// Represents the item case in UDS.
    Upload,
}

impl TransferKind {
    /// Provides the as str operation used by UDS callers.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Download => "download",
            Self::Upload => "upload",
        }
    }
}

#[derive(Debug, Clone)]
/// Metadata required to identify and audit an in-flight transfer.
pub struct ActiveTransfer {
    /// The transfer id carried by this UDS data contract.
    pub transfer_id: Uuid,

    /// The request id carried by this UDS data contract.
    pub request_id: String,

    /// The kind carried by this UDS data contract.
    pub kind: TransferKind,

    /// The fields carried by this UDS data contract.
    pub fields: BTreeMap<String, Value>,

    /// Stores the forced value used by this UDS component.
    forced: bool,
}

#[derive(Debug, Default, Clone, Copy)]
/// Lifetime counters used to summarize completed and aborted transfers.
pub struct TransferTotals {
    /// The completed carried by this UDS data contract.
    pub completed: u64,

    /// The aborted carried by this UDS data contract.
    pub aborted: u64,
}

#[derive(Debug, Default)]
/// Shared state that coordinates listener draining and active transfers.
pub struct ShutdownState {
    /// Stores the draining value used by this UDS component.
    draining: AtomicBool,

    /// Stores the active value used by this UDS component.
    active: Mutex<BTreeMap<Uuid, ActiveTransfer>>,

    /// Stores the completed value used by this UDS component.
    completed: AtomicU64,

    /// Stores the aborted value used by this UDS component.
    aborted: AtomicU64,

    /// Stores the changed value used by this UDS component.
    changed: Notify,

    /// Stores the drain started value used by this UDS component.
    drain_started: Notify,
}

impl ShutdownState {
    /// Provides the begin draining operation used by UDS callers.
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

    /// Returns whether is draining applies to the current UDS state.
    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }

    /// Provides the draining notified operation used by UDS callers.
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

    /// Runs the start transfer workflow for UDS.
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

    /// Provides the active count operation used by UDS callers.
    pub fn active_count(&self) -> usize {
        self.active.lock().expect("transfer tracker poisoned").len()
    }

    /// Provides the mark active forced operation used by UDS callers.
    pub fn mark_active_forced(&self) -> Vec<ActiveTransfer> {
        let mut active = self.active.lock().expect("transfer tracker poisoned");
        for transfer in active.values_mut() {
            transfer.forced = true;
        }
        active.values().cloned().collect()
    }

    /// Provides the totals operation used by UDS callers.
    pub fn totals(&self) -> TransferTotals {
        TransferTotals {
            completed: self.completed.load(Ordering::SeqCst),
            aborted: self.aborted.load(Ordering::SeqCst),
        }
    }

    /// Provides the wait for no transfers operation used by UDS callers.
    pub async fn wait_for_no_transfers(&self) {
        loop {
            let changed = self.changed.notified();
            if self.active_count() == 0 {
                return;
            }
            changed.await;
        }
    }

    /// Performs the set field operation required by UDS.
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

    /// Performs the finish operation required by UDS.
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

/// RAII guard that unregisters a transfer when its response body completes.
pub struct TransferGuard {
    /// Stores the state value used by this UDS component.
    state: Arc<ShutdownState>,

    /// Stores the transfer id value used by this UDS component.
    transfer_id: Uuid,
}

impl TransferGuard {
    /// Applies the set field mutation while preserving UDS consistency guarantees.
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

    /// Verifies that tracks completed and forced transfers.
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
