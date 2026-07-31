use std::{fs, path::Path};

#[cfg(test)]
use std::collections::HashSet;

use flamelens::flame::{FlameGraph, StackIdentifier, ROOT_ID};

#[derive(Debug)]
pub struct FlamegraphData {
    pub cycles: Option<FlameGraph>,
    pub instructions: Option<FlameGraph>,
    pub error: Option<String>,
}

#[derive(Debug)]
pub struct FlameFrame {
    pub id: StackIdentifier,
    pub name: String,
    pub value: u64,
    pub self_value: u64,
    pub x: f32,
    pub width: f32,
    pub depth: usize,
}

#[derive(Debug)]
pub struct FlameLayout {
    pub frames: Vec<FlameFrame>,
    pub max_depth: usize,
    pub total: u64,
    pub root_name: String,
}

impl FlamegraphData {
    pub fn load(result_directory: &Path) -> Self {
        let cycles = read(result_directory.join("flamegraph_cycles.folded"));
        let instructions = read(result_directory.join("flamegraph_instructions.folded"));
        let error = match (&cycles, &instructions) {
            (Err(cycles), Err(instructions)) => Some(format!(
                "Could not load cycle or instruction flamegraphs.\nCycles: {cycles}\nInstructions: {instructions}"
            )),
            (Err(error), _) => Some(format!("Could not load cycle flamegraph: {error}")),
            (_, Err(error)) => Some(format!("Could not load instruction flamegraph: {error}")),
            _ => None,
        };

        Self {
            cycles: cycles.ok(),
            instructions: instructions.ok(),
            error,
        }
    }

    pub fn layout(&self, instructions: bool, zoom: StackIdentifier) -> Option<FlameLayout> {
        let graph = self.graph(instructions)?;
        let zoom = graph.get_stack(&zoom).map(|_| zoom).unwrap_or(ROOT_ID);
        let root = graph.get_stack(&zoom)?;
        let total = root.total_count;
        if total == 0 {
            return None;
        }

        let mut frames = Vec::new();
        let mut max_depth = 0;
        flatten(graph, zoom, 0.0, 1.0, 0, &mut max_depth, &mut frames);
        Some(FlameLayout {
            frames,
            max_depth,
            total,
            root_name: graph
                .get_stack_short_name(&zoom)
                .unwrap_or("all")
                .to_string(),
        })
    }

    pub fn hottest_stack_named(
        &self,
        instructions: bool,
        function: &str,
    ) -> Option<StackIdentifier> {
        let graph = self.graph(instructions)?;
        (0..graph.get_num_levels())
            .filter_map(|level| graph.get_stacks_at_level(level))
            .flatten()
            .filter_map(|id| graph.get_stack(id))
            .filter(|stack| names_match(graph.get_stack_short_name_from_info(stack), function))
            .max_by_key(|stack| stack.total_count)
            .map(|stack| stack.id)
    }

    #[cfg(test)]
    pub fn descendant_function_names(
        &self,
        instructions: bool,
        stack_id: StackIdentifier,
    ) -> HashSet<String> {
        let Some(graph) = self.graph(instructions) else {
            return HashSet::new();
        };
        graph
            .get_descendants(&stack_id)
            .into_iter()
            .filter(|id| *id != ROOT_ID)
            .filter_map(|id| graph.get_stack_short_name(&id))
            .map(|name| name.replace("[unknown]", "Unknown"))
            .collect()
    }

    fn graph(&self, instructions: bool) -> Option<&FlameGraph> {
        if instructions {
            self.instructions.as_ref()
        } else {
            self.cycles.as_ref()
        }
    }
}

fn read(path: impl AsRef<Path>) -> Result<FlameGraph, String> {
    let path = path.as_ref();
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    validate(&contents).map_err(|error| format!("invalid {}: {error}", path.display()))?;
    Ok(FlameGraph::from_string(contents, false))
}

fn validate(contents: &str) -> Result<(), String> {
    let mut sample_count = 0;
    for (index, line) in contents.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let valid = line
            .rsplit_once(' ')
            .is_some_and(|(stack, count)| !stack.is_empty() && count.parse::<u64>().is_ok());
        if !valid {
            return Err(format!("malformed folded stack on line {}", index + 1));
        }
        sample_count += 1;
    }
    if sample_count == 0 {
        return Err("no valid folded stack samples found".to_string());
    }
    Ok(())
}

fn flatten(
    graph: &FlameGraph,
    stack_id: StackIdentifier,
    x: f32,
    width: f32,
    depth: usize,
    max_depth: &mut usize,
    frames: &mut Vec<FlameFrame>,
) {
    let Some(stack) = graph.get_stack(&stack_id) else {
        return;
    };
    *max_depth = (*max_depth).max(depth);
    frames.push(FlameFrame {
        id: stack.id,
        name: graph
            .get_stack_short_name_from_info(stack)
            .replace("[unknown]", "Unknown"),
        value: stack.total_count,
        self_value: stack.self_count,
        x,
        width,
        depth,
    });

    let mut child_x = x;
    for child_id in &stack.children {
        let Some(child) = graph.get_stack(child_id) else {
            continue;
        };
        let child_width = width * child.total_count as f32 / stack.total_count as f32;
        flatten(
            graph,
            *child_id,
            child_x,
            child_width,
            depth + 1,
            max_depth,
            frames,
        );
        child_x += child_width;
    }
}

fn names_match(stack_name: &str, function: &str) -> bool {
    stack_name == function
        || stack_name.strip_prefix('_') == Some(function)
        || function.strip_prefix('_') == Some(stack_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_proportional_layout_from_folded_stacks() {
        let graph = FlameGraph::from_string("main;hot 75\nmain;cold 25\n".to_string(), false);
        let data = FlamegraphData {
            cycles: Some(graph),
            instructions: None,
            error: None,
        };

        let layout = data.layout(false, ROOT_ID).unwrap();
        assert_eq!(layout.total, 100);
        assert_eq!(layout.max_depth, 2);
        let hot = layout
            .frames
            .iter()
            .find(|frame| frame.name == "hot")
            .unwrap();
        assert!((hot.width - 0.75).abs() < f32::EPSILON);
        assert_eq!(data.hottest_stack_named(false, "hot"), Some(hot.id));
        assert_eq!(
            data.descendant_function_names(false, ROOT_ID),
            HashSet::from(["main".to_string(), "hot".to_string(), "cold".to_string()])
        );
    }

    #[test]
    fn rejects_corrupt_folded_stacks() {
        assert!(validate("main not-a-number\n").is_err());
        assert!(validate("# no samples\n").is_err());
    }
}
