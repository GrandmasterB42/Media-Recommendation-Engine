use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use crate::{
    database::Connection,
    state::{AppError, AppResult},
    utils::{bail, pseudo_random, relative, HandleErr},
};

use anyhow::Context;
use axum::{http::StatusCode, response::IntoResponse};
use tokio::process::Command;
use tracing::{trace, warn};

use super::{
    cache::{MediaCache, MediaSegment},
    SessionId, StreamIndex, StreamIndicies, FFMPEG_LOG_LEVEL, PRECOMPUTE_SEGMENTS,
    SEGMENT_DURATION,
};

pub enum MediaRequest {
    Playlist {
        streams: StreamIndicies,
    },
    Media {
        part: usize,
        streams: StreamIndicies,
    },
}

pub struct TranscodedStream {
    media_source: Mutex<PathBuf>,
    duration: Mutex<f64>,
    cache: MediaCache,
    temp_directory: PathBuf,
}

impl TranscodedStream {
    pub async fn new(media_source: &Path, session_id: SessionId) -> AppResult<Self> {
        let media_context = ffmpeg::format::input(&media_source)?;
        let duration = media_context.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE);

        let mut temp_directory = PathBuf::from(relative!("../tmp"));
        temp_directory.push(pseudo_random().to_string());

        fs::create_dir_all(&temp_directory)
            .log_err_with_msg("Failed to create temporary transcode directory");

        Ok(Self {
            media_source: Mutex::new(media_source.to_path_buf()),
            cache: MediaCache::new(media_source, session_id, &temp_directory).await?,
            duration: Mutex::new(duration),
            temp_directory,
        })
    }

    pub fn current_path(&self) -> PathBuf {
        self.media_source
            .lock()
            .expect("This should never happen")
            .clone()
    }

    pub async fn respond(
        &self,
        db: Connection,
        request: MediaRequest,
    ) -> AppResult<impl IntoResponse> {
        match request {
            MediaRequest::Playlist { streams } => Ok(self
                .respond_to_playlistrequest(db, streams)
                .await
                .into_response()),
            MediaRequest::Media { part, streams } => Ok(self
                .respond_to_mediarequest(part, streams)
                .await
                .into_response()),
        }
    }

    async fn respond_to_mediarequest(
        &self,
        index: usize,
        streams: StreamIndicies,
    ) -> AppResult<Vec<u8>> {
        let segment = self.request_segment(index, streams).await;

        if let Some(segment) = segment {
            Ok(segment)
        } else {
            let source = self.current_path();
            warn!("Failed to generate segment {index} for {source:?}");
            Err(AppError::Status(StatusCode::NOT_FOUND))
        }
    }

    /// Returns None if the segment is out of bounds or an error occured
    async fn request_segment(&self, index: usize, streams: StreamIndicies) -> Option<Vec<u8>> {
        let segment = self.cache.find_cached_segment(index, &streams);

        if let Some(segment) = segment {
            return Some(segment.data);
        }
        // Check for out of bounds index
        if (index as f64 * SEGMENT_DURATION.as_secs_f64())
            >= *self.duration.lock().expect("This should never happen")
        {
            return None;
        }

        let Some(segments) = self
            .generate_segments_after(index, &streams)
            .await
            .log_err()
        else {
            let source = self.media_source.lock().expect("This should never happen");
            warn!("Failed to generate segments after index {index} for {source:?}");
            return None;
        };
        self.cache.extend(segments);

        let segment = self.cache.find_cached_segment(index, &streams).unwrap();
        Some(segment.data.clone())
    }

    async fn respond_to_playlistrequest(
        &self,
        db: Connection,
        streams: StreamIndicies,
    ) -> AppResult<String> {
        let source = self.current_path();
        let playlist = self.request_playlist(db, &source, &streams).await;

        if let Some(playlist) = playlist {
            Ok(playlist)
        } else {
            warn!("Failed to generate playlist for path: {source:?} with streams: {streams:?}");
            Err(AppError::Status(StatusCode::NOT_FOUND))
        }
    }

    /// Returns None if the segment is out of bounds or an error occured
    async fn request_playlist(
        &self,
        db: Connection,
        source: &Path,
        streams: &StreamIndicies,
    ) -> Option<String> {
        let playlist = self.cache.find_cached_playlist(db, source, streams).await;

        if let Some(playlist) = playlist {
            return Some(playlist);
        }

        let Some(segments) = self
            .generate_segments_after(index, &streams)
            .await
            .log_err()
        else {
            let source = self.media_source.lock().expect("This should never happen");
            warn!("Failed to generate segments after index {index} for {source:?}");
            return None;
        };
        self.cache.extend(segments);

        let segment = self.cache.find_cached_segment(index, &streams).unwrap();
        Some(segment.data.clone())
    }

    pub async fn reuse(&self, media_source: &Path) -> AppResult<()> {
        self.media_source
            .lock()
            .expect("This should never happen")
            .clone_from(&media_source.to_path_buf());

        self.cache.reuse(media_source, &self.temp_directory).await?;

        let media_context = ffmpeg::format::input(&media_source)?;
        let duration = media_context.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE);

        self.duration
            .lock()
            .expect("This should never happen")
            .clone_from(&duration);

        Ok(())
    }

    /// Expects a valid index
    pub async fn generate_segments_after(
        &self,
        requested_index: usize,
        streams: &StreamIndicies,
    ) -> AppResult<Vec<MediaSegment>> {
        // Generate one extra segment before and after to not have trouble with any artifacting
        let mut segments = Vec::new();

        trace!("Processing Request for segment {requested_index}");

        let segmentation = self
            .cache
            .segmentation_for_segment(requested_index)
            .await
            .expect("Only none with invalid index");

        // TODO: use the ffmpeg-next bindings instead of this
        // once that is done, the tokio process feature can be removed
        // Some arguments might not be necessary, more testing is needed

        let mut args = vec![
            "-loglevel".to_string(),
            FFMPEG_LOG_LEVEL.to_string(),
            "-copyts".to_string(),
            "-ss".to_string(),
            segmentation.start_time.to_string(),
            "-t".to_string(),
            segmentation.duration.to_string(),
            "-i".to_string(),
            self.media_source
                .lock()
                .expect("This should never happen")
                .to_str()
                .unwrap()
                .to_string(),
            "-c".to_string(),
            "copy".to_string(),
            "-f".to_string(),
            "segment".to_string(),
            "-segment_times".to_string(),
            segmentation.segment_times,
            "-force_key_frames".to_string(),
            segmentation.keyframe_times,
            "-segment_start_number".to_string(),
            segmentation.start_index.to_string(),
            "-segment_time_delta".to_string(),
            "0.5".to_string(),
            "-hls_flags".to_string(),
            "independent_segments".to_string(),
            "-segment_format".to_string(),
            "mpegts".to_string(),
        ];

        let path_template = format!("%d.{}.ts", streams.str_repr);
        let file_path = |segment_index: usize| format!("{segment_index}.{}.ts", streams.str_repr);

        {
            let inp = ffmpeg::format::input(&*self.media_source.lock().unwrap())?;
            for stream in &streams.streams {
                match stream {
                    StreamIndex::Video => {
                        if let Some(i) = inp.streams().best(ffmpeg::media::Type::Video) {
                            args.insert(9, "-map".to_string());
                            args.insert(10, format!("0:{}", i.index()))
                        }
                    }
                    StreamIndex::Audio => {
                        if let Some(i) = inp.streams().best(ffmpeg::media::Type::Audio) {
                            args.insert(9, "-map".to_string());
                            args.insert(10, format!("0:{}", i.index()))
                        }
                    }
                    StreamIndex::Index(index) => {
                        args.insert(9, "-map".to_string());
                        args.insert(10, format!("0:{index}"))
                    }
                }
            }
        }

        args.push(path_template);
        args.push("-y".to_string());

        let transcode_status = Command::new("ffmpeg")
            .current_dir(&self.temp_directory)
            .args(args)
            .spawn()
            .with_context(|| "Failed to spawn ffmpeg")?
            .wait()
            .await
            .with_context(|| "Failed to wait for ffmpeg")?;

        if !transcode_status.success() {
            bail!("Failed to transcode segments");
        }

        for index in requested_index..(requested_index + PRECOMPUTE_SEGMENTS) {
            let segment_path = self.temp_directory.join(file_path(index));
            let Ok(data) = fs::read(&segment_path) else {
                break;
            };

            fs::remove_file(&segment_path).with_context(|| {
                format!("Failed to remove transcoding artifact: {segment_path:?}")
            })?;

            tracing::trace!("Generated segment {index} for stream {}", streams.str_repr);

            segments.push(MediaSegment {
                data,
                index,
                stream_ident: streams.str_repr.clone(),
            });
        }

        Ok(segments)
    }
}

impl Drop for TranscodedStream {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.temp_directory)
            .log_err_with_msg("Failed to remove temporary transcode directory");
    }
}
