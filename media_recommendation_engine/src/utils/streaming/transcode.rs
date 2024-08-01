use std::{
    borrow::Cow,
    fmt::Write,
    fs,
    path::{Path, PathBuf},
    sync::RwLock,
    time::Duration,
};

use tokio::{process::Command, sync::Mutex};

use crate::{
    state::{AppError, AppResult},
    utils::{bail, pseudo_random, relative, HandleErr, ParseBetween},
};

use anyhow::Context;
use axum::{http::StatusCode, response::IntoResponse};
use tracing::{trace, warn};

const SEGMENT_DURATION: Duration = Duration::from_secs(10);
const MAX_CACHED_SEGMENTS: usize = 64;
// One segment at a time tends to cause more artifacting and similar
const PRECOMPUTE_SEGMENTS: usize = if cfg!(debug_assertions) { 1 } else { 8 };

const FFMPEG_LOG_LEVEL: &str = if cfg!(debug_assertions) {
    "warning"
} else {
    "fatal"
};

pub enum MediaRequest {
    MasterPlaylist,
    TrackPlaylist { index: usize },
    VideoSegment { index: usize },
    AudioSegment { index: usize, language_index: usize },
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
            MediaRequest::MasterPlaylist => Ok(self
                .media_cache
                .playlist
                .read()
                .expect("This should never happen")
                .master_playlist
                .clone()
                .into_response()),
            MediaRequest::TrackPlaylist { index } => {
                let playlist = self
                    .media_cache
                    .playlist
                    .read()
                    .expect("This should never happen")
                    .get_playlist_for_track(index);
                Ok(playlist.into_response())
            }
            MediaRequest::VideoSegment { index } => Ok(self
                .respond_to_mediarequest(index, None)
                .await
                .into_response()),
            MediaRequest::AudioSegment {
                index,
                language_index,
            } => Ok(self
                .respond_to_mediarequest(index, Some(language_index))
                .await
                .into_response()),
        }
    }

    async fn respond_to_mediarequest(
        &self,
        index: usize,
        language_index: Option<usize>,
    ) -> AppResult<Vec<u8>> {
        let segment = self
            .media_cache
            .request_segment(index, language_index)
            .await;

        match segment {
            Some(segment) => Ok(segment),
            None => {
                let source = self.media_source.lock().await;
                warn!("Failed to generate segment {index} for {source:?}");
                Err(AppError::Status(StatusCode::NOT_FOUND))
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

#[derive(Clone, Copy, PartialEq)]
enum SegmentType {
    Video,
    Audio { language_index: usize },
}

impl SegmentType {
    fn ffmpeg_mapping(&self) -> Cow<'_, str> {
        match self {
            SegmentType::Video => Cow::Borrowed("0:v:0"),
            SegmentType::Audio { language_index } => Cow::Owned(format!("0:{language_index}")),
        }
    }

    fn file_path(&self, index: usize) -> String {
        match self {
            SegmentType::Video => format!("{index}.ts"),
            SegmentType::Audio { language_index } => format!("{index}.{language_index}.ts"),
        }
    }
}

#[derive(Clone)]
struct MediaSegment {
    pub data: Vec<u8>,
    index: usize,
    typ: SegmentType,
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
    async fn find_cached_segment(&self, index: usize, typ: SegmentType) -> Option<MediaSegment> {
        self.cache
            .read()
            .expect("This should never happen")
            .iter()
            .find(|segment| segment.index == index && segment.typ == typ)
            .cloned()
    }

    async fn extend_cache(&self, segments: Vec<MediaSegment>) {
        let mut cache = self.cache.write().expect("This should never happen");

        let not_cached = segments
            .into_iter()
            .filter(|segment| {
                let already_cached = cache
                    .iter()
                    .any(|cached| cached.index == segment.index && cached.typ == segment.typ);
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
    async fn request_segment(
        &self,
        index: usize,
        language_index: Option<usize>,
    ) -> Option<Vec<u8>> {
        let segment_type = if let Some(language_index) = language_index {
            SegmentType::Audio { language_index }
        } else {
            SegmentType::Video
        };

        let segment = self.find_cached_segment(index, segment_type).await;

        if let Some(segment) = segment {
            return Some(segment.data);
        }
        // Check for out of bounds index
        if (index as f64 * SEGMENT_DURATION.as_secs_f64()) >= *self.duration.lock().await {
            return None;
        }

        let Some(segments) = self
            .generate_segments_after(index, segment_type)
            .await
            .log_err()
        else {
            let source = self.media_source.lock().await;
            warn!("Failed to generate segments after index {index} for {source:?}");
            return None;
        };
        self.extend_cache(segments).await;

        let segment = self.find_cached_segment(index, segment_type).await.unwrap();
        Some(segment.data.clone())
    }

    /// Expects a valid index
    async fn generate_segments_after(
        &self,
        index: usize,
        segment_type: SegmentType,
    ) -> AppResult<Vec<MediaSegment>> {
        // Generate one extra segment before and after to not have trouble with any artifacting
        let mut segments = Vec::new();

        trace!("Processing Request for segment {index}");

        let segmentation = self
            .playlist
            .read()
            .expect("This should never happen")
            .range_for_segment(index)
            .expect("Only none with invalid index");

        // TODO: use the ffmpeg-next bindings instead of this
        // once that is done, the tokio process feature can be removed
        // Some arguments might not be necessary, more testing is needed

        let mut args = vec![
            "-loglevel".to_string(),
            FFMPEG_LOG_LEVEL.to_string(),
            "-ss".to_string(),
            segmentation.start_time.to_string(),
            "-t".to_string(),
            segmentation.duration.to_string(),
            "-copyts".to_string(),
            "-i".to_string(),
            self.media_source.lock().await.to_str().unwrap().to_string(),
            "-map".to_string(),
            segment_type.ffmpeg_mapping().to_string(),
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

        if let SegmentType::Audio { language_index } = segment_type {
            // This mapping the video into the audio stream is a workaround for audio gaps
            args.insert(11, "-map".to_string());
            args.insert(12, "0:v".to_string());
            args.push(format!("%d.{language_index}.ts"));
        } else {
            args.push("%d.ts".to_string());
        }
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

        for index in index..(index + PRECOMPUTE_SEGMENTS) {
            let segment_path = self.temp_directory.join(segment_type.file_path(index));
            let Ok(data) = fs::read(&segment_path) else {
                break;
            };

            fs::remove_file(&segment_path).with_context(|| {
                format!("Failed to remove transcoding artifact: {segment_path:?}")
            })?;

            tracing::trace!("Generated segment {index}");

            tracing::trace!(
                "{}",
                match segment_type {
                    SegmentType::Video => "Video".to_string(),
                    SegmentType::Audio { language_index } => format!("Audio {language_index}"),
                }
            );

            segments.push(MediaSegment {
                data,
                index,
                typ: segment_type,
            });
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
    start_index: usize,
    start_time: f64,
    duration: f64,
    segment_times: String,
    keyframe_times: String,
}

struct Playlist {
    master_playlist: String,
    general_playlist: String,
    segments: Vec<Segment>,
}

impl Playlist {
    async fn generate(session_id: u32, temp_dir: &Path, media_source: &Path) -> AppResult<Self> {
        let input = ffmpeg::format::input(&media_source)?;

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
                "0:a",
                "-map",
                "0:v",
                "-c",
                "copy",
                "-f",
                "segment",
                "-hls_playlist_type",
                "vod",
                "-segment_time",
                &format!("{}", SEGMENT_DURATION.as_secs_f64()),
                "-segment_time_delta",
                "0.5",
                "-hls_flags",
                "independent_segments",
                "-segment_list",
                "playlist.m3u8",
                "-segment_list_type",
                "m3u8",
                "-y",
                &format!("{session_id}.%d.ts"),
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
                "failed to read playlist from temp directory for {}",
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

        let audio_streams = input
            .streams()
            .map(|stream| {
                let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters());
                (stream, context)
            })
            .filter_map(|(stream, codec)| {
                let Ok(codec) = codec else {
                    return None;
                };

                let medium = codec.medium();
                let decoder = codec.decoder().audio();

                if medium == ffmpeg::media::Type::Audio && decoder.is_ok() {
                    let decoder = decoder.unwrap();
                    Some((stream, decoder))
                } else {
                    None
                }
            })
            .map(|(stream, audio)| {
                let index = stream.index();
                let language = stream.metadata().get("language").unwrap_or("").to_string();
                let channel_layout = audio.channel_layout();
                (index, language, channel_layout)
            });

        let video_stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .with_context(|| format!("Failed to find video stream for {media_source:?}"))?;

        let codec = ffmpeg::codec::context::Context::from_parameters(video_stream.parameters())?;
        let decoder = codec.decoder().video();
        let Ok(decoder) = decoder else {
            bail!("Failed to find video decoder");
        };

        let width = decoder.width();
        let height = decoder.height();

        let mut master_playlist = String::from("#EXTM3U\n");

        let mut first = true;
        for (index, language, channel_layout) in audio_streams {
            master_playlist.push_str(&format!(
                "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio_group\",NAME=\"{language} Channels:{channel_layout}\",DEFAULT={default},LANGUAGE=\"{language}\",URI=\"{session_id}.{index}.m3u8\"\n",
                default = if first {
                    first = false;
                    "YES"
                } else {
                    "NO"
                },
                channel_layout = channel_layout.channels()
            ));
        }
        master_playlist.push_str(&format!("#EXT-X-STREAM-INF:BANDWIDTH=1000000,RESOLUTION={width}x{height},AUDIO=\"audio_group\",CODECS=\"avc1.42e00a,mp4a.40.2\"\n"));
        master_playlist.push_str(&format!("{session_id}.0.m3u8\n"));

        Ok(Self {
            master_playlist,
            general_playlist: file_repr,
            segments,
        })
    }

    fn get_playlist_for_track(&self, index: usize) -> impl IntoResponse {
        if index == 0 {
            return self.general_playlist.clone();
        }

        self.general_playlist
            .lines()
            .fold(String::new(), |mut s, line| {
                if line.ends_with(".ts") {
                    let without_ts = line.trim_end_matches(".ts");
                    writeln!(&mut s, "{without_ts}.{index}.ts").unwrap();
                } else {
                    writeln!(&mut s, "{line}").unwrap();
                };
                s
            })
    }

    /// Return None if the index is out of bounds
    fn range_for_segment(&self, index: usize) -> Option<Segmentation> {
        let mut start_time = -1.0;
        let mut duration = 0.0;
        let mut segment_times = String::new();
        let mut keyframe_times = String::new();

        let last_index = (index + PRECOMPUTE_SEGMENTS) - 1;
        for segment_index in index..=last_index {
            let Some(segment) = self.segments.get(segment_index) else {
                break;
            };

            if start_time == -1.0 {
                start_time = segment.start_time;
            }

            segment_times.push_str(&format!("{},", duration + segment.duration));
            keyframe_times.push_str(&format!("{},", segment.start_time));

            duration += segment.duration;

            if segment_index == last_index {
                keyframe_times.push_str(&format!("{},", segment.start_time + segment.duration));
            }
        }

        let segment_times = segment_times.trim_end_matches(',').to_string();
        let keyframe_times = keyframe_times.trim_end_matches(',').to_string();

        Some(Segmentation {
            start_index: index,
            start_time,
            duration,
            segment_times,
            keyframe_times,
        })
    }
}
