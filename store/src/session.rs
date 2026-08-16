use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use duckdb::Connection;

/// Logical table name of a session Parquet file: the file stem with every
/// trailing `-<number>` (pid, segment) component removed.
pub fn table_base_name(file_stem: &str) -> &str {
    let mut base = file_stem;
    while let Some((head, tail)) = base.rsplit_once('-') {
        if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) {
            base = head;
        } else {
            break;
        }
    }
    base
}

/// A session directory opened for querying: an in-memory DuckDB with one view
/// per logical table found in the directory.
pub struct Session {
    connection: Connection,
    tables: Vec<String>,
}

impl Session {
    pub fn open(dir: &Path) -> Result<Session> {
        let connection = Connection::open_in_memory().context("failed to open DuckDB")?;
        let mut tables: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
        for entry in std::fs::read_dir(dir)
            .with_context(|| format!("failed to read session directory {}", dir.display()))?
        {
            let path = entry?.path();
            if path.extension().is_none_or(|ext| ext != "parquet") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            tables
                .entry(table_base_name(stem).to_owned())
                .or_default()
                .push(path);
        }
        for (table, mut files) in tables {
            files.sort();
            let list = files
                .iter()
                .map(|file| format!("'{}'", file.display().to_string().replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(", ");
            connection
                .execute_batch(&format!(
                    "CREATE VIEW \"{table}\" AS SELECT * FROM read_parquet([{list}])"
                ))
                .with_context(|| format!("failed to create view over table '{table}'"))?;
        }
        let tables = {
            let mut statement = connection.prepare(
                "SELECT view_name FROM duckdb_views() WHERE NOT internal ORDER BY view_name",
            )?;
            let names = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<duckdb::Result<Vec<_>>>()?;
            names
        };
        Ok(Session { connection, tables })
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Logical tables available in this session.
    pub fn tables(&self) -> &[String] {
        &self.tables
    }

    pub fn has_table(&self, name: &str) -> bool {
        self.tables.iter().any(|table| table == name)
    }
}
