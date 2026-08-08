//! Public handle for talking to the sampler actor.

use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};

use xai_grok_sampling_types::{ConversationRequest, ConversationResponse, SamplingError};

use crate::commands::{ProtectedSubmission, SamplerCommand};
use crate::config::SamplerConfig;
use crate::metrics::InferenceLatencyStats;
use crate::protected_overlay::{
    ProtectedInferenceOverlay, ProtectedOverlayAck, ProtectedOverlayError, ProtectedOverlayKey,
};
use crate::types::RequestId;

/// Cheaply-cloneable handle to the sampler actor.
///
/// Internally just an `mpsc::UnboundedSender<SamplerCommand>`. All
/// methods are non-blocking (fire-and-forget) except for the
/// `*_async` queries which return a future awaiting an
/// `oneshot::Receiver`.
#[derive(Clone)]
pub struct SamplerHandle {
    cmd_tx: mpsc::UnboundedSender<SamplerCommand>,
    protected_overlay_key: Arc<ProtectedOverlayKey>,
}

impl SamplerHandle {
    /// Construct a handle from a command sender. `pub(crate)` because
    /// only [`SamplerActor::spawn`](crate::actor::SamplerActor::spawn)
    /// produces one of these.
    pub(crate) fn new(cmd_tx: mpsc::UnboundedSender<SamplerCommand>) -> Self {
        Self {
            cmd_tx,
            protected_overlay_key: Arc::new(ProtectedOverlayKey),
        }
    }

    /// Create a no-op handle that discards all commands.
    ///
    /// Useful for tests and callers that need a `SamplerHandle` field
    /// before the actor is wired up. Mirrors
    /// [`HunkTrackerHandle::noop`](https://docs.rs/xai-hunk-tracker).
    pub fn noop() -> Self {
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        // Receiver is dropped immediately; sends will fail but every
        // send-site uses `let _ = ...` so that is fine.
        Self::new(cmd_tx)
    }

    /// Validate and attest a PNG for request-only computer use.
    ///
    /// The returned move-only overlay is scoped to this handle family: it can
    /// only be submitted by this handle or one of its clones.
    pub fn attest_protected_overlay(
        &self,
        snapshot_id: impl Into<String>,
        observation: impl Into<String>,
        png: Vec<u8>,
        expected_sha256_hex: &str,
        pixel_width: u32,
        pixel_height: u32,
    ) -> Result<ProtectedInferenceOverlay, ProtectedOverlayError> {
        ProtectedInferenceOverlay::attest(
            Arc::clone(&self.protected_overlay_key),
            snapshot_id.into(),
            observation.into(),
            png,
            expected_sha256_hex,
            pixel_width,
            pixel_height,
        )
    }

    pub(crate) fn protected_overlay_key(&self) -> &Arc<ProtectedOverlayKey> {
        &self.protected_overlay_key
    }

    /// Submit a sampling request. Fire-and-forget -- results arrive
    /// via the shared event channel.
    pub fn submit(&self, request_id: RequestId, request: ConversationRequest) {
        let _ = self.cmd_tx.send(SamplerCommand::Submit {
            request_id,
            request: Box::new(request),
            config: None,
            protected: None,
            completion_tx: None,
        });
    }

    /// Submit a sampling request with an explicit per-request config
    /// override (e.g., a different model than the actor's default).
    pub fn submit_with_config(
        &self,
        request_id: RequestId,
        request: ConversationRequest,
        config: SamplerConfig,
    ) {
        let _ = self.cmd_tx.send(SamplerCommand::Submit {
            request_id,
            request: Box::new(request),
            config: Some(Box::new(config)),
            protected: None,
            completion_tx: None,
        });
    }

    /// Cancel an in-flight request. No-op if the request id is
    /// unknown (already finished or never submitted).
    pub fn cancel(&self, request_id: RequestId) {
        let _ = self.cmd_tx.send(SamplerCommand::Cancel { request_id });
    }

    /// Update the default sampling config (e.g., after model switch
    /// or auth refresh). The next request submitted without an
    /// override will use it.
    pub fn update_config(&self, config: SamplerConfig) {
        let _ = self.cmd_tx.send(SamplerCommand::UpdateConfig {
            config: Box::new(config),
        });
    }

    /// Query whether a request is still in flight. Returns `false`
    /// for unknown / finished / cancelled ids.
    pub async fn is_active(&self, request_id: RequestId) -> bool {
        let (reply_tx, reply_rx) = oneshot::channel();
        let _ = self.cmd_tx.send(SamplerCommand::IsActive {
            request_id,
            reply: reply_tx,
        });
        reply_rx.await.unwrap_or(false)
    }

    /// Query the number of in-flight requests. Returns 0 if the
    /// actor has been shut down.
    pub async fn active_count(&self) -> usize {
        let (reply_tx, reply_rx) = oneshot::channel();
        let _ = self
            .cmd_tx
            .send(SamplerCommand::ActiveCount { reply: reply_tx });
        reply_rx.await.unwrap_or(0)
    }

    /// Submit a request and await its completion. Events still flow
    /// to the shared channel for live UI updates -- this method just
    /// additionally awaits the per-request completion oneshot so the
    /// caller gets a clean `Result` without filtering events.
    ///
    /// Used by sequential callers like compaction / summary /
    /// `/btw` side questions.
    pub async fn submit_and_collect(
        &self,
        request_id: RequestId,
        request: ConversationRequest,
    ) -> Result<(ConversationResponse, InferenceLatencyStats), SamplingError> {
        let (ack, result) = self
            .submit_and_collect_inner(request_id, request, None)
            .await;
        debug_assert!(ack.is_none());
        result
    }

    /// Submit a request with one protected PNG attached only to its first and
    /// only HTTP attempt.
    ///
    /// The acknowledgement is resolved by the final backend body builder,
    /// before HTTP execution. `NotAttached` means no model-produced response
    /// from this call is safe to dispatch as computer-use actions.
    pub async fn submit_and_collect_protected(
        &self,
        request_id: RequestId,
        request: ConversationRequest,
        overlay: ProtectedInferenceOverlay,
    ) -> (
        ProtectedOverlayAck,
        Result<(ConversationResponse, InferenceLatencyStats), SamplingError>,
    ) {
        if !overlay.is_authorized_for(self) {
            return (
                ProtectedOverlayAck::NotAttached,
                Err(SamplingError::InvalidConfiguration(
                    "protected overlay capability mismatch",
                )),
            );
        }
        let (ack, result) = self
            .submit_and_collect_inner(request_id, request, Some(overlay))
            .await;
        (ack.unwrap_or(ProtectedOverlayAck::NotAttached), result)
    }

    async fn submit_and_collect_inner(
        &self,
        request_id: RequestId,
        request: ConversationRequest,
        overlay: Option<ProtectedInferenceOverlay>,
    ) -> (
        Option<ProtectedOverlayAck>,
        Result<(ConversationResponse, InferenceLatencyStats), SamplingError>,
    ) {
        // RAII guard: when this future is dropped (cancel, panic, or normal return),
        // tell the sampler actor to cancel the in-flight request_id. No-op if the
        // actor already finished and removed it from its active set.
        struct CancelOnDrop {
            cmd_tx: mpsc::UnboundedSender<SamplerCommand>,
            request_id: RequestId,
        }
        impl Drop for CancelOnDrop {
            fn drop(&mut self) {
                // fire-and-forget the send.
                let _ = self.cmd_tx.send(SamplerCommand::Cancel {
                    request_id: self.request_id.clone(),
                });
            }
        }

        let (completion_tx, completion_rx) = oneshot::channel();
        let (protected, ack_rx) = match overlay {
            Some(overlay) => {
                let (ack_tx, ack_rx) = oneshot::channel();
                (
                    Some(Box::new(ProtectedSubmission { overlay, ack_tx })),
                    Some(ack_rx),
                )
            }
            None => (None, None),
        };
        let cancel_id = request_id.clone();

        // Only arm the guard if Submit actually reached the actor.
        let _guard = self
            .cmd_tx
            .send(SamplerCommand::Submit {
                request_id,
                request: Box::new(request),
                config: None,
                protected,
                completion_tx: Some(completion_tx),
            })
            .ok()
            .map(|_| CancelOnDrop {
                cmd_tx: self.cmd_tx.clone(),
                request_id: cancel_id,
            });
        // Await the pre-dispatch acknowledgement first. This orders it before
        // any successful completion can reach a caller that executes actions.
        let ack = match ack_rx {
            Some(rx) => Some(rx.await.unwrap_or(ProtectedOverlayAck::NotAttached)),
            None => None,
        };
        let result = completion_rx.await.unwrap_or_else(|_| {
            Err(SamplingError::auth_unknown(
                "sampler actor dropped before completion",
            ))
        });
        (ack, result)
    }
}
