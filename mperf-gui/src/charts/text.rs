use gpui::{Hsla, SharedString, ShapedLine, Window, px};

/// Shapes one line of canvas text in the window's current text style.
pub fn shape_label(text: &str, size_px: f32, color: Hsla, window: &mut Window) -> ShapedLine {
    let style = window.text_style();
    let mut run = style.to_run(text.len());
    run.color = color;
    window.text_system().shape_line(
        SharedString::from(text.to_string()),
        px(size_px),
        &[run],
        None,
    )
}

/// Ellipsizes `label` to at most `max_chars` characters.
pub fn truncate_label(label: &str, max_chars: usize) -> String {
    if label.chars().count() <= max_chars {
        label.to_owned()
    } else {
        let mut truncated: String = label.chars().take(max_chars.saturating_sub(1)).collect();
        truncated.push('…');
        truncated
    }
}
