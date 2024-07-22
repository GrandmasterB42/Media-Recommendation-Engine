use std::{
    fs,
    path::{Path, PathBuf},
    sync::RwLock,
};

use tokio::{process::Command, sync::Mutex};

use crate::{
    state::{AppError, AppResult},
    utils::{bail, pseudo_random, relative, HandleErr, ParseBetween},
};

use anyhow::Context;
use axum::{http::StatusCode, response::IntoResponse};
use tracing::{trace, warn};

const SEGMENT_DURATION: f64 = 10.0; // In seconds
const MAX_CACHED_SEGMENTS: usize = 32;

const FFMPEG_LOG_LEVEL: &str = if cfg!(debug_assertions) {
    "warning"
} else {
    "fatal"
};

pub enum MediaRequest {
    PlayList,
    Segment(usize),
}
pub struct TranscodedStream {
    media_source: Mutex<PathBuf>,
    media_cache: MediaCache,
}

impl TranscodedStream {
    pub async fn new(media_source: &Path, session_id: u32) -> AppResult<Self> {
        let media_context = ffmpeg::format::input(&media_source)?;
        let duration = media_context.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE);

        let mut temp_dir = PathBuf::from(relative!("../tmp"));
        temp_dir.push(pseudo_random().to_string());

        Ok(Self {
            media_source: Mutex::new(media_source.to_path_buf()),
            media_cache: MediaCache::new(media_source, &temp_dir, duration, session_id).await?,
        })
    }

    pub async fn current_path(&self) -> PathBuf {
        self.media_source.lock().await.clone()
    }

    pub async fn respond(&self, request: MediaRequest) -> AppResult<impl IntoResponse> {
        match request {
            MediaRequest::PlayList => {
                let playlist = self
                    .media_cache
                    .playlist
                    .read()
                    .expect("This should never happen");
                Ok(playlist.file_repr.clone().into_response())
            }
            MediaRequest::Segment(index) => {
                let segment = self.media_cache.request_segment(index).await;

                match segment {
                    Some(segment) => Ok(segment.into_response()),
                    None => {
                        let source = self.media_source.lock().await;
                        warn!("Failed to generate segment {index} for {source:?}");
                        Err(AppError::Status(StatusCode::NOT_FOUND))
                    }
                }
            }
        }
    }

    pub async fn reuse(&self, media_source: &Path) -> AppResult<()> {
        self.media_source
            .lock()
            .await
            .clone_from(&media_source.to_path_buf());

        let media_context = ffmpeg::format::input(&media_source)?;
        let duration = media_context.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE);

        self.media_cache.clear(media_source, duration).await?;

        Ok(())
    }
}

struct MediaSegment {
    pub data: Vec<u8>,
    index: usize,
}

struct MediaCache {
    session_id: u32,
    media_source: Mutex<PathBuf>,
    cache: RwLock<Vec<MediaSegment>>,
    playlist: RwLock<Playlist>,
    temp_directory: PathBuf,
    duration: Mutex<f64>,
}

impl MediaCache {
    async fn new(
        media_source: &Path,
        temp_directory: &Path,
        duration: f64,
        session_id: u32,
    ) -> AppResult<Self> {
        fs::create_dir_all(temp_directory)
            .log_err_with_msg("Failed to create temporary transcode directory");

        Ok(Self {
            session_id,
            media_source: Mutex::new(media_source.to_path_buf()),
            cache: RwLock::new(Vec::new()),
            playlist: RwLock::new(
                Playlist::generate(session_id, temp_directory, media_source).await?,
            ),
            temp_directory: temp_directory.to_path_buf(),
            duration: Mutex::new(duration),
        })
    }

    async fn clear(&self, media_source: &Path, duration: f64) -> AppResult<()> {
        self.media_source
            .lock()
            .await
            .clone_from(&media_source.to_path_buf());

        self.duration.lock().await.clone_from(&duration);

        self.cache
            .write()
            .expect("This should never happen")
            .clear();

        *self.playlist.write().expect("This should never happen") =
            Playlist::generate(self.session_id, &self.temp_directory, media_source).await?;

        Ok(())
    }

    /// Returns None if the segment is out of bounds or an error occured
    async fn request_segment(&self, index: usize) -> Option<Vec<u8>> {
        let segment = self
            .cache
            .read()
            .expect("This should never happen")
            .iter()
            .find(|segment| segment.index == index)
            .map(|segment| segment.data.clone());

        if let Some(segment) = segment {
            Some(segment)
        } else {
            // Check for out of bounds index
            if (index as f64 * SEGMENT_DURATION) >= *self.duration.lock().await {
                return None;
            }

            let Some(segments) = self.generate_segments_after(index).await.log_err() else {
                let source = self.media_source.lock().await;
                warn!("Failed to generate segments after index {index} for {source:?}");
                return None;
            };

            let mut cache = self.cache.write().expect("This should never happen");

            // Caching logic
            {
                let not_cached = segments
                    .into_iter()
                    .filter(|segment| {
                        let already_cached =
                            cache.iter().any(|cached| cached.index == segment.index);
                        !already_cached
                    })
                    .collect::<Vec<_>>();
                cache.extend(not_cached);

                let overflow_items = cache
                    .len()
                    .checked_sub(MAX_CACHED_SEGMENTS)
                    .unwrap_or_default();
                cache.drain(0..overflow_items);
            }

            let segment = cache.iter().find(|segment| segment.index == index).unwrap();
            Some(segment.data.clone())
        }
    }

    /// Expects a valid index
    async fn generate_segments_after(&self, index: usize) -> AppResult<Vec<MediaSegment>> {
        // Generate one extra segment before and after to not have trouble with any artifacting
        let mut segments = Vec::new();

        trace!("Generating segment {index}");

        let segmentation = self
            .playlist
            .read()
            .expect("This should never happen")
            .range_for_segment(index)
            .expect("Only none with invalid index");

        // TODO: use the ffmpeg-next bindings instead of this
        // once that is done, the tokio process feature can be removed
        let transcode_status = Command::new("ffmpeg")
            .current_dir(&self.temp_directory)
            .args([
                "-loglevel",
                FFMPEG_LOG_LEVEL,
                "-ss",
                &segmentation.start_time.to_string(),
                "-t",
                &segmentation.duration.to_string(),
                "-copyts",
                "-i",
                self.media_source.lock().await.to_str().unwrap(),
                "-map",
                "0",
                "-c",
                "copy",
                "-f",
                "segment",
                "-segment_times",
                &segmentation.segment_times,
                "-force_key_frames",
                &segmentation.keyframe_times,
                "-segment_start_number",
                &index.to_string(),
                "-segment_time_delta",
                "0.5",
                "-hls_flags",
                "independent_segments",
                "-segment_format",
                "mpegts",
                "%d.ts",
                "-y",
            ])
            .spawn()
            .with_context(|| "Failed to spawn ffmpeg")?
            .wait()
            .await
            .with_context(|| "Failed to wait for ffmpeg")?;

        if !transcode_status.success() {
            bail!("Failed to transcode segments");
        }

        let segment_path = self.temp_directory.join(format!("{index}.ts"));

        let data = fs::read(&segment_path)
            .with_context(|| format!("failed to read transcoded segment from {segment_path:?}"))?;

        tracing::trace!("Generated segment {index}");

        segments.push(MediaSegment { data, index });

        for file in fs::read_dir(&self.temp_directory)
            .with_context(|| "Failed to read temp directory for deletion")?
        {
            let file = file.with_context(|| "Failed to read temp directory entry")?;
            fs::remove_file(file.path())
                .with_context(|| format!("Failed to remove temp directory entry {file:?}"))?;
        }

        Ok(segments)
    }
}

impl Drop for MediaCache {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.temp_directory)
            .log_err_with_msg("Failed to remove temporary transcode directory");
    }
}

#[derive(Default, Clone, Copy)]
struct Segment {
    pub start_time: f64,
    pub duration: f64,
}

struct Segmentation {
    start_time: f64,
    duration: f64,
    segment_times: String,
    keyframe_times: String,
}

struct Playlist {
    pub file_repr: String,
    segments: Vec<Segment>,
}

impl Playlist {
    async fn generate(session_id: u32, temp_dir: &Path, media_source: &Path) -> AppResult<Self> {
        trace!("Generating playlist");
        let task = Command::new("ffmpeg")
            .current_dir(temp_dir)
            .args([
                "-loglevel",
                FFMPEG_LOG_LEVEL,
                "-copyts",
                "-i",
                media_source.to_str().unwrap(),
                "-map",
                "0",
                "-c",
                "copy",
                "-f",
                "hls",
                "-hls_playlist_type",
                "vod",
                "-hls_time",
                &format!("{}", SEGMENT_DURATION as u64),
                "-segment_time_delta",
                "0.5",
                "-hls_flags",
                "independent_segments",
                "-hls_segment_type",
                "mpegts",
                "-hls_segment_filename",
                &format!("{session_id}.%d"),
                "-y",
                "playlist.m3u8",
            ])
            .spawn()
            .with_context(|| "Failed to spawn ffmpeg")?
            .wait()
            .await
            .with_context(|| "Failed to wait for ffmpeg")?;

        if !task.success() {
            bail!("Failed to transcode segments");
        }

        let file_repr = fs::read_to_string(temp_dir.join("playlist.m3u8")).with_context(|| {
            format!(
                "failed to read playlist from tempt directory for {}",
                session_id
            )
        })?;

        // Remove all the files to not cause giant storage issues
        for file in
            fs::read_dir(temp_dir).with_context(|| "Failed to read temp directory for deletion")?
        {
            let file = file.with_context(|| "Failed to read temp directory entry")?;
            fs::remove_file(file.path())
                .with_context(|| format!("Failed to remove temp directory entry {file:?}"))?;
        }

        let segments = file_repr
            .lines()
            .filter(|line| line.starts_with("#EXTINF:"))
            .scan(0.0f64, |elapsed_time, line| {
                let time = line.parse_between(':', ',');

                let Ok(duration) = time else {
                    warn!("A playlist file format seems to be invalid");
                    return None;
                };

                let segment = Segment {
                    start_time: *elapsed_time,
                    duration,
                };

                *elapsed_time += duration;

                Some(segment)
            })
            .collect::<Vec<_>>();

        Ok(Self {
            file_repr,
            segments,
        })
    }

    /// Return None if the index is out of bounds
    fn range_for_segment(&self, index: usize) -> Option<Segmentation> {
        let segment = self.segments.get(index)?;

        let segment_times = format!("{}", segment.duration);
        let keyframe_times = format!(
            "{},{}",
            segment.start_time,
            segment.start_time + segment.duration
        );

        Some(Segmentation {
            start_time: segment.start_time,
            duration: segment.duration,
            segment_times,
            keyframe_times,
        })
    }
}
