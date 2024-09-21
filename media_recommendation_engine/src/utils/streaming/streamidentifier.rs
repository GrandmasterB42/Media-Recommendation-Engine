use axum::http::StatusCode;
use std::fmt::Debug;

use crate::state::AppError;

pub enum StreamIndex {
    Video,
    Audio,
    Index(u16),
}

pub struct StreamIndicies {
    pub str_repr: String,
    pub streams: Vec<StreamIndex>,
}

impl StreamIndicies {
    fn new(mut streams: Vec<StreamIndex>) -> Self {
        let mut str_repr = String::new();

        if streams.iter().any(|p| matches!(p, StreamIndex::Video)) {
            str_repr.push_str("v,");
        }

        if streams.iter().any(|p| matches!(p, StreamIndex::Audio)) {
            str_repr.push_str("a,");
        }

        streams.sort_by_key(|index| match index {
            StreamIndex::Video => -1,
            StreamIndex::Audio => -1,
            StreamIndex::Index(i) => *i as i32,
        });

        for index in &streams {
            match index {
                StreamIndex::Video => continue,
                StreamIndex::Audio => continue,
                StreamIndex::Index(i) => str_repr.push_str(&format!("{i},")),
            }
        }

        Self { str_repr, streams }
    }
}

impl TryFrom<&str> for StreamIndicies {
    type Error = AppError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let mut streams = Vec::new();
        for segment in value.split(',') {
            let len = segment.len();
            if len == 0 {
                break;
            } else if segment.len() != 1 {
                return Err(AppError::Status(StatusCode::BAD_REQUEST));
            }

            let index = match segment.chars().next().unwrap() {
                'v' => StreamIndex::Video,
                'a' => StreamIndex::Audio,
                index if index.is_ascii_digit() => StreamIndex::Index(index as u16),
                _ => return Err(AppError::Status(StatusCode::BAD_REQUEST)),
            };
            streams.push(index);
        }

        Ok(Self::new(streams))
    }
}

impl Debug for StreamIndicies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamIndicies")
            .field("streams", &self.str_repr)
            .finish()
    }
}
