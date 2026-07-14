use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::process::Command;

#[cfg(target_os = "linux")]
use anyhow::Context;
use anyhow::{anyhow, Result};

#[derive(Debug, Clone)]
#[cfg_attr(
    not(target_os = "linux"),
    expect(dead_code, reason = "the objdump backend is Linux-only")
)]
pub struct DisassembleRequest {
    pub module_path: PathBuf,
    pub load_bias: i64,
    pub targets: Vec<DisassembleTarget>,
}

#[derive(Debug, Clone)]
pub struct DisassembleTarget {
    /// Raw object symbol passed to objdump. When absent, the address range is used.
    pub raw_symbol: Option<String>,
    /// Stable, demangled owner stored for every emitted instruction.
    pub owner_symbol: String,
    pub start_address: u64,
    pub end_address: u64,
}

#[derive(Debug, Clone)]
pub struct AssemblyLine {
    pub rel_address: u64,
    pub symbol: Option<String>,
    pub instruction: String,
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

    fn run_objdump(
        &self,
        request: &DisassembleRequest,
        target: &DisassembleTarget,
        selected_symbol: Option<&str>,
        demangle: bool,
    ) -> Result<Vec<AssemblyLine>> {
        let mut command = Command::new(&self.program);
        command.arg("-d").arg("--no-show-raw-insn");
        if demangle {
            command.arg("--demangle");
        }
        if let Some(symbol) = selected_symbol {
            command.arg(format!("--disassemble={symbol}"));
        } else {
            command
                .arg(format!("--start-address={}", target.start_address))
                .arg(format!("--stop-address={}", target.end_address));
        }
        let output = command
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

        parse_objdump(
            &String::from_utf8_lossy(&output.stdout),
            request.load_bias,
            Some(&target.owner_symbol),
        )
        .with_context(|| {
            format!(
                "failed to parse disassembly for {}",
                request.module_path.display()
            )
        })
    }

    fn disassemble_target(
        &self,
        request: &DisassembleRequest,
        target: &DisassembleTarget,
    ) -> Result<Vec<AssemblyLine>> {
        let Some(raw_symbol) = target.raw_symbol.as_deref() else {
            return self.run_objdump(request, target, None, true);
        };

        // With --demangle enabled, GNU objdump matches --disassemble against the demangled
        // spelling, not the raw object symbol. Try that spelling first to retain readable call
        // targets, then fall back to the raw spelling without demangling for toolchain-specific
        // formatting differences.
        let lines = self.run_objdump(request, target, Some(&target.owner_symbol), true)?;
        if !lines.is_empty() {
            return Ok(lines);
        }
        let lines = self.run_objdump(request, target, Some(raw_symbol), false)?;
        if !lines.is_empty() {
            return Ok(lines);
        }

        // Versioned and aliased ELF symbols are not always accepted by --disassemble even when
        // they came directly from the object symbol table. Their known bounds are still precise.
        self.run_objdump(request, target, None, true)
    }
}

#[cfg(target_os = "linux")]
impl Disassembler for ObjdumpDisassembler {
    fn disassemble(&self, request: &DisassembleRequest) -> Result<Vec<AssemblyLine>> {
        if request.targets.is_empty() {
            return Ok(Vec::new());
        }
        let worker_count = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .min(request.targets.len());
        let chunk_size = request.targets.len().div_ceil(worker_count);
        let mut chunks = std::thread::scope(|scope| {
            let handles = request
                .targets
                .chunks(chunk_size)
                .enumerate()
                .map(|(chunk_index, targets)| {
                    scope.spawn(move || {
                        let mut lines = Vec::new();
                        for target in targets {
                            lines.extend(self.disassemble_target(request, target)?);
                        }
                        Ok::<_, anyhow::Error>((chunk_index, lines))
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .map_err(|_| anyhow!("disassembler worker panicked"))?
                })
                .collect::<Result<Vec<_>>>()
        })?;
        chunks.sort_unstable_by_key(|(index, _)| *index);
        Ok(chunks.into_iter().flat_map(|(_, lines)| lines).collect())
    }
}

#[cfg(target_os = "linux")]
fn parse_objdump(output: &str, _load_bias: i64, owner: Option<&str>) -> Result<Vec<AssemblyLine>> {
    let mut lines = Vec::new();
    let mut current_symbol: Option<String> = None;

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
                        symbol: owner.map(str::to_owned).or_else(|| current_symbol.clone()),
                        instruction,
                    });
                }
                continue;
            }
        }
    }

    Ok(lines)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use object::{Object, ObjectSymbol, SymbolKind};
    use std::borrow::Cow;

    #[inline(never)]
    fn targeted_disassembly_fixture(value: u64) -> u64 {
        std::hint::black_box(value.wrapping_mul(17).wrapping_add(3))
    }

    #[test]
    fn targeted_rust_symbol_produces_instructions() {
        assert_eq!(targeted_disassembly_fixture(2), 37);
        let module_path = std::env::current_exe().unwrap();
        let bytes = std::fs::read(&module_path).unwrap();
        let object = object::File::parse(bytes.as_slice()).unwrap();
        let symbol = object
            .symbols()
            .find(|symbol| {
                symbol.kind() == SymbolKind::Text
                    && symbol.address() != 0
                    && symbol
                        .name()
                        .is_ok_and(|name| name.contains("targeted_disassembly_fixture"))
            })
            .expect("test fixture symbol must be present");
        let raw_symbol = symbol.name().unwrap().to_string();
        let owner_symbol =
            addr2line::demangle_auto(Cow::Borrowed(raw_symbol.as_str()), None).into_owned();
        let request = DisassembleRequest {
            module_path,
            load_bias: 0,
            targets: vec![DisassembleTarget {
                raw_symbol: Some(raw_symbol),
                owner_symbol: owner_symbol.clone(),
                start_address: symbol.address(),
                end_address: symbol.address().saturating_add(symbol.size()),
            }],
        };

        let lines = ObjdumpDisassembler::new(None)
            .unwrap()
            .disassemble(&request)
            .unwrap();

        assert!(!lines.is_empty());
        assert!(lines
            .iter()
            .all(|line| line.symbol.as_deref() == Some(owner_symbol.as_str())));
        assert!(lines
            .iter()
            .any(|line| line.rel_address == symbol.address()));
    }
}
