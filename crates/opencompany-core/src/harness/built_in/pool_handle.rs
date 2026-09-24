//! The pool a teammate's own tools reach back into.
//!
//! A tool is built by the roster, the roster is built by the pool, so a tool
//! cannot be handed the pool at construction without a cycle. This is the
//! same answer
//! [`WorkflowRunnerHandle`](crate::harness::orchestrator::WorkflowRunnerHandle)
//! gives for the same problem, and it lives beside [`HarnessDeps`] rather
//! than beside its first caller because more than one tool needs it:
//! `consult_desk` opens an episode on it, and `hand_off` runs the teammate it
//! names.

use std::sync::{Arc, OnceLock, Weak};

use super::HarnessPool;

/// The pool an agent's own tools reach back into, filled once the pool exists.
///
/// A tool is built by the roster, the roster is built by the pool, so a tool
/// cannot be handed the pool at construction without a cycle. The handle is
/// the same answer
/// [`WorkflowRunnerHandle`](crate::harness::orchestrator::WorkflowRunnerHandle)
/// gives for the same problem: a shared cell filled after the fact, holding a
/// `Weak` so the tool never keeps the pool alive.
#[derive(Clone, Default)]
pub struct PoolHandle {
    inner: Arc<OnceLock<Weak<HarnessPool>>>,
}

impl PoolHandle {
    /// Fills the handle. Idempotent: a second fill is ignored, because the
    /// pool is built once per company boot.
    pub fn set(&self, pool: &Arc<HarnessPool>) {
        let _ = self.inner.set(Arc::downgrade(pool));
    }

    /// The wired pool, or `None` when none was attached or the runtime that
    /// owned it has been dropped.
    #[must_use]
    pub fn get(&self) -> Option<Arc<HarnessPool>> {
        self.inner.get().and_then(Weak::upgrade)
    }
}

impl std::fmt::Debug for PoolHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PoolHandle")
            .field("wired", &self.get().is_some())
            .finish()
    }
}
