// Zed-style UI roles on a deliberately neutral near-black palette.
pub const WORKSPACE: u32 = 0x0d0d0e;
pub const CHROME: u32 = 0x171718;
pub const SURFACE: u32 = 0x1d1d1f;
pub const TITLE_BAR: u32 = 0x1a1a1b;
pub const STATUS_BAR: u32 = 0x171718;
pub const TOOLBAR: u32 = 0x101011;
pub const HOVER: u32 = 0x252527;
pub const BORDER: u32 = 0x303033;
pub const TEXT: u32 = 0xe2e2e3;
pub const MUTED_TEXT: u32 = 0x929296;
pub const ACCENT: u32 = 0xd2a45f;
pub const SELECTION: u32 = 0xd2a45f;
pub const SELECTION_MUTED: u32 = 0x3a3329;
pub const ERROR: u32 = 0xdf6b70;

pub const TITLE_BAR_HEIGHT: f32 = 34.0;
pub const TRAFFIC_LIGHT_SPACE: f32 = 78.0;
pub const DOCUMENT_BAR_HEIGHT: f32 = 30.0;
pub const STATUS_BAR_HEIGHT: f32 = 20.0;
pub const SIDEBAR_DEFAULT_WIDTH: f32 = 208.0;
pub const SIDEBAR_MIN_WIDTH: f32 = 160.0;
pub const SIDEBAR_MAX_WIDTH: f32 = 360.0;
pub const BOTTOM_PANEL_DEFAULT_HEIGHT: f32 = 220.0;
pub const BOTTOM_PANEL_MIN_HEIGHT: f32 = 120.0;
pub const BOTTOM_PANEL_MAX_HEIGHT: f32 = 420.0;
pub const FLAME_FRAME_HEIGHT: f32 = 18.0;

pub fn flame_frame_color(name: &str, depth: usize) -> u32 {
    let mut hash = depth as u64;
    for byte in name.bytes() {
        hash = hash.wrapping_mul(16_777_619) ^ u64::from(byte);
    }

    // Warm, desaturated hues distinguish frames without tinting the whole app.
    const PALETTE: [u32; 10] = [
        0x765044, 0x6e5a42, 0x696347, 0x63505b, 0x625a50, 0x704a4a, 0x5a604d, 0x67513f, 0x5f4d59,
        0x6e6448,
    ];
    PALETTE[hash as usize % PALETTE.len()]
}
