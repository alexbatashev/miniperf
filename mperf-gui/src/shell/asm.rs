//! Lazy per-function disassembly. The session drops its sqlite connection
//! after loading, so each listing re-opens `perf.db` on the background
//! executor and joins `assembly_lines` against `assembly_samples`.

use std::{collections::BTreeMap, path::Path};

use anyhow::{Context, Result};
use sqlite::Connection;

#[derive(Clone, Debug)]
pub struct AsmInstruction {
    pub address: u64,
    pub text: String,
    pub line: Option<usize>,
    pub samples: u64,
    pub llc_misses: u64,
}

/// One function's instructions plus the per-source-line heat derived from them.
#[derive(Clone, Debug)]
pub struct AsmListing {
    pub instructions: Vec<AsmInstruction>,
    pub source_file: Option<String>,
    pub line_samples: BTreeMap<usize, u64>,
    pub total_samples: u64,
    pub max_line_samples: u64,
    pub max_instruction_samples: u64,
    pub total_llc_misses: u64,
}

/// Whether this recording carries disassembly at all.
pub fn has_assembly(connection: &Connection) -> bool {
    connection
        .prepare("SELECT 1 FROM assembly_lines LIMIT 1;")
        .and_then(|mut statement| statement.next().map(|state| state == sqlite::State::Row))
        .unwrap_or(false)
}

/// Loads the listing for `function` in `module`. Symbols are resolved through
/// the sampled addresses first, because the disassembler's symbol names and
/// the profile's `func_name` do not always agree.
pub fn load(result_directory: &Path, module: &str, function: &str) -> Result<AsmListing> {
    let database_path = result_directory.join("perf.db");
    let connection = sqlite::open(&database_path)
        .with_context(|| format!("failed to open {}", database_path.display()))?;

    let mut symbols = query_symbols(&connection, module, function)?;
    if symbols.is_empty() {
        symbols.push(function.to_owned());
    }

    let mut instructions = Vec::new();
    let mut source_files = BTreeMap::<String, usize>::new();
    for symbol in &symbols {
        let mut statement = connection.prepare(
            "SELECT l.runtime_address, l.instruction, l.source_file, l.source_line,
                    COALESCE(s.samples, 0), COALESCE(s.llc_misses, 0)
             FROM assembly_lines l
             LEFT JOIN assembly_samples s
               ON s.module_path = l.module_path AND s.address = l.runtime_address
             WHERE l.module_path = ? AND l.symbol = ?
             ORDER BY l.runtime_address;",
        )?;
        statement.bind((1, module))?;
        statement.bind((2, symbol.as_str()))?;
        for row in statement.into_iter() {
            let row = row?;
            if let Some(file) = row.read::<Option<&str>, _>(2) {
                *source_files.entry(file.to_owned()).or_default() += 1;
            }
            instructions.push(AsmInstruction {
                address: row.read::<i64, _>(0).max(0) as u64,
                text: row.read::<&str, _>(1).to_owned(),
                line: row
                    .read::<Option<i64>, _>(3)
                    .filter(|line| *line > 0)
                    .map(|line| line as usize),
                samples: row.read::<i64, _>(4).max(0) as u64,
                llc_misses: row.read::<i64, _>(5).max(0) as u64,
            });
        }
    }
    instructions.sort_by_key(|instruction| instruction.address);

    let source_file = source_files
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(file, _)| file);

    let mut line_samples = BTreeMap::<usize, u64>::new();
    let mut total_samples = 0u64;
    let mut total_llc_misses = 0u64;
    for instruction in &instructions {
        total_samples = total_samples.saturating_add(instruction.samples);
        total_llc_misses = total_llc_misses.saturating_add(instruction.llc_misses);
        if let Some(line) = instruction.line {
            *line_samples.entry(line).or_default() += instruction.samples;
        }
    }

    Ok(AsmListing {
        max_line_samples: line_samples.values().copied().max().unwrap_or(0),
        max_instruction_samples: instructions
            .iter()
            .map(|instruction| instruction.samples)
            .max()
            .unwrap_or(0),
        instructions,
        source_file,
        line_samples,
        total_samples,
        total_llc_misses,
    })
}

fn query_symbols(connection: &Connection, module: &str, function: &str) -> Result<Vec<String>> {
    let mut statement = connection.prepare(
        "SELECT DISTINCT l.symbol
         FROM assembly_lines l
         JOIN assembly_samples s
           ON s.module_path = l.module_path AND s.address = l.runtime_address
         WHERE l.module_path = ? AND s.func_name = ? AND l.symbol IS NOT NULL;",
    )?;
    statement.bind((1, module))?;
    statement.bind((2, function))?;
    statement
        .into_iter()
        .map(|row| Ok(row?.read::<&str, _>(0).to_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Connection {
        let connection = sqlite::open(":memory:").unwrap();
        connection
            .execute(
                "CREATE TABLE assembly_lines (
                    module_path TEXT, symbol TEXT, rel_address INTEGER,
                    runtime_address INTEGER, instruction TEXT,
                    source_file TEXT, source_line INTEGER
                 );
                 CREATE TABLE assembly_samples (
                    module_path TEXT, func_name TEXT, address INTEGER,
                    samples INTEGER, cycles INTEGER, instructions INTEGER,
                    branch_misses INTEGER, branch_instructions INTEGER,
                    llc_misses INTEGER, llc_references INTEGER
                 );
                 INSERT INTO assembly_lines VALUES
                    ('/bin/app', '_Z7computev', 16, 4096, 'mov %rax,%rbx', '/src/a.c', 10),
                    ('/bin/app', '_Z7computev', 20, 4100, 'add %rax,%rbx', '/src/a.c', 11),
                    ('/bin/app', 'other', 40, 8192, 'ret', '/src/b.c', 1);
                 INSERT INTO assembly_samples VALUES
                    ('/bin/app', 'compute', 4096, 7, 0, 0, 0, 0, 3, 0),
                    ('/bin/app', 'compute', 4100, 2, 0, 0, 0, 0, 1, 0);",
            )
            .unwrap();
        connection
    }

    #[test]
    fn resolves_the_symbol_through_sampled_addresses() {
        let connection = fixture();
        assert!(has_assembly(&connection));
        assert_eq!(
            query_symbols(&connection, "/bin/app", "compute").unwrap(),
            vec!["_Z7computev".to_owned()]
        );
    }

    #[test]
    fn unsampled_functions_resolve_to_no_symbol() {
        let connection = fixture();
        assert!(
            query_symbols(&connection, "/bin/app", "missing")
                .unwrap()
                .is_empty()
        );
    }
}
