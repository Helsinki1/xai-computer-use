//! Capability-gated, request-only computer-use screenshots.
//!
//! This module deliberately keeps protected screenshots outside
//! [`ConversationRequest`](xai_grok_sampling_types::ConversationRequest) until
//! the last backend-specific conversion. The public payload type is move-only,
//! has no serde or `Debug` implementation, and can only be constructed by a
//! [`SamplerHandle`](crate::SamplerHandle) that owns the matching capability.

use std::fmt;
use std::io::Read as _;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;
use xai_grok_sampling_types::{ContentPart, ConversationItem, ConversationRequest, UserItem};

use crate::handle::SamplerHandle;

/// Maximum encoded source image accepted by the protected inference path.
pub const MAX_PROTECTED_OVERLAY_BYTES: usize = 900_000;
/// Maximum width or height accepted by the protected inference path.
pub const MAX_PROTECTED_OVERLAY_DIMENSION: u32 = 1_280;
/// Maximum decoded pixel count accepted by the protected inference path.
pub const MAX_PROTECTED_OVERLAY_PIXELS: u64 = 1_638_400;
/// Maximum model-only accessibility/observation text accepted with a PNG.
pub const MAX_PROTECTED_OVERLAY_OBSERVATION_BYTES: usize = 16 * 1024;

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const MAX_SNAPSHOT_ID_BYTES: usize = 256;
const DATA_URL_PREFIX: &str = "data:image/png;base64,";

/// Validation failure while attesting a protected PNG.
///
/// Variants intentionally carry no supplied values, so formatting an error
/// cannot reveal the snapshot identifier, hash, or image bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ProtectedOverlayError {
    InvalidSnapshotId,
    InvalidSha256,
    Sha256Mismatch,
    ImageTooLarge,
    InvalidPng,
    InvalidDimensions,
    DimensionMismatch,
    PixelLimitExceeded,
    ObservationTooLarge,
}

impl fmt::Display for ProtectedOverlayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidSnapshotId => "protected overlay snapshot id is invalid",
            Self::InvalidSha256 => "protected overlay sha256 is invalid",
            Self::Sha256Mismatch => "protected overlay sha256 does not match",
            Self::ImageTooLarge => "protected overlay exceeds the byte limit",
            Self::InvalidPng => "protected overlay is not a valid PNG container",
            Self::InvalidDimensions => "protected overlay dimensions are invalid",
            Self::DimensionMismatch => "protected overlay dimensions do not match the PNG",
            Self::PixelLimitExceeded => "protected overlay exceeds the pixel limit",
            Self::ObservationTooLarge => "protected overlay observation exceeds the byte limit",
        })
    }
}

impl std::error::Error for ProtectedOverlayError {}

/// A handle-scoped capability. Only `SamplerHandle` can mint the key carried
/// by an overlay, and submission checks that the same handle family is used.
pub(crate) struct ProtectedOverlayKey;

/// A validated, request-only computer-use screenshot.
///
/// This type intentionally implements neither `Clone`, `Debug`, nor serde.
/// Consuming it is the only way to attach its PNG to an inference request.
pub struct ProtectedInferenceOverlay {
    capability: Arc<ProtectedOverlayKey>,
    snapshot_id: Box<str>,
    observation: Box<str>,
    sha256: [u8; 32],
    png: Vec<u8>,
    pixel_width: u32,
    pixel_height: u32,
}

impl ProtectedInferenceOverlay {
    pub(crate) fn attest(
        capability: Arc<ProtectedOverlayKey>,
        snapshot_id: String,
        observation: String,
        png: Vec<u8>,
        expected_sha256_hex: &str,
        pixel_width: u32,
        pixel_height: u32,
    ) -> Result<Self, ProtectedOverlayError> {
        if snapshot_id.is_empty()
            || snapshot_id.len() > MAX_SNAPSHOT_ID_BYTES
            || snapshot_id.chars().any(char::is_control)
        {
            return Err(ProtectedOverlayError::InvalidSnapshotId);
        }
        if png.len() > MAX_PROTECTED_OVERLAY_BYTES {
            return Err(ProtectedOverlayError::ImageTooLarge);
        }
        if observation.len() > MAX_PROTECTED_OVERLAY_OBSERVATION_BYTES {
            return Err(ProtectedOverlayError::ObservationTooLarge);
        }
        let expected_sha256 =
            parse_sha256(expected_sha256_hex).ok_or(ProtectedOverlayError::InvalidSha256)?;
        let actual_sha256: [u8; 32] = Sha256::digest(&png).into();
        if actual_sha256 != expected_sha256 {
            return Err(ProtectedOverlayError::Sha256Mismatch);
        }

        validate_dimensions(pixel_width, pixel_height)?;
        let (png_width, png_height) = png_dimensions(&png)?;
        if (pixel_width, pixel_height) != (png_width, png_height) {
            return Err(ProtectedOverlayError::DimensionMismatch);
        }

        Ok(Self {
            capability,
            snapshot_id: snapshot_id.into_boxed_str(),
            observation: observation.into_boxed_str(),
            sha256: expected_sha256,
            png,
            pixel_width,
            pixel_height,
        })
    }

    pub(crate) fn is_authorized_for(&self, handle: &SamplerHandle) -> bool {
        Arc::ptr_eq(&self.capability, handle.protected_overlay_key())
    }

    /// Attach this image to the request-local copy and retain an exact body
    /// needle for the backend's final serialized-body check.
    pub(crate) fn attach_to<'a>(
        self,
        request: &mut ConversationRequest,
        ack: &'a mut ProtectedOverlayAckGuard,
    ) -> ProtectedBodyAttachment<'a> {
        let Self {
            capability: _,
            snapshot_id,
            observation,
            sha256,
            png,
            pixel_width,
            pixel_height,
        } = self;
        let encoded_png = STANDARD.encode(png);
        let data_url = format!("{DATA_URL_PREFIX}{encoded_png}");
        let coordinate_note = format!(
            "<computer_use_screenshot>\n\
             snapshot_id={}\n\
             Coordinates for the attached screenshot use continuous PNG edge-space: \
             width={pixel_width}, height={pixel_height}, origin=(0,0) at the top-left, \
             x increases right, and y increases down. Use x in [0,{pixel_width}) and \
             y in [0,{pixel_height}); pixel centers are at half-integer coordinates. \
             Do not convert from display points, CSS pixels, or device-independent units.\n\
             <protected_observation>\n{}\n</protected_observation>\n\
             </computer_use_screenshot>",
            serde_json::to_string(snapshot_id.as_ref()).expect("string serialization cannot fail"),
            observation,
        );
        request.items.push(ConversationItem::User(UserItem {
            content: vec![
                ContentPart::Text {
                    text: Arc::<str>::from(coordinate_note),
                },
                ContentPart::Image {
                    url: Arc::<str>::from(data_url.as_str()),
                },
            ],
            synthetic_reason: None,
            ..Default::default()
        }));
        ProtectedBodyAttachment {
            exact_data_url_json: serde_json::to_vec(&data_url)
                .expect("string serialization cannot fail"),
            exact_base64_json: serde_json::to_vec(&encoded_png)
                .expect("string serialization cannot fail"),
            receipt: ProtectedOverlayReceipt {
                snapshot_id,
                sha256,
                pixel_width,
                pixel_height,
            },
            ack,
        }
    }

    /// Pixel dimensions proven against the PNG IHDR during attestation.
    pub fn pixel_dimensions(&self) -> (u32, u32) {
        (self.pixel_width, self.pixel_height)
    }
}

fn validate_dimensions(width: u32, height: u32) -> Result<(), ProtectedOverlayError> {
    if width == 0
        || height == 0
        || width > MAX_PROTECTED_OVERLAY_DIMENSION
        || height > MAX_PROTECTED_OVERLAY_DIMENSION
    {
        return Err(ProtectedOverlayError::InvalidDimensions);
    }
    if u64::from(width) * u64::from(height) > MAX_PROTECTED_OVERLAY_PIXELS {
        return Err(ProtectedOverlayError::PixelLimitExceeded);
    }
    Ok(())
}

fn png_dimensions(png: &[u8]) -> Result<(u32, u32), ProtectedOverlayError> {
    // Validate the container boundaries through the terminal IEND. SHA-256
    // authenticates the exact bytes; this parser establishes that those bytes
    // are a bounded PNG with one authoritative IHDR and image data.
    if png.len() < 33
        || &png[..8] != PNG_SIGNATURE
        || u32::from_be_bytes(png[8..12].try_into().expect("fixed slice")) != 13
        || &png[12..16] != b"IHDR"
        // The native relay emits one canonical format. Rejecting every other
        // legal PNG format keeps the producer and sampler coordinate/image
        // contract exact instead of silently accepting a divergent encoder.
        || png[24..29] != [8, 6, 0, 0, 0]
    {
        return Err(ProtectedOverlayError::InvalidPng);
    }
    let width = u32::from_be_bytes(png[16..20].try_into().expect("fixed slice"));
    let height = u32::from_be_bytes(png[20..24].try_into().expect("fixed slice"));
    validate_dimensions(width, height)?;

    let mut offset = 8_usize;
    let mut saw_idat = false;
    let mut idat_ended = false;
    let mut compressed_pixels = Vec::new();
    loop {
        let header_end = offset
            .checked_add(8)
            .filter(|end| *end <= png.len())
            .ok_or(ProtectedOverlayError::InvalidPng)?;
        let length = usize::try_from(u32::from_be_bytes(
            png[offset..offset + 4].try_into().expect("fixed slice"),
        ))
        .map_err(|_| ProtectedOverlayError::InvalidPng)?;
        let chunk_type = &png[offset + 4..header_end];
        let chunk_end = header_end
            .checked_add(length)
            .and_then(|end| end.checked_add(4))
            .filter(|end| *end <= png.len())
            .ok_or(ProtectedOverlayError::InvalidPng)?;
        let data_end = header_end + length;
        let expected_crc =
            u32::from_be_bytes(png[data_end..chunk_end].try_into().expect("fixed slice"));
        let mut crc = crc32fast::Hasher::new();
        crc.update(chunk_type);
        crc.update(&png[header_end..data_end]);
        if crc.finalize() != expected_crc {
            return Err(ProtectedOverlayError::InvalidPng);
        }
        if chunk_type == b"IHDR" {
            if offset != 8 {
                return Err(ProtectedOverlayError::InvalidPng);
            }
        } else if chunk_type == b"IDAT" {
            if idat_ended {
                return Err(ProtectedOverlayError::InvalidPng);
            }
            saw_idat = true;
            compressed_pixels.extend_from_slice(&png[header_end..data_end]);
        } else if chunk_type == b"IEND" {
            if length != 0 || !saw_idat || chunk_end != png.len() {
                return Err(ProtectedOverlayError::InvalidPng);
            }
            break;
        } else {
            idat_ended |= saw_idat;
            if chunk_type[0].is_ascii_uppercase() {
                return Err(ProtectedOverlayError::InvalidPng);
            }
        }
        offset = chunk_end;
    }

    let row_bytes = usize::try_from(width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .and_then(|bytes| bytes.checked_add(1))
        .ok_or(ProtectedOverlayError::InvalidPng)?;
    let decoded_len = row_bytes
        .checked_mul(usize::try_from(height).map_err(|_| ProtectedOverlayError::InvalidPng)?)
        .ok_or(ProtectedOverlayError::InvalidPng)?;
    let decoder = flate2::read::ZlibDecoder::new(compressed_pixels.as_slice());
    let mut bounded = decoder.take(
        u64::try_from(decoded_len)
            .map_err(|_| ProtectedOverlayError::InvalidPng)?
            .saturating_add(1),
    );
    let mut decoded = Vec::with_capacity(decoded_len);
    bounded
        .read_to_end(&mut decoded)
        .map_err(|_| ProtectedOverlayError::InvalidPng)?;
    let decoder = bounded.into_inner();
    if decoded.len() != decoded_len
        || usize::try_from(decoder.total_in()).ok() != Some(compressed_pixels.len())
        || decoded.chunks_exact(row_bytes).any(|row| row[0] > 4)
    {
        return Err(ProtectedOverlayError::InvalidPng);
    }
    Ok((width, height))
}

fn parse_sha256(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut output = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = hex_nibble(pair[0])?;
        let low = hex_nibble(pair[1])?;
        output[index] = (high << 4) | low;
    }
    Some(output)
}

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

/// Whether the protected PNG was proven present in the final request body.
pub enum ProtectedOverlayAck {
    Attached(ProtectedOverlayReceipt),
    NotAttached,
}

/// Opaque delivery correlation returned only after exact body attestation.
///
/// It intentionally has no `Clone`, `Debug`, or serde implementation. The
/// caller can correlate the acknowledgement with its private snapshot state
/// without placing the identifier or hash in logs, traces, or chat state.
pub struct ProtectedOverlayReceipt {
    snapshot_id: Box<str>,
    sha256: [u8; 32],
    pixel_width: u32,
    pixel_height: u32,
}

impl ProtectedOverlayReceipt {
    pub fn snapshot_id(&self) -> &str {
        &self.snapshot_id
    }

    pub fn pixel_dimensions(&self) -> (u32, u32) {
        (self.pixel_width, self.pixel_height)
    }

    /// Compare against the source attestation without exposing stored hash
    /// bytes through a formatting trait.
    pub fn matches_attestation(&self, snapshot_id: &str, sha256_hex: &str) -> bool {
        self.snapshot_id.as_ref() == snapshot_id
            && parse_sha256(sha256_hex).is_some_and(|sha256| sha256 == self.sha256)
    }
}

/// Sends `NotAttached` on every early return unless a backend body builder
/// proves the exact data URL is present once and marks it attached.
pub(crate) struct ProtectedOverlayAckGuard {
    sender: Option<oneshot::Sender<ProtectedOverlayAck>>,
}

impl ProtectedOverlayAckGuard {
    pub(crate) fn new(sender: Option<oneshot::Sender<ProtectedOverlayAck>>) -> Self {
        Self { sender }
    }

    fn mark_attached(&mut self, receipt: ProtectedOverlayReceipt) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(ProtectedOverlayAck::Attached(receipt));
        }
    }
}

impl Drop for ProtectedOverlayAckGuard {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(ProtectedOverlayAck::NotAttached);
        }
    }
}

/// Request-body proof carried from request-local conversion to exactly one
/// backend body builder. It contains sensitive encoded image data and
/// therefore intentionally has no formatting or serialization traits.
pub(crate) struct ProtectedBodyAttachment<'a> {
    exact_data_url_json: Vec<u8>,
    exact_base64_json: Vec<u8>,
    receipt: ProtectedOverlayReceipt,
    ack: &'a mut ProtectedOverlayAckGuard,
}

impl ProtectedBodyAttachment<'_> {
    /// Prove that the exact data URL derived from the attested PNG occurs once
    /// in the final serialized body, then acknowledge before HTTP execution.
    pub(crate) fn acknowledge_data_url(
        self,
        request: &reqwest::Request,
    ) -> xai_grok_sampling_types::Result<()> {
        let Self {
            exact_data_url_json,
            exact_base64_json: _,
            receipt,
            ack,
        } = self;
        Self::acknowledge_exact(request, &exact_data_url_json, receipt, ack)
    }

    /// Messages API separates a data URL into `media_type` and raw base64, so
    /// attest its exact encoded PNG scalar rather than the removed prefix.
    pub(crate) fn acknowledge_base64(
        self,
        request: &reqwest::Request,
    ) -> xai_grok_sampling_types::Result<()> {
        let Self {
            exact_data_url_json: _,
            exact_base64_json,
            receipt,
            ack,
        } = self;
        Self::acknowledge_exact(request, &exact_base64_json, receipt, ack)
    }

    fn acknowledge_exact(
        request: &reqwest::Request,
        exact_json_scalar: &[u8],
        receipt: ProtectedOverlayReceipt,
        ack: &mut ProtectedOverlayAckGuard,
    ) -> xai_grok_sampling_types::Result<()> {
        let Some(body) = request.body().and_then(reqwest::Body::as_bytes) else {
            return Err(
                xai_grok_sampling_types::SamplingError::serialization_message(
                    "protected overlay request body is not inspectable",
                ),
            );
        };
        let occurrences = body
            .windows(exact_json_scalar.len())
            .filter(|window| *window == exact_json_scalar)
            .take(2)
            .count();
        if occurrences != 1 {
            return Err(
                xai_grok_sampling_types::SamplingError::serialization_message(
                    "protected overlay request body attestation failed",
                ),
            );
        }
        ack.mark_attached(receipt);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_container(
        width: u32,
        height: u32,
        bit_depth: u8,
        color_type: u8,
        interlace: u8,
    ) -> Vec<u8> {
        let mut png = Vec::from(PNG_SIGNATURE.as_slice());
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[bit_depth, color_type, 0, 0, interlace]);
        append_chunk(&mut png, b"IHDR", &ihdr);
        let row_bytes = usize::try_from(width).unwrap() * 4 + 1;
        let pixels = vec![0_u8; row_bytes * usize::try_from(height).unwrap()];
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &pixels).unwrap();
        append_chunk(&mut png, b"IDAT", &encoder.finish().unwrap());
        append_chunk(&mut png, b"IEND", &[]);
        png
    }

    fn canonical_png_container(width: u32, height: u32) -> Vec<u8> {
        png_container(width, height, 8, 6, 0)
    }

    fn append_chunk(png: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        png.extend_from_slice(chunk_type);
        png.extend_from_slice(data);
        let mut crc = crc32fast::Hasher::new();
        crc.update(chunk_type);
        crc.update(data);
        png.extend_from_slice(&crc.finalize().to_be_bytes());
    }

    #[test]
    fn validates_hash_and_authoritative_dimensions_without_echoing_values() {
        let png = canonical_png_container(17, 23);
        let hash = format!("{:x}", Sha256::digest(&png));
        let capability = Arc::new(ProtectedOverlayKey);
        let overlay = ProtectedInferenceOverlay::attest(
            capability,
            "secret-snapshot-id".to_string(),
            "AXButton: Save".to_string(),
            png.clone(),
            &hash,
            17,
            23,
        )
        .expect("valid attestation");
        assert_eq!(overlay.pixel_dimensions(), (17, 23));

        let error = match ProtectedInferenceOverlay::attest(
            Arc::new(ProtectedOverlayKey),
            "secret-snapshot-id".to_string(),
            "AXButton: Save".to_string(),
            png,
            &"0".repeat(64),
            17,
            23,
        ) {
            Ok(_) => panic!("expected hash mismatch"),
            Err(error) => error,
        };
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains("secret-snapshot-id"));
        assert!(!rendered.contains(&"0".repeat(64)));
    }

    #[test]
    fn rejects_declared_dimensions_that_differ_from_png() {
        let png = canonical_png_container(17, 23);
        let hash = format!("{:x}", Sha256::digest(&png));
        let error = match ProtectedInferenceOverlay::attest(
            Arc::new(ProtectedOverlayKey),
            "snapshot".to_string(),
            "AXButton: Save".to_string(),
            png,
            &hash,
            18,
            23,
        ) {
            Ok(_) => panic!("expected dimension mismatch"),
            Err(error) => error,
        };
        assert_eq!(error, ProtectedOverlayError::DimensionMismatch);
    }

    #[test]
    fn rejects_noncanonical_color_and_interlace_formats() {
        for png in [
            png_container(17, 23, 8, 4, 0),
            png_container(17, 23, 8, 6, 1),
        ] {
            let hash = format!("{:x}", Sha256::digest(&png));
            let error = match ProtectedInferenceOverlay::attest(
                Arc::new(ProtectedOverlayKey),
                "snapshot".to_string(),
                "AXButton: Save".to_string(),
                png,
                &hash,
                17,
                23,
            ) {
                Ok(_) => panic!("expected noncanonical PNG rejection"),
                Err(error) => error,
            };
            assert_eq!(error, ProtectedOverlayError::InvalidPng);
        }
    }

    #[test]
    fn rejects_empty_image_data() {
        let mut png = Vec::from(PNG_SIGNATURE.as_slice());
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&17_u32.to_be_bytes());
        ihdr.extend_from_slice(&23_u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        append_chunk(&mut png, b"IHDR", &ihdr);
        append_chunk(&mut png, b"IDAT", &[]);
        append_chunk(&mut png, b"IEND", &[]);
        assert_eq!(png_dimensions(&png), Err(ProtectedOverlayError::InvalidPng));
    }
}
