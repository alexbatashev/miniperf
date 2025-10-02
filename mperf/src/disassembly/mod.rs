use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, Context, Result};

#[derive(Debug, Clone)]
pub struct DisassembleRequest {
    pub module_path: PathBuf,
    pub load_bias: i64,
}

#[derive(Debug, Clone)]
pub struct AssemblyLine {
    pub rel_address: u64,
    pub symbol: Option<String>,
    pub instruction: String,
    pub source_file: Option<String>,
    pub source_line: Option<u32>,
}

pub trait Disassembler: Send + Sync {
    fn disassemble(&self, request: &DisassembleRequest) -> Result<Vec<AssemblyLine>>;
}

pub fn default_disassembler() -> Result<Box<dyn Disassembler>> {
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(ObjdumpDisassembler::new(None)?))
    }

    #[cfg(not(target_os = "linux"))]
    {
        Err(anyhow!(
            "no default disassembler is available for this platform"
        ))
    }
}

#[cfg(target_os = "linux")]
struct ObjdumpDisassembler {
    program: PathBuf,
}

#[cfg(target_os = "linux")]
impl ObjdumpDisassembler {
    fn new(program: Option<PathBuf>) -> Result<Self> {
        let program = program.unwrap_or_else(|| PathBuf::from("objdump"));
        if which::which(&program).is_err() {
            return Err(anyhow!(
                "failed to locate '{}' required for disassembly",
                program.display()
            ));
        }
        Ok(ObjdumpDisassembler { program })
    }
}

#[cfg(target_os = "linux")]
impl Disassembler for ObjdumpDisassembler {
    fn disassemble(&self, request: &DisassembleRequest) -> Result<Vec<AssemblyLine>> {
        let output = Command::new(&self.program)
            .arg("-d")
            .arg("--no-show-raw-insn")
            .arg("--line-numbers")
            .arg("--demangle")
            .arg(&request.module_path)
            .output()
            .with_context(|| {
                format!(
                    "failed to run {} on {}",
                    self.program.display(),
                    request.module_path.display()
                )
            })?;

        if !output.status.success() {
            return Err(anyhow!(
                "disassembler returned non-zero exit status for {}",
                request.module_path.display()
            ));
        }

        parse_objdump(&String::from_utf8_lossy(&output.stdout), request.load_bias).with_context(
            || {
                format!(
                    "failed to parse disassembly for {}",
                    request.module_path.display()
                )
            },
        )
    }
}

#[cfg(target_os = "linux")]
fn parse_objdump(output: &str, _load_bias: i64) -> Result<Vec<AssemblyLine>> {
    let mut lines = Vec::new();
    let mut current_symbol: Option<String> = None;
    let mut current_source: Option<String> = None;
    let mut current_line: Option<u32> = None;

    for raw_line in output.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(idx) = trimmed.find(" <") {
            if trimmed.ends_with(":") {
                let symbol = trimmed[idx + 2..trimmed.len() - 2].trim();
                if !symbol.is_empty() {
                    current_symbol = Some(symbol.to_string());
                }
                continue;
            }
        }

        if let Some(pos) = trimmed.rfind(':') {
            let (left, right) = trimmed.split_at(pos);
            let number_part = &right[1..];
            if !left.chars().all(|c| c.is_ascii_hexdigit())
                && number_part.chars().all(|c| c.is_ascii_digit())
            {
                current_source = Some(left.trim().to_string());
                current_line = number_part.parse::<u32>().ok();
                continue;
            }
        }

        let mut parts = trimmed.splitn(2, ':');
        if let (Some(addr_part), Some(rest)) = (parts.next(), parts.next()) {
            if addr_part.chars().all(|c| c.is_ascii_hexdigit()) {
                if let Ok(rel_addr) = u64::from_str_radix(addr_part, 16) {
                    let instruction = rest.trim().to_string();
                    if instruction.is_empty() {
                        continue;
                    }
                    lines.push(AssemblyLine {
                        rel_address: rel_addr,
                        symbol: current_symbol.clone(),
                        instruction,
                        source_file: current_source.clone(),
                        source_line: current_line,
                    });
                }
                continue;
            }
        }
    }

    Ok(lines)
}
