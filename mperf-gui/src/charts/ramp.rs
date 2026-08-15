use gpui::{Hsla, rgb};

/// Sequential blue ramp shared by the flame-scope heatmap and the source heat
/// gutters, ported from the prototype. Identical in both themes so that heat
/// reads the same on a screenshot.
const RAMP: [u32; 13] = [
    0xcde2fb, 0xb7d3f6, 0x9ec5f4, 0x86b6ef, 0x6da7ec, 0x5598e7, 0x3987e5, 0x2a78d6, 0x256abf,
    0x1c5cab, 0x184f95, 0x104281, 0x0d366b,
];

/// Maps a 0..=1 intensity to the ramp with a √ transfer, matching the
/// prototype's perceptual weighting of sparse bins.
pub fn heat(fraction: f32) -> Hsla {
    let index = (fraction.clamp(0.0, 1.0).sqrt() * RAMP.len() as f32) as usize;
    rgb(RAMP[index.min(RAMP.len() - 1)]).into()
}
