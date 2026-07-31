use std::{
    fs,
    path::{Path, PathBuf},
};

use gpui::ScrollHandle;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLocation {
    pub path: PathBuf,
    pub line: usize,
}

pub struct SourceDocument {
    pub path: PathBuf,
    pub title: String,
    pub lines: Vec<String>,
    pub focus_line: usize,
    pub error: Option<String>,
    pub scroll_handle: ScrollHandle,
}

impl SourceDocument {
    pub fn load(location: SourceLocation) -> Self {
        let title = location
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| location.path.display().to_string());

        match fs::read_to_string(&location.path) {
            Ok(contents) => {
                let lines = contents.lines().map(ToOwned::to_owned).collect();
                let scroll_handle = ScrollHandle::new();
                scroll_handle.scroll_to_item(location.line.saturating_sub(4));
                Self {
                    path: location.path,
                    title,
                    lines,
                    focus_line: location.line,
                    error: None,
                    scroll_handle,
                }
            }
            Err(error) => Self {
                path: location.path.clone(),
                title,
                lines: Vec::new(),
                focus_line: location.line,
                error: Some(format!(
                    "Could not read {}: {error}",
                    location.path.display()
                )),
                scroll_handle: ScrollHandle::new(),
            },
        }
    }

    pub fn matches(&self, path: &Path) -> bool {
        self.path == path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_source_is_still_a_document() {
        let document = SourceDocument::load(SourceLocation {
            path: PathBuf::from("/definitely/missing/source.rs"),
            line: 42,
        });

        assert_eq!(document.title, "source.rs");
        assert_eq!(document.focus_line, 42);
        assert!(document.error.is_some());
    }
}
