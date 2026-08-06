//! Dynamic CFG accounting shared by the instrumentation backends.

use crate::classify::{BlockCost, FlowKind};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DynamicBlockCounts {
    pub end_vaddr: u64,
    pub executions: u64,
    pub scalar_int: u64,
    pub scalar_float: u64,
    pub scalar_double: u64,
    pub vector_int: u64,
    pub vector_float: u64,
    pub vector_double: u64,
    pub bytes_load: u64,
    pub bytes_store: u64,
    pub unclassified: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VectorClass {
    Integer,
    Float,
    Double,
}

pub struct DynamicCfg {
    last_blocks: Vec<Option<(u64, FlowKind)>>,
    call_stacks: Vec<Vec<u64>>,
    pub entries: BTreeSet<u64>,
    pub edges: BTreeMap<(u64, u64), u64>,
    pub blocks: BTreeMap<u64, DynamicBlockCounts>,
}

impl DynamicCfg {
    pub const fn new() -> Self {
        Self {
            last_blocks: Vec::new(),
            call_stacks: Vec::new(),
            entries: BTreeSet::new(),
            edges: BTreeMap::new(),
            blocks: BTreeMap::new(),
        }
    }

    /// Records one execution of a block: control-flow edge bookkeeping plus
    /// accumulation of the block's static cost.
    pub fn record_block(&mut self, vcpu: usize, cost: &BlockCost) {
        if self.last_blocks.len() <= vcpu {
            self.last_blocks.resize(vcpu + 1, None);
        }
        if self.call_stacks.len() <= vcpu {
            self.call_stacks.resize_with(vcpu + 1, Vec::new);
        }
        if let Some((previous, flow)) = self.last_blocks[vcpu] {
            match flow {
                FlowKind::Normal => {
                    *self.edges.entry((previous, cost.vaddr)).or_default() += 1;
                }
                FlowKind::Call => {
                    self.entries.insert(cost.vaddr);
                    self.call_stacks[vcpu].push(previous);
                }
                FlowKind::Return => {
                    if let Some(caller) = self.call_stacks[vcpu].pop() {
                        // Summarize the call as an intraprocedural edge from the
                        // call block to its observed continuation. This preserves
                        // caller loops without adding call/return edges that can
                        // create false interprocedural cycles.
                        *self.edges.entry((caller, cost.vaddr)).or_default() += 1;
                    } else {
                        self.entries.insert(cost.vaddr);
                    }
                }
            }
        } else {
            self.entries.insert(cost.vaddr);
        }
        self.last_blocks[vcpu] = Some((cost.vaddr, cost.flow));
        let block = self.blocks.entry(cost.vaddr).or_default();
        if block.end_vaddr == 0 {
            block.end_vaddr = cost.end_vaddr;
        } else if block.end_vaddr != cost.end_vaddr {
            // Self-modifying code or context-dependent translation at the same
            // virtual address cannot be represented as one stable source range.
            block.end_vaddr = block.end_vaddr.max(cost.end_vaddr);
        }
        block.executions = block.executions.saturating_add(1);
        block.scalar_int = block.scalar_int.saturating_add(cost.scalar_int);
        block.scalar_float = block.scalar_float.saturating_add(cost.scalar_float);
        block.scalar_double = block.scalar_double.saturating_add(cost.scalar_double);
        block.vector_int = block.vector_int.saturating_add(cost.vector_int);
        block.vector_float = block.vector_float.saturating_add(cost.vector_float);
        block.vector_double = block.vector_double.saturating_add(cost.vector_double);
    }

    pub fn attribute_unclassified(&mut self, block_address: u64) {
        let block = self.blocks.entry(block_address).or_default();
        block.unclassified = block.unclassified.saturating_add(1);
    }

    /// Attributes modeled DRAM traffic to the block that issued the access.
    pub fn attribute_memory(&mut self, block_address: u64, bytes_load: u64, bytes_store: u64) {
        let block = self.blocks.entry(block_address).or_default();
        block.bytes_load = block.bytes_load.saturating_add(bytes_load);
        block.bytes_store = block.bytes_store.saturating_add(bytes_store);
    }

    /// Attributes dynamically-counted vector operations (RVV) to a block.
    pub fn attribute_vector(&mut self, block_address: u64, class: VectorClass, operations: u64) {
        let block = self.blocks.entry(block_address).or_default();
        match class {
            VectorClass::Integer => {
                block.vector_int = block.vector_int.saturating_add(operations);
            }
            VectorClass::Float => {
                block.vector_float = block.vector_float.saturating_add(operations);
            }
            VectorClass::Double => {
                block.vector_double = block.vector_double.saturating_add(operations);
            }
        }
    }
}

impl Default for DynamicCfg {
    fn default() -> Self {
        Self::new()
    }
}
