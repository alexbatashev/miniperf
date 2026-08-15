use gpui::{App, Div, div, prelude::*, px};

use super::theme::ActiveTheme;

pub fn separator(cx: &App) -> Div {
    div().flex_none().h(px(1.0)).w_full().bg(cx.theme().border)
}

pub fn separator_vertical(cx: &App) -> Div {
    div().flex_none().w(px(1.0)).h_full().bg(cx.theme().border)
}
