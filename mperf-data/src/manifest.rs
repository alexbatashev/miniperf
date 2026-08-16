use std::path::Path;

use serde::{Deserialize, Serialize};

/// Declarative visualization manifest attached in the viewer — never at
/// record time. A closed vocabulary the GUI contract honors: unknown keys
/// are ignored, so growing it is non-breaking. Absence never hides data:
/// without a manifest custom events render as a table grouped by function
/// and spans appear as generic tracks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VisualizationManifest {
    /// Manifest schema version; currently 1.
    #[serde(default = "default_manifest_version")]
    pub version: u32,
    /// Timeline row assignment: which domain > track > lane an event kind
    /// renders on and how rows nest.
    #[serde(default)]
    pub tracks: Vec<TrackSpec>,
    /// Instant-event glyphs and severity classes.
    #[serde(default)]
    pub markers: Vec<MarkerSpec>,
    /// Counter presentation.
    #[serde(default)]
    pub counters: Vec<CounterSpec>,
    /// Named pages composed from selected tracks and counters, rendered
    /// first-class by the GUI.
    #[serde(default)]
    pub views: Vec<ViewSpec>,
}

fn default_manifest_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackSpec {
    /// Trace-point name the row matches (the payload's `name`).
    pub event: String,
    #[serde(default)]
    pub domain: String,
    pub track: String,
    #[serde(default)]
    pub lane: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    #[default]
    Info,
    Warn,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkerSpec {
    pub event: String,
    #[serde(default)]
    pub glyph: String,
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub severity: Severity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CounterRender {
    #[default]
    Line,
    Area,
    Stat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CounterSpec {
    pub event: String,
    #[serde(default)]
    pub unit: String,
    /// Multiplier applied to raw values before display; 0 means 1.
    #[serde(default)]
    pub scale: f64,
    #[serde(default)]
    pub render: CounterRender,
    #[serde(default)]
    pub warn: Option<f64>,
    #[serde(default)]
    pub critical: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewSpec {
    pub name: String,
    #[serde(default)]
    pub tracks: Vec<String>,
    #[serde(default)]
    pub counters: Vec<String>,
}

impl VisualizationManifest {
    /// Load a manifest from a YAML (default) or JSON file.
    pub fn load(path: &Path) -> Result<VisualizationManifest, String> {
        let data = std::fs::read_to_string(path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        let manifest = if path.extension().is_some_and(|ext| ext == "json") {
            serde_json::from_str(&data)
                .map_err(|error| format!("invalid manifest {}: {error}", path.display()))?
        } else {
            serde_yaml::from_str(&data)
                .map_err(|error| format!("invalid manifest {}: {error}", path.display()))?
        };
        Ok(manifest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_yaml_with_unknown_keys_ignored() {
        let manifest: VisualizationManifest = serde_yaml::from_str(
            r#"
version: 1
future_section:
  anything: goes
tracks:
  - event: solver_step
    domain: app
    track: solver
    lane: main
markers:
  - event: checkpoint
    glyph: flag
    severity: warn
counters:
  - event: residual
    unit: "1"
    render: stat
    critical: 10.0
views:
  - name: Solver
    tracks: [solver]
    counters: [residual]
"#,
        )
        .unwrap();
        assert_eq!(manifest.version, 1);
        assert_eq!(manifest.tracks[0].track, "solver");
        assert_eq!(manifest.markers[0].severity, Severity::Warn);
        assert_eq!(manifest.counters[0].render, CounterRender::Stat);
        assert_eq!(manifest.counters[0].critical, Some(10.0));
        assert_eq!(manifest.views[0].name, "Solver");
    }

    #[test]
    fn empty_manifest_is_valid() {
        let manifest: VisualizationManifest = serde_yaml::from_str("{}").unwrap();
        assert_eq!(manifest.version, 1);
        assert!(manifest.tracks.is_empty());
    }
}
