use std::rc::Rc;

use gpui::{
    App, Axis, Context, CursorStyle, DispatchPhase, IntoElement, MouseButton, MouseMoveEvent,
    MouseUpEvent, Pixels, Render, Window, canvas, div, fill, point, prelude::*, px, size,
};

use super::theme::ActiveTheme;

/// Resizable-pane handle. Owns its drag state (no global root flags): while
/// dragging, its canvas registers window-wide move/up handlers for that
/// frame and reports the pointer position along its axis via `on_drag`.
pub struct Splitter {
    axis: Axis,
    dragging: bool,
    on_drag: Rc<dyn Fn(Pixels, &mut Window, &mut App)>,
}

impl Splitter {
    /// `axis` is the axis the divider separates along: `Axis::Horizontal`
    /// resizes left/right panes (vertical bar), `Axis::Vertical` top/bottom.
    pub fn new(axis: Axis, on_drag: impl Fn(Pixels, &mut Window, &mut App) + 'static) -> Self {
        Self {
            axis,
            dragging: false,
            on_drag: Rc::new(on_drag),
        }
    }
}

impl Render for Splitter {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let horizontal = self.axis == Axis::Horizontal;
        let axis = self.axis;
        let dragging = self.dragging;
        let on_drag = self.on_drag.clone();
        let entity = cx.entity();

        div()
            .id("splitter-handle")
            .flex_none()
            .when(horizontal, |el| el.w(px(5.0)).h_full().cursor_col_resize())
            .when(!horizontal, |el| el.h(px(5.0)).w_full().cursor_row_resize())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.dragging = true;
                    cx.notify();
                }),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let line = if horizontal {
                            gpui::Bounds::new(
                                point(bounds.left() + px(2.0), bounds.top()),
                                size(px(1.0), bounds.size.height),
                            )
                        } else {
                            gpui::Bounds::new(
                                point(bounds.left(), bounds.top() + px(2.0)),
                                size(bounds.size.width, px(1.0)),
                            )
                        };
                        window.paint_quad(fill(line, theme.border));

                        if !dragging {
                            return;
                        }
                        window.set_window_cursor_style(if horizontal {
                            CursorStyle::ResizeLeftRight
                        } else {
                            CursorStyle::ResizeUpDown
                        });

                        let move_on_drag = on_drag.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase != DispatchPhase::Bubble || !event.dragging() {
                                return;
                            }
                            let position = match axis {
                                Axis::Horizontal => event.position.x,
                                Axis::Vertical => event.position.y,
                            };
                            move_on_drag(position, window, cx);
                        });

                        let up_entity = entity.clone();
                        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                            if phase != DispatchPhase::Bubble {
                                return;
                            }
                            up_entity.update(cx, |this, cx| {
                                if this.dragging {
                                    this.dragging = false;
                                    cx.notify();
                                }
                            });
                        });
                    },
                )
                .size_full(),
            )
    }
}
