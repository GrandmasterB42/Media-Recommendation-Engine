use std::{path::PathBuf, time::Duration};

use tracing::debug;

use crate::{
    database::{Database, DatabaseResult},
    utils::HandleErr,
};

pub async fn periodic_indexing(db: Database) -> ! {
    loop {
        // TODO: Handle this error better?
        indexing(&db).log_err_with_msg("Failed the indexing");
        // TODO: Setting for this duration?
        tokio::time::sleep(Duration::from_secs(60 * 5)).await;
    }
}

fn indexing(db: &Database) -> DatabaseResult<()> {
    let locations: Vec<String> = db.run(|conn| {
        let mut stmt = conn.prepare("SELECT path FROM storage_locations")?;
        let paths = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(paths)
    })?;

    let (directories, files): (Vec<_>, Vec<_>) = locations
        .into_iter()
        .map(PathBuf::from)
        .partition(|path| path.is_dir());

    let filesystem = files
        .into_iter()
        .chain(directories.into_iter().flat_map(scan_dir))
        .collect::<Vec<_>>();

    let database_registered = db.run(|conn| {
        let mut stmt = conn.prepare("SELECT path from data_files")?;

        let mut out: Vec<PathBuf> = Vec::new();
        if let Ok(mut rows) = stmt.query([]) {
            while let Some(row) = rows.next()? {
                out.push(row.get::<usize, String>(0)?.into());
            }
        }
        Ok(out)
    })?;

    // Note: There is probably a faster? way to do these two loops, but this is good enough for now
    // TODO: Also track changes and handle that
    let mut conn = db.get()?;
    let tx = conn.transaction()?;

    for file in &filesystem {
        if !database_registered.contains(file) {
            debug!("Want to add {file:?}");
            let str_path = file.display().to_string(); // Note: This conversion doesn't seem quite right, but it works i guess
            tx.execute("INSERT INTO data_files (path) VALUES (?1) ", [str_path])?;
        }
    }

    tx.commit()?;

    // TODO: Removals will probably have to do more than just remove the data_file
    for file in &database_registered {
        if !filesystem.contains(file) {
            debug!("Want to remove {file:?}")
        }
    }

    // For added files extract as much information as possible from the file path, assign to series/franchise if similar files already have an assignment

    Ok(())
}

// TODO: Make recursive a setting maybe?
/// Requires that the given PathBuf points to a directory
fn scan_dir(path: PathBuf) -> Vec<PathBuf> {
    path.read_dir()
        .log_err_with_msg("Failed to read directory")
        .map_or(Vec::new(), |read_dir| {
            let mut out = Vec::new();

            for entry in read_dir {
                if let Some(entry) =
                    entry.log_err_with_msg("Encountered IO Error while scanning directory")
                {
                    let path = entry.path();
                    if path.is_dir() {
                        out.extend(scan_dir(path));
                    } else {
                        out.push(path);
                    }
                }
            }
            out
        })
}
