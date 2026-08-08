//! Internal actor protocol.
//!
//! `SamplerCommand` is `pub(crate)` because it is the wire between
//! [`SamplerHandle`](crate::handle::SamplerHandle) and the actor task,
//! not a public type. External callers always go through `SamplerHandle`.

use tokio::sync::oneshot;

use xai_grok_sampling_types::{ConversationRequest, ConversationResponse, SamplingError};

use crate::config::SamplerConfig;
use crate::metrics::InferenceLatencyStats;
use crate::protected_overlay::{ProtectedInferenceOverlay, ProtectedOverlayAck};
use crate::types::RequestId;

/// Capability-checked payload and its pre-dispatch acknowledgement channel.
/// Kept as one move-only unit so the actor cannot accidentally separate them.
pub(crate) struct ProtectedSubmission {
    pub(crate) overlay: ProtectedInferenceOverlay,
    pub(crate) ack_tx: oneshot::Sender<ProtectedOverlayAck>,
}

/// Commands sent from a [`SamplerHandle`](crate::handle::SamplerHandle)
/// to the actor task.
///
/// Large payloads (`ConversationRequest`, `SamplerConfig`) are boxed so
/// every command stays cheap to copy through the mpsc channel.
pub(crate) enum SamplerCommand {
    /// Submit a new sampling request. Fire-and-forget — results come via
    /// events. When `completion_tx` is set the per-request task also
    /// signals that channel for `submit_and_collect` callers.
    Submit {
        request_id: RequestId,
        request: Box<ConversationRequest>,
        config: Option<Box<SamplerConfig>>,
        protected: Option<Box<ProtectedSubmission>>,
        completion_tx: Option<
            oneshot::Sender<Result<(ConversationResponse, InferenceLatencyStats), SamplingError>>,
        >,
    },

    /// Cancel an in-flight request.
    Cancel { request_id: RequestId },

    /// Update the default sampling config (model switch, auth refresh).
    UpdateConfig { config: Box<SamplerConfig> },

    /// Query: is a specific request still in flight?
    IsActive {
        request_id: RequestId,
        reply: oneshot::Sender<bool>,
    },

    /// Query: how many requests are in flight?
    ActiveCount { reply: oneshot::Sender<usize> },
}
