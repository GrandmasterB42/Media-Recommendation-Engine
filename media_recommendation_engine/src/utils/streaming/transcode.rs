use std::{
    fmt::Write,
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, RwLock},
    time::Duration,
};

use rusqlite::{params, OptionalExtension};
use tokio::process::Command;

use crate::{
    database::{Connection, QueryRowGetConnExt},
    indexing::AsDBString,
    state::{AppError, AppResult},
    utils::{bail, pseudo_random, relative, HandleErr, ParseBetween},
};

use anyhow::Context;
use axum::{http::StatusCode, response::IntoResponse};
use tracing::{trace, warn};

const SEGMENT_DURATION: Duration = Duration::from_secs(10);
const MAX_CACHED_SEGMENTS: usize = 32;
// Fewer segments at a time tends to cause more artifacting, requests, ..
const PRECOMPUTE_SEGMENTS: usize = if cfg!(debug_assertions) { 1 } else { 4 };

const FFMPEG_LOG_LEVEL: &str = if cfg!(debug_assertions) {
    "warning"
} else {
    "fatal"
};

pub enum MediaRequest {
    MasterPlaylist,
    TrackPlaylist { index: usize },
    Media { part: usize, stream_index: usize },
}

pub struct TranscodedStream {
    media_source: Mutex<PathBuf>,
    media_cache: MediaCache,
}

impl TranscodedStream {
    pub fn new(media_source: &Path, session_id: u32) -> AppResult<Self> {
        let media_context = ffmpeg::format::input(&media_source)?;
        let duration = media_context.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE);

        let mut temp_dir = PathBuf::from(relative!("../tmp"));
        temp_dir.push(pseudo_random().to_string());

        Ok(Self {
            media_source: Mutex::new(media_source.to_path_buf()),
            media_cache: MediaCache::new(media_source, &temp_dir, duration, session_id)?,
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
            MediaRequest::MasterPlaylist => Ok(self
                .media_cache
                .playlist
                .read()
                .await
                .master_playlist
                .clone()
                .into_response()),
            MediaRequest::TrackPlaylist { index } => {
                let playlist = self.media_cache.get_playlist_for_stream(db, index).await?;
                Ok(playlist.into_response())
            }
            MediaRequest::Media { part, stream_index } => Ok(self
                .respond_to_mediarequest(part, stream_index)
                .await
                .into_response()),
        }
    }

    async fn respond_to_mediarequest(&self, index: usize, stream: usize) -> AppResult<Vec<u8>> {
        let segment = self.media_cache.request_segment(index, stream).await;

        if let Some(segment) = segment {
            Ok(segment)
        } else {
            let source = self.current_path();
            warn!("Failed to generate segment {index} for {source:?}");
            Err(AppError::Status(StatusCode::NOT_FOUND))
        }
    }

    pub async fn reuse(&self, media_source: &Path) -> AppResult<()> {
        self.media_source
            .lock()
            .expect("This should never happen")
            .clone_from(&media_source.to_path_buf());

        let media_context = ffmpeg::format::input(&media_source)?;
        let duration = media_context.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE);

        self.media_cache.clear(media_source, duration).await?;

        Ok(())
    }
}

#[derive(Clone)]
struct MediaSegment {
    pub data: Vec<u8>,
    index: usize,
    stream: usize,
}

struct MediaCache {
    session_id: u32,
    media_source: Mutex<PathBuf>,
    cache: RwLock<Vec<MediaSegment>>,
    playlist: tokio::sync::RwLock<Playlist>,
    temp_directory: PathBuf,
    duration: Mutex<f64>,
}

impl MediaCache {
    fn find_cached_segment(&self, index: usize, stream: usize) -> Option<MediaSegment> {
        self.cache
            .read()
            .expect("This should never happen")
            .iter()
            .find(|segment| segment.index == index && segment.stream == stream)
            .cloned()
    }

    fn extend_cache(&self, segments: Vec<MediaSegment>) {
        let mut cache = self.cache.write().expect("This should never happen");

        let not_cached = segments
            .into_iter()
            .filter(|segment| {
                let already_cached = cache
                    .iter()
                    .any(|cached| cached.index == segment.index && cached.stream == segment.stream);
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

    async fn get_playlist_for_stream(
        &self,
        db: Connection,
        stream_idx: usize,
    ) -> AppResult<String> {
        let temp_dir = self.temp_directory.clone();

        let media_source = self
            .media_source
            .lock()
            .expect("This should never happen")
            .clone();

        let playlist = self.playlist.read().await;
        playlist
            .get_playlist_for_stream(db, self.session_id, temp_dir, media_source, stream_idx)
            .await
    }
}

impl MediaCache {
    fn new(
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
            playlist: tokio::sync::RwLock::new(Playlist::generate_master(
                session_id,
                media_source,
            )?),
            temp_directory: temp_directory.to_path_buf(),
            duration: Mutex::new(duration),
        })
    }

    async fn clear(&self, media_source: &Path, duration: f64) -> AppResult<()> {
        self.media_source
            .lock()
            .expect("This should never happen")
            .clone_from(&media_source.to_path_buf());

        self.duration
            .lock()
            .expect("This should never happen")
            .clone_from(&duration);

        self.cache
            .write()
            .expect("This should never happen")
            .clear();

        *self.playlist.write().await = Playlist::generate_master(self.session_id, media_source)?;

        Ok(())
    }

    /// Returns None if the segment is out of bounds or an error occured
    async fn request_segment(&self, index: usize, stream: usize) -> Option<Vec<u8>> {
        let segment = self.find_cached_segment(index, stream);

        if let Some(segment) = segment {
            return Some(segment.data);
        }
        // Check for out of bounds index
        if (index as f64 * SEGMENT_DURATION.as_secs_f64())
            >= *self.duration.lock().expect("This should never happen")
        {
            return None;
        }

        let Some(segments) = self.generate_segments_after(index, stream).await.log_err() else {
            let source = self.media_source.lock().expect("This should never happen");
            warn!("Failed to generate segments after index {index} for {source:?}");
            return None;
        };
        self.extend_cache(segments);

        let segment = self.find_cached_segment(index, stream).unwrap();
        Some(segment.data.clone())
    }

    /// Expects a valid index
    async fn generate_segments_after(
        &self,
        requested_index: usize,
        stream_index: usize,
    ) -> AppResult<Vec<MediaSegment>> {
        // Generate one extra segment before and after to not have trouble with any artifacting
        let mut segments = Vec::new();

        trace!("Processing Request for segment {requested_index}");

        let segmentation = self
            .playlist
            .read()
            .await
            .range_for_segment(requested_index, stream_index)
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
            self.media_source
                .lock()
                .expect("This should never happen")
                .to_str()
                .unwrap()
                .to_string(),
            "-map".to_string(),
            format!("0:{stream_index}"),
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

        let path_template = format!("%d.{stream_index}.ts");
        let file_path = |segment_index: usize| format!("{segment_index}.{stream_index}.ts");

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

            tracing::trace!("Generated segment {index} for stream {stream_index}");

            segments.push(MediaSegment {
                data,
                index,
                stream: stream_index,
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
    segments: Mutex<Vec<(usize, String, Vec<Segment>)>>,
}

impl Playlist {
    fn generate_master(session_id: u32, media_source: &Path) -> AppResult<Self> {
        let input = ffmpeg::format::input(&media_source)?;
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
        master_playlist.push_str(&format!("{session_id}.{}.m3u8\n", video_stream.index()));

        Ok(Self {
            master_playlist,
            segments: Mutex::new(Vec::new()),
        })
    }

    async fn get_playlist_for_stream(
        &self,
        db: Connection,
        session_id: u32,
        temp_dir: PathBuf,
        media_source: PathBuf,
        stream_idx: usize,
    ) -> AppResult<String> {
        let maybe_cache = self
            .segments
            .lock()
            .expect("This should never happen")
            .iter()
            .find(|(idx, _, _)| *idx == stream_idx)
            .cloned();

        if let Some((_, playlist, _)) = maybe_cache {
            return Ok(playlist);
        }

        let content_id = db.query_row_get::<u32>(
            "SELECT content.id FROM content, data_file WHERE content.data_id = data_file.id AND data_file.path = ?1",
            [&media_source.as_db_string()],
        ).unwrap();

        let db_file = db
            .query_row_get::<String>(
                "SELECT content_playlist.playlist FROM content_playlist WHERE content_playlist.content_id = ?1 AND content_playlist.stream_index = ?2",
                params![content_id, stream_idx]
            )
            .optional()?;

        let playlist = if let Some(file) = db_file {
            let segments = file
                .lines()
                .filter(|line| line.starts_with("#EXTINF:"))
                .scan(0.0, |elapsed_time, line| {
                    let duration = line.parse_between(':', ',').ok()?;

                    let segment = Segment {
                        start_time: *elapsed_time,
                        duration,
                    };

                    *elapsed_time += duration;

                    Some(segment)
                })
                .collect::<Vec<_>>();

            let file = file.lines().fold(String::new(), |mut file, line| {
                if !line.starts_with('#') {
                    let rest = line.split_once('.').unwrap().1;
                    writeln!(file, "{session_id}.{rest}").unwrap();
                } else {
                    writeln!(file, "{line}").unwrap();
                }
                file
            });

            self.segments
                .lock()
                .expect("This should never happen")
                .push((stream_idx, file.clone(), segments));

            file
        } else {
            trace!("Generating playlist for {media_source:?} for stream {stream_idx}");
            let probe_task = Command::new("ffprobe")
                .current_dir(&temp_dir)
                .args([
                    "-loglevel",
                    FFMPEG_LOG_LEVEL,
                    "-show_entries",
                    "packet=pts_time,flags,stream_index",
                    "-of",
                    "csv=print_section=0",
                    "-o",
                    "probe.csv",
                    "-i",
                    media_source.to_str().unwrap(),
                ])
                .spawn()
                .with_context(|| "Failed to spawn ffprobe")?
                .wait()
                .await
                .with_context(|| "Failed to wait for ffprobe")?;

            if !probe_task.success() {
                bail!("Failed to probe media");
            }

            let path = temp_dir.join("probe.csv");
            let file = fs::read_to_string(&path).with_context(|| {
                format!("failed to read probe file from temp directory for {session_id}")
            })?;
            fs::remove_file(&path).with_context(|| {
                format!("failed to remove probe file from temp directory for {session_id}")
            })?;

            let inp = ffmpeg::format::input(&media_source)?;

            let mut returned_playlist = String::new();
            for stream in inp.streams() {
                let medium = {
                    let codec = ffmpeg::codec::Context::from_parameters(stream.parameters())?;
                    codec.medium()
                };

                let this_idx = stream.index();

                match medium {
                    ffmpeg::media::Type::Video | ffmpeg::media::Type::Audio => (),
                    _ => {
                        trace!("Encountered unsupported stream type while generating playlist for stream {this_idx}");
                        continue;
                    }
                }

                let mut fake_playlist = String::new();
                let mut segments = Vec::new();

                writeln!(fake_playlist, "#EXTM3U").unwrap();
                writeln!(fake_playlist, "#EXT-X-VERSION:3").unwrap();
                writeln!(fake_playlist, "#EXT-X-MEDIA-SEQUENCE:0").unwrap();
                writeln!(fake_playlist, "#EXT-X-ALLOW-CACHE:YES").unwrap();
                writeln!(fake_playlist, "#EXT-X-TARGETDURATION:HERE",).unwrap();

                let mut last_split_time = 0.0;
                let mut max_duration: f64 = 0.0;
                let mut index = 0;
                let mut lines = file
                    .lines()
                    .filter(|line| line.starts_with(&format!("{this_idx}")))
                    .filter(|line| line.contains('K'))
                    .peekable();

                let first_keyframe = lines.peek().unwrap();
                let first_time: f64 = first_keyframe.parse_between(',', ',').unwrap();
                if first_time != 0.0 {
                    last_split_time = first_time;
                    writeln!(fake_playlist, "#EXT-X-START:TIME_OFFSET={first_time:.8}").unwrap();
                }

                while let Some(line) = lines.next() {
                    let segment_timestamp = line
                        .parse_between(',', ',')
                        .expect("Wrongly formatted file");

                    let target_duration = SEGMENT_DURATION.as_secs_f64();
                    let duration = segment_timestamp - last_split_time;
                    let tolerance = 2.0;
                    if !(lines.peek().is_none() || duration > (target_duration - tolerance)) {
                        continue;
                    }

                    writeln!(fake_playlist, "#EXTINF:{duration:.8}",).unwrap();
                    writeln!(fake_playlist, "{session_id}.{index}.{this_idx}.ts").unwrap();

                    segments.push(Segment {
                        start_time: last_split_time,
                        duration,
                    });

                    max_duration = max_duration.max(duration);

                    last_split_time = segment_timestamp;
                    index += 1;
                }

                let replace_index = fake_playlist.find("HERE").unwrap();
                fake_playlist.replace_range(
                    replace_index..replace_index + 4,
                    &format!("{max_duration:.8}"),
                );

                writeln!(fake_playlist, "#EXT-X-ENDLIST").unwrap();

                self.segments
                    .lock()
                    .expect("This should never happen")
                    .push((this_idx, fake_playlist.clone(), segments));

                db.execute(
                    "INSERT INTO content_playlist (content_id, stream_index, playlist) VALUES (?1, ?2, ?3)",
                    params![content_id, this_idx, fake_playlist]
                )?;

                if this_idx == stream_idx {
                    returned_playlist = fake_playlist;
                }
            }
            returned_playlist
        };

        Ok(playlist)
    }

    /// Return None if the index is out of bounds
    fn range_for_segment(&self, index: usize, stream_index: usize) -> Option<Segmentation> {
        let mut start_time = -1.0;
        let mut duration = 0.0;
        let mut segment_times = String::new();
        let mut keyframe_times = String::new();

        let (_, _, segments) = self
            .segments
            .lock()
            .expect("This should never happen")
            .iter()
            .find(|(stream_idx, _, _)| *stream_idx == stream_index)
            .cloned()?;

        let last_index = (index + PRECOMPUTE_SEGMENTS) - 1;
        for segment_index in index..=last_index {
            let Some(segment) = segments.get(segment_index) else {
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
