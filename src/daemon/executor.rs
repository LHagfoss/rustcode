//! Execution boundary for scheduled jobs. Concrete action dispatch is Task 4.
use super::model::{JobRecord, JobRunRecord};
use std::{future::Future, pin::Pin};
use tokio_util::sync::CancellationToken;

pub type ExecutionFuture = Pin<Box<dyn Future<Output = RunOutcome> + Send + 'static>>;

#[derive(Clone)]
pub struct JobRunContext {
    pub job: JobRecord,
    pub run: JobRunRecord,
    pub cancellation: CancellationToken,
}

/// Implementations must honor cancellation at safe action boundaries and report
/// Ambiguous whenever an external effect may have happened without confirmation.
pub trait JobExecutor: Send + Sync {
    fn execute(&self, context: JobRunContext) -> ExecutionFuture;
}

#[derive(Debug, Clone)]
pub enum RunOutcome {
    Succeeded {
        summary: String,
        output: Option<String>,
    },
    Transient {
        error: String,
        output: Option<String>,
    },
    Permanent {
        error: String,
        output: Option<String>,
    },
    Cancelled {
        output: Option<String>,
    },
    Ambiguous {
        error: String,
        output: Option<String>,
    },
}
