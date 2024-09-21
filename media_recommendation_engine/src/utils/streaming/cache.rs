use std::{
    path::{Path, PathBuf},
    sync::{Mutex, RwLock},
};

use crate::{database::Connection, state::AppResult};

use super::{
    playlist::{Playlists, Segmentation},
    SessionId, StreamIndicies, MAX_CACHED_SEGMENTS,
};

#[derive(Clone)]
pub struct MediaSegment {
    pub data: Vec<u8>,
    pub index: usize,
    pub stream_ident: String,
}

pub struct MediaCache {
    pub session_id: SessionId,
    pub media_source: Mutex<PathBuf>,
    playlist: tokio::sync::RwLock<Playlists>,
    cache: RwLock<Vec<MediaSegment>>,
}

impl MediaCache {
    pub async fn new(
        media_source: &Path,
        session_id: SessionId,
        temp_directory: &Path,
    ) -> AppResult<Self> {
        Ok(Self {
            session_id,
            media_source: Mutex::new(media_source.to_path_buf()),
            cache: RwLock::new(Vec::new()),
            playlist: tokio::sync::RwLock::new(Playlists::new(media_source, temp_directory).await?),
        })
    }

    pub async fn reuse(&self, media_source: &Path, temp_directory: &Path) -> AppResult<()> {
        self.media_source
            .lock()
            .expect("This should never happen")
            .clone_from(&media_source.to_path_buf());

        self.cache
            .write()
            .expect("This should never happen")
            .clear();

        *self.playlist.write().await = Playlists::new(media_source, temp_directory).await?;

        Ok(())
    }

    pub fn find_cached_segment(
        &self,
        index: usize,
        streams: &StreamIndicies,
    ) -> Option<MediaSegment> {
        self.cache
            .read()
            .expect("This should never happen")
            .iter()
            .find(|segment| segment.index == index && segment.stream_ident == streams.str_repr)
            .cloned()
    }

    pub async fn find_cached_playlist(
        &self,
        db: Connection,
        media_source: &Path,
        streams: &StreamIndicies,
    ) -> AppResult<String> {
        self.playlist
            .write()
            .await
            .get_playlist_for_streams(db, self.session_id, media_source, streams)
            .await
    }

    pub fn extend(&self, segments: Vec<MediaSegment>) {
        let mut cache = self.cache.write().expect("This should never happen");

        let not_cached = segments
            .into_iter()
            .filter(|segment| {
                let already_cached = cache.iter().any(|cached| {
                    cached.index == segment.index && cached.stream_ident == segment.stream_ident
                });
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

    #[inline(always)]
    pub async fn segmentation_for_segment(&self, index: usize) -> Option<Segmentation> {
        self.playlist.read().await.segmentation_for_segment(index)
    }
}
