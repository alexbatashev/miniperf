use std::{fs, path::PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLocation {
    pub path: PathBuf,
    pub line: usize,
}

pub struct SourceDocument {
    pub path: PathBuf,
    pub lines: Vec<String>,
    pub focus_line: usize,
    pub error: Option<String>,
}

impl SourceDocument {
    pub fn load(location: SourceLocation) -> Self {
        match fs::read_to_string(&location.path) {
            Ok(contents) => Self {
                path: location.path,
                lines: contents.lines().map(ToOwned::to_owned).collect(),
                focus_line: location.line,
                error: None,
            },
            Err(error) => Self {
                path: location.path.clone(),
                lines: Vec::new(),
                focus_line: location.line,
                error: Some(format!(
                    "Could not read {}: {error}",
                    location.path.display()
                )),
            },
        }
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

        assert_eq!(document.focus_line, 42);
        assert!(document.error.is_some());
    }
}
