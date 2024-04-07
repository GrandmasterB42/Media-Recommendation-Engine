use rusqlite::{Connection, OptionalExtension};

use crate::{
    database::{Database, QueryRowGetConnExt, QueryRowIntoConnExt},
    state::{AppError, AppResult},
    utils::{pseudo_random_range, templates::RecommendationPopup, HandleErr},
};

// Probably spawn a recommendation Engine and have a mpsc channel in appstate, to be able to make request to the recommendation engine, which responds with a future. This entire things makes it so there is one global state for the recommendor

impl RecommendationPopup {
    pub async fn new(db: Database, video_id: u64) -> AppResult<Self> {
        let recommendation = tokio::task::spawn_blocking(move || {
            let conn = db.get()?;
            Self::recommend(&conn, video_id)
        });

        let Some(output) = recommendation
            .await
            .log_err_with_msg("failed to resolve tokio thread for recommendation")
            .transpose()?
        else {
            return Err(AppError::Custom(
                "No recommendations could be made".to_string(),
            ));
        };

        Ok(RecommendationPopup {
            id: output.id,
            image: String::new(),
            title: output.title,
        })
    }

    // TODO: This doesn't recognize movies properly
    // This is not the end goal, just something to make it kinda work
    fn recommend(conn: &rusqlite::Connection, video_id: u64) -> AppResult<Recommendation> {
        let maybe_season_episode: Option<(u64, u64)> = conn
            .query_row(
                "SELECT seasonid, episode FROM episodes WHERE videoid=? AND referenceflag=0",
                [video_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        let Some((season_id, episode)) = maybe_season_episode else {
            return Recommendation::random(conn);
        };

        let maybe_next_episode: Option<(u64, Option<String>, u64)> = conn
            .query_row_into(
                "SELECT videoid, title, episode FROM episodes WHERE seasonid=? AND episode=?",
                [season_id, episode + 1],
            )
            .optional()?;

        if let Some((next_episode_id, title, episode)) = maybe_next_episode {
            return Ok(Recommendation {
                id: next_episode_id,
                title: title.unwrap_or(format!("Episode {episode}")),
            });
        }

        let (series_id, season): (u64, u64) = conn.query_row_into(
            "SELECT seriesid, season FROM seasons WHERE id=?",
            [season_id],
        )?;

        let maybe_next_season: Option<u64> = conn
            .query_row_get(
                "SELECT id FROM seasons WHERE seriesid=? AND season=?",
                [series_id, season + 1],
            )
            .optional()?;

        let Some(next_season_id) = maybe_next_season else {
            return Recommendation::random(conn);
        };

        let maybe_first_episode: Option<(u64, Option<String>)> = conn
            .query_row(
                "SELECT videoid, title FROM episodes WHERE seasonid=? AND episode=?",
                [next_season_id, 1],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        if let Some(maybe_first_episode) = maybe_first_episode {
            Ok(Recommendation {
                id: maybe_first_episode.0,
                title: maybe_first_episode.1.unwrap_or("Episode 1".to_owned()),
            })
        } else {
            Recommendation::random(conn)
        }
    }
}

struct Recommendation {
    id: u64,
    title: String,
}

impl Recommendation {
    fn random(conn: &Connection) -> AppResult<Self> {
        // get a random movie or episode
        let maybe_random_episode: Option<(u64, Option<String>, u64)> = conn
            .query_row(
                "SELECT videoid, title, episode FROM episodes WHERE referenceflag=0 ORDER BY RANDOM() LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;

        let maybe_random_movie: Option<(u64, String)> = conn
            .query_row(
                "SELECT videoid, title FROM movies ORDER BY RANDOM() LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        match (maybe_random_episode, maybe_random_movie) {
            (Some((episode_id, title, episode)), None) => Ok(Recommendation {
                id: episode_id,
                title: title.unwrap_or(format!("Episode {episode}")),
            }),
            (None, Some((movie_id, title))) => Ok(Recommendation {
                id: movie_id,
                title,
            }),
            (None, None) => Err(AppError::Custom(
                "No movies or episodes in database".to_string(),
            )),
            (Some((episode_id, episode_title, episode)), Some((movie_id, movie_title))) => {
                let random = pseudo_random_range(0, 2);
                if random == 0 {
                    Ok(Recommendation {
                        id: episode_id,
                        title: episode_title.unwrap_or(format!("Episode {episode}")),
                    })
                } else {
                    Ok(Recommendation {
                        id: movie_id,
                        title: movie_title,
                    })
                }
            }
        }
    }
}
