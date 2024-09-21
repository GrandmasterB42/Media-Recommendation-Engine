mod cache;
mod communication;
mod playlist;
mod session;
mod streamidentifier;
mod transcode;

use std::time::Duration;

pub use session::SessionId;
pub use session::{Session, StreamingSessions};
pub use streamidentifier::{StreamIndex, StreamIndicies};
pub use transcode::MediaRequest;

const SEGMENT_DURATION: Duration = Duration::from_secs(10);
const MAX_CACHED_SEGMENTS: usize = 32;
// Fewer segments at a time tends to cause more artifacting, requests, ..
const PRECOMPUTE_SEGMENTS: usize = if cfg!(debug_assertions) { 1 } else { 4 };
const FFMPEG_LOG_LEVEL: &str = if cfg!(debug_assertions) {
    "warning"
} else {
    "fatal"
};

// TODO: Add tests for some of these formats, for example the stream identifier and segmentation and generation of playlists
