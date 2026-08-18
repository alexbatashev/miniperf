use gpui::{
    App, Context, DispatchPhase, Entity, Hitbox, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Window,
};

use super::frame::PlotFrame;

/// Drag state of a horizontal time brush. One instance is shared by every
/// chart that brushes the same time axis, so a drag started on the master
/// timeline previews on the flame scope too.
#[derive(Clone, Copy, Default)]
pub struct Brush {
    pub anchor: Option<f64>,
    pub preview: Option<(f64, f64)>,
}

impl Brush {
    /// The range to paint: the in-flight drag wins over the committed filter.
    pub fn shown(&self, committed: Option<(f64, f64)>) -> Option<(f64, f64)> {
        self.preview.or(committed)
    }

    pub fn clear(&mut self) {
        self.anchor = None;
        self.preview = None;
    }
}

/// Wires press/drag/release on `hitbox` to `brush`, committing the range on
/// release and clearing it on double-click. Handlers re-register every paint,
/// so this must be called from the canvas paint closure.
pub fn register_time_brush<V: 'static>(
    entity: &Entity<V>,
    frame: PlotFrame,
    hitbox: &Hitbox,
    duration: f64,
    window: &mut Window,
    brush: impl Fn(&mut V) -> &mut Brush + Copy + 'static,
    commit: impl Fn(&mut V, Option<(f64, f64)>, &mut Context<V>) + Copy + 'static,
) {
    let hitbox = hitbox.clone();
    let down_entity = entity.clone();
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx: &mut App| {
        if phase != DispatchPhase::Bubble
            || !hitbox.is_hovered(window)
            || !frame.in_plot(event.position.x)
        {
            return;
        }
        let time = frame.time_at(event.position.x, duration);
        down_entity.update(cx, |view, cx| {
            if event.click_count >= 2 {
                brush(view).clear();
                commit(view, None, cx);
            } else {
                let state = brush(view);
                state.anchor = Some(time);
                state.preview = Some((time, time));
            }
            cx.notify();
        });
    });

    let move_entity = entity.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx: &mut App| {
        if phase != DispatchPhase::Bubble || !event.dragging() {
            return;
        }
        let time = frame.time_at(event.position.x, duration);
        move_entity.update(cx, |view, cx| {
            let state = brush(view);
            if let Some(anchor) = state.anchor {
                state.preview = Some((anchor.min(time), anchor.max(time)));
                cx.notify();
            }
        });
    });

    let up_entity = entity.clone();
    window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx: &mut App| {
        if phase != DispatchPhase::Bubble {
            return;
        }
        up_entity.update(cx, |view, cx| {
            let state = brush(view);
            if state.anchor.take().is_some() {
                let range = state
                    .preview
                    .take()
                    .filter(|(start, end)| end - start > duration * 0.002);
                commit(view, range, cx);
                cx.notify();
            }
        });
    });
}
