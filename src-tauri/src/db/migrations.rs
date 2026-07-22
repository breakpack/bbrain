use rusqlite::Connection;

use crate::error::{AppError, Result};

pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
}

/// Migrations are append-only and applied in ascending version order. Released
/// versions are never edited — a schema change is always a new file.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial",
        sql: include_str!("../../migrations/0001_initial.sql"),
    },
    Migration {
        version: 2,
        name: "vector_index",
        sql: include_str!("../../migrations/0002_vector_index.sql"),
    },
    Migration {
        version: 3,
        name: "sentence_paragraph",
        sql: include_str!("../../migrations/0003_sentence_paragraph.sql"),
    },
    Migration {
        version: 4,
        name: "topics",
        sql: include_str!("../../migrations/0004_topics.sql"),
    },
    Migration {
        version: 5,
        name: "deepseek_translation_engine",
        sql: include_str!("../../migrations/0005_deepseek_translation_engine.sql"),
    },
];

pub fn current_version(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}

/// Applies every pending migration inside one transaction each, so an
/// interrupted upgrade leaves the database at the last fully applied version.
pub fn migrate(conn: &mut Connection) -> Result<i64> {
    let mut version = current_version(conn)?;

    for migration in MIGRATIONS {
        if migration.version <= version {
            continue;
        }
        if migration.version != version + 1 {
            return Err(AppError::Migration(format!(
                "migration {} is out of order after version {version}",
                migration.version
            )));
        }

        let tx = conn.transaction()?;
        tx.execute_batch(migration.sql)?;
        // PRAGMA does not accept bound parameters.
        tx.pragma_update(None, "user_version", migration.version)?;
        tx.commit()?;

        tracing::info!(
            version = migration.version,
            name = migration.name,
            "applied migration"
        );
        version = migration.version;
    }

    Ok(version)
}

pub fn latest_version() -> i64 {
    MIGRATIONS.last().map(|m| m.version).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory() -> Connection {
        crate::db::register_sqlite_vec();
        let conn = Connection::open_in_memory().unwrap();
        crate::db::apply_pragmas(&conn).unwrap();
        conn
    }

    #[test]
    fn migrates_a_fresh_database_to_the_latest_version() {
        let mut conn = memory();
        let version = migrate(&mut conn).unwrap();

        assert_eq!(version, latest_version());
        assert_eq!(current_version(&conn).unwrap(), latest_version());
    }

    #[test]
    fn migrating_twice_is_a_no_op() {
        let mut conn = memory();
        migrate(&mut conn).unwrap();
        let version = migrate(&mut conn).unwrap();

        assert_eq!(version, latest_version());
    }

    #[test]
    fn creates_every_declared_entity_table() {
        let mut conn = memory();
        migrate(&mut conn).unwrap();

        for table in [
            "papers",
            "paper_metadata",
            "groups",
            "paper_groups",
            "tags",
            "paper_tags",
            "pages",
            "sentences",
            "highlights",
            "analyses",
            "translations",
            "chunks",
            "chunk_vectors",
            "relations",
            "topics",
            "paper_topics",
            "topic_edges",
            "topic_build",
            "chat_sessions",
            "chat_messages",
            "chat_citations",
            "jobs",
            "sync_records",
            "settings",
            "papers_fts",
            "chunks_fts",
        ] {
            let found: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(found, 1, "missing table {table}");
        }
    }

    #[test]
    fn foreign_keys_cascade_from_papers() {
        let mut conn = memory();
        migrate(&mut conn).unwrap();

        conn.execute_batch(
            "INSERT INTO papers (id, sha256, managed_path, title, import_status, created_at, updated_at)
             VALUES ('p1', 'hash', '/tmp/p1.pdf', 'Paper', 'ready', '2026-07-14T00:00:00Z', '2026-07-14T00:00:00Z');
             INSERT INTO pages (paper_id, page_number, width, height, text, text_hash)
             VALUES ('p1', 1, 612.0, 792.0, 'text', 'h');
             DELETE FROM papers WHERE id = 'p1';",
        )
        .unwrap();

        let pages: i64 = conn
            .query_row("SELECT count(*) FROM pages", [], |row| row.get(0))
            .unwrap();
        assert_eq!(pages, 0);
    }
}
