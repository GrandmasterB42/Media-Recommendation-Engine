use std::{fmt::Write, fs, path::Path};

use anyhow::Context;
use rusqlite::{params, OptionalExtension};
use tokio::process::Command;
use tracing::trace;

use crate::{
    database::Connection,
    database::QueryRowGetConnExt,
    indexing::AsDBString,
    state::AppResult,
    utils::{bail, ParseUntil},
};

use super::{SessionId, StreamIndicies, FFMPEG_LOG_LEVEL, PRECOMPUTE_SEGMENTS, SEGMENT_DURATION};

#[derive(Default, Clone, Copy)]
pub struct Segment {
    pub start_time: f64,
    pub duration: f64,
}

pub struct Segmentation {
    pub start_index: usize,
    pub start_time: f64,
    pub duration: f64,
    pub segment_times: String,
    pub keyframe_times: String,
}

#[derive(Clone)]
pub struct Playlist {
    identifier: String,
    string_repr: String,
}

pub struct Playlists {
    segments: Vec<Segment>,
    playlists: Vec<Playlist>,
    pub max_duration: f64,
}

impl Playlists {
    pub async fn new(media_source: &Path, temp_dir: &Path) -> AppResult<Self> {
        trace!("Generating playlist segmentation for {media_source:?} for stream",);

        // TODO: Switch back to real playlists with on the fly intermediate file deletion
        let probe_task = Command::new("ffprobe")
            .current_dir(temp_dir)
            .args([
                "-loglevel",
                FFMPEG_LOG_LEVEL,
                "-select_streams",
                "0",
                "-show_entries",
                "packet=pts_time,flags",
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
        let file = fs::read_to_string(&path)
            .with_context(|| format!("failed to read probe file from {temp_dir:?}"))?;
        fs::remove_file(&path)
            .with_context(|| format!("failed to remove probe file from {temp_dir:?}"))?;

        let mut target_timestamps: Vec<f64> = Vec::new();

        let mut timestamps = file
            .lines()
            .filter(|line| line.contains('K'))
            .map(|line| line.parse_until(',').expect("Wrongly formatted file"))
            .peekable();

        let mut last_split_time = 0.0;
        let mut max_duration: f64 = 0.0;
        let mut index = 0;

        let mut segments = Vec::new();

        while let Some(segment_timestamp) = timestamps.next() {
            let duration = segment_timestamp - last_split_time;

            let target_timestamp = target_timestamps.get(index).cloned().unwrap_or_else(|| {
                target_timestamps
                    .get(index.checked_sub(1).unwrap_or_default())
                    .unwrap_or(&0.0)
                    + SEGMENT_DURATION.as_secs_f64()
            });

            // limit addition of frames to the one closest to the desired timestamp -> minimal desync
            if let Some(next_segment_timestamp) = timestamps.peek() {
                if (next_segment_timestamp - target_timestamp).abs()
                    < (segment_timestamp - target_timestamp).abs()
                {
                    continue;
                }
            }

            target_timestamps.push(segment_timestamp);

            segments.push(Segment {
                start_time: last_split_time,
                duration,
            });

            max_duration = max_duration.max(duration);

            last_split_time = segment_timestamp;
            index += 1;
        }

        Ok(Self {
            segments,
            playlists: Vec::new(),
            max_duration,
        })
    }

    pub async fn get_playlist_for_streams(
        &mut self,
        db: Connection,
        session_id: SessionId,
        media_source: &Path,
        streams: &StreamIndicies,
    ) -> AppResult<String> {
        let maybe_cache = self
            .playlists
            .iter()
            .find(|playlist| playlist.identifier == streams.str_repr)
            .cloned();

        if let Some(Playlist {
            identifier: _,
            string_repr: playlist,
        }) = maybe_cache
        {
            return Ok(playlist);
        }

        let content_id = db.query_row_get::<u32>(
            "SELECT content.id FROM content, data_file WHERE content.data_id = data_file.id AND data_file.path = ?1",
            [&media_source.as_db_string()],
        ).unwrap();

        let db_file = db
            .query_row_get::<String>(
                "SELECT content_playlist.playlist FROM content_playlist WHERE content_playlist.content_id = ?1 AND content_playlist.stream_index = ?2",
                params![content_id, streams.str_repr]
            )
            .optional()?;

        let playlist = if let Some(file) = db_file {
            let file = file.lines().fold(String::new(), |mut file, line| {
                if !line.starts_with('#') {
                    let rest = line.split_once('.').unwrap().1;
                    writeln!(file, "{session_id}.{rest}").unwrap();
                } else {
                    writeln!(file, "{line}").unwrap();
                }
                file
            });

            self.playlists.push(Playlist {
                identifier: streams.str_repr.clone(),
                string_repr: file.clone(),
            });

            file
        } else {
            let mut fake_playlist = String::new();

            writeln!(fake_playlist, "#EXTM3U").unwrap();
            writeln!(fake_playlist, "#EXT-X-VERSION:3").unwrap();
            writeln!(fake_playlist, "#EXT-X-MEDIA-SEQUENCE:0").unwrap();
            writeln!(fake_playlist, "#EXT-X-ALLOW-CACHE:YES").unwrap();
            writeln!(fake_playlist, "#EXT-X-TARGETDURATION:{}", self.max_duration).unwrap();

            for (i, segment) in self.segments.iter().enumerate() {
                write!(
                    fake_playlist,
                    "#EXTINF:{}\n{}.{i}.{}.ts\n",
                    segment.duration, session_id, &streams.str_repr
                )
                .unwrap();
            }

            writeln!(fake_playlist, "#EXT-X-ENDLIST").unwrap();

            self.playlists.push(Playlist {
                identifier: streams.str_repr.clone(),
                string_repr: fake_playlist.clone(),
            });

            // TODO: Make the "playlist contain just the timestamps to reconstruct from"
            db.execute(
                    "INSERT INTO content_playlist (content_id, stream_index, playlist) VALUES (?1, ?2, ?3)",
                    params![content_id, streams.str_repr, fake_playlist.clone()]
                )?;

            fake_playlist
        };

        Ok(playlist)
    }

    /// Return None if the index is out of bounds
    pub fn segmentation_for_segment(&self, index: usize) -> Option<Segmentation> {
        let mut start_time = -1.0;
        let mut duration = 0.0;
        let mut segment_times = String::new();
        let mut keyframe_times = String::new();

        let last_index = index + PRECOMPUTE_SEGMENTS - 1;
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
