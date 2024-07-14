use rusqlite::{params, OptionalExtension};

use crate::{
    database::{Connection, Database, QueryRowGetConnExt, QueryRowIntoConnExt},
    indexing::{CollectionType, ContentType, TableId},
    state::AppResult,
    utils::{pseudo_random_range, templates::RecommendationPopup, HandleErr},
};

// Probably spawn a recommendation Engine and have a mpsc channel in appstate, to be able to make request to the recommendation engine, which responds with a future. This entire things makes it so there is one global state for the recommendor

impl RecommendationPopup {
    pub async fn new(db: Database, content_id: u64) -> AppResult<Self> {
        let recommendation = tokio::task::spawn_blocking(move || {
            let conn = db.get()?;
            Self::recommend(&conn, content_id)
        });

        let Some(output) = recommendation
            .await
            .log_err_with_msg("failed to resolve tokio thread for recommendation")
            .transpose()?
        else {
            bail!("No recommendations could be made");
        };

        Ok(RecommendationPopup {
            id: output.id,
            image: String::new(),
            title: output.title,
        })
    }

    // TODO: This doesn't recognize movies properly
    // This is not the end goal, just something to make it kinda work
    fn recommend(conn: &Connection, content_id: u64) -> AppResult<Recommendation> {
        let this_episode: Option<u64> = conn
            .query_row_get(
                "SELECT episode.episode FROM content, episode
                    WHERE content.type = ?1
                    AND content.reference = episode.id
                    AND content.id = ?2",
                params![ContentType::Episode, content_id],
            )
            .optional()?;

        let maybe_season_id: Option<(u64, u64, String)> = conn
            .query_row_into(
                "SELECT collection.id, season.season, season.title FROM collection_contains, collection, season
                WHERE collection_contains.collection_id = collection.id
                AND collection_contains.type = ?1
                AND collection_contains.reference = ?2
                AND collection.type = ?3
                AND collection.reference = season.id",
                params![TableId::Content, content_id, CollectionType::Season],
            )
            .optional()?;

        let (Some((season_id, season, season_title)), Some(episode)) =
            (maybe_season_id, this_episode)
        else {
            return Recommendation::random(conn);
        };

        let maybe_next_episode: Option<(u64, String, u64)> = conn
            .query_row_into(
                "SELECT content.id, episode.title, episode.episode FROM collection_contains, episode, content
                    WHERE collection_contains.collection_id = ?1
                    AND collection_contains.type = ?2
                    AND collection_contains.reference = content.id
                    AND content.type = ?3
                    AND content.reference = episode.id
                    AND episode.episode = ?4",
                params![season_id, TableId::Content, ContentType::Episode, episode + 1],
            )
            .optional()?;

        if let Some((next_episode_id, title, episode)) = maybe_next_episode {
            return Ok(Recommendation {
                id: next_episode_id,
                title: format!("{title} - {season_title} - Season {season} - Episode {episode}"),
            });
        }

        let maybe_series_id: Option<u64> = conn.query_row_get(
            "SELECT collection.id FROM collection, collection_contains
                WHERE collection.id = collection_contains.collection_id
                AND collection_contains.type = ?1
                AND collection_contains.reference = ?2
                AND collection.type = ?3",
            params![TableId::Collection, season_id, CollectionType::Series],
        )?;

        let Some(series_id) = maybe_series_id else {
            return Recommendation::random(conn);
        };

        let maybe_next_season: Option<u64> = conn
            .query_row_get(
                "SELECT collection.id FROM collection, collection_contains, season
                    WHERE collection.id = collection_contains.collection_id
                    AND collection_contains.type = ?1
                    AND collection_contains.reference = season.id
                    AND collection.type = ?2
                    AND season.season = ?3
                    AND collection.reference = season.id
                    AND collection_contains.collection_id = ?4",
                params![
                    TableId::Collection,
                    CollectionType::Season,
                    season + 1,
                    series_id
                ],
            )
            .optional()?;

        let Some(next_season_id) = maybe_next_season else {
            return Recommendation::random(conn);
        };

        let maybe_first_episode: Option<(u64, String)> = conn
            .query_row_into(
                "SELECT content.id, episode.title FROM collection_contains, episode, content
                    WHERE collection_contains.collection_id = ?1
                    AND collection_contains.type = ?2
                    AND collection_contains.reference = content.id
                    AND content.type = ?3
                    AND content.reference = episode.id
                    AND episode.episode = 1",
                params![next_season_id, TableId::Content, ContentType::Episode],
            )
            .optional()?;

        if let Some((id, title)) = maybe_first_episode {
            Ok(Recommendation {
                id,
                title: format!("{title} - Episode 1"),
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
        let maybe_random_episode: Option<(u64, String, u64)> = conn
            .query_row_into(
                "SELECT episode.id, episode.title, episode.episode FROM episode, content 
                WHERE episode.id = content.reference
                AND content.type = ?1
                ORDER BY RANDOM() LIMIT 1",
                [ContentType::Episode],
            )
            .optional()?;

        let maybe_random_movie: Option<(u64, String)> = conn
            .query_row_into(
                "SELECT movie.id, movie.title FROM movie, content 
                WHERE movie.id = content.reference
                AND content.type = ?1
                ORDER BY RANDOM() LIMIT 1",
                [ContentType::Movie],
            )
            .optional()?;

        match (maybe_random_episode, maybe_random_movie) {
            (Some((id, title, episode)), None) => Ok(Recommendation {
                id,
                title: format!("{title} - Episode {episode}"),
            }),
            (None, Some((id, title))) => Ok(Recommendation { id, title }),
            (None, None) => bail!("No movies or episodes in database"),
            (Some((episode_id, episode_title, episode)), Some((movie_id, movie_title))) => {
                let random = pseudo_random_range(0, 2);
                if random == 0 {
                    Ok(Recommendation {
                        id: episode_id,
                        title: format!("{episode_title} - Episode {episode}"),
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
