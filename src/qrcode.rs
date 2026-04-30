use anyhow::Result;
use qrcode::{Color, EcLevel, QrCode};

#[derive(Clone)]
pub struct TerminalQrCode {
    code: QrCode,
}

// Quadrant block elements (U+2596–U+259F + half/full blocks):
// 4 QR modules (2×2) → 1 character. Cuts the rendered QR to a
// quarter of the Dense1x2 footprint, which is the difference
// between "fits in one terminal screen" and "scroll to scan" for
// long Lark deep-link URLs.
//
// Bit layout (1 = render this quadrant as a bright glyph,
// i.e. the QR module is *light*; terminals are dark-bg, so dark
// modules get the empty quadrant of the glyph and rely on the
// background showing through):
//   bit 3 = top-left
//   bit 2 = top-right
//   bit 1 = bottom-left
//   bit 0 = bottom-right
const QUAD: [char; 16] = [
    ' ', '▗', '▖', '▄', '▝', '▐', '▞', '▟', '▘', '▚', '▌', '▙', '▀', '▜', '▛', '█',
];

impl TerminalQrCode {
    pub fn from_bytes<D: AsRef<[u8]>>(data: D) -> Result<TerminalQrCode> {
        // EcLevel::L (~7% recovery) is plenty for on-screen scanning
        // and produces a noticeably smaller QR than the default M
        // (~15%) — important for the long Lark URLs.
        let code = QrCode::with_error_correction_level(data.as_ref(), EcLevel::L)?;
        Ok(TerminalQrCode { code })
    }

    pub fn print(&self) {
        let width = self.code.width();
        let colors = self.code.to_colors();
        // Out-of-bounds modules are treated as light. QR widths are
        // always odd (21, 25, 29, …), so 2×2 blocking leaves a
        // half-row / half-column at the right & bottom edges; rendering
        // those as bright also gives the scanner a sliver of natural
        // quiet-zone padding.
        let is_light = |x: usize, y: usize| -> bool {
            x >= width || y >= width || matches!(colors[y * width + x], Color::Light)
        };
        let mut out = String::new();
        let mut y = 0;
        while y < width {
            let mut x = 0;
            while x < width {
                let idx = ((is_light(x, y) as u8) << 3)
                    | ((is_light(x + 1, y) as u8) << 2)
                    | ((is_light(x, y + 1) as u8) << 1)
                    | (is_light(x + 1, y + 1) as u8);
                out.push(QUAD[idx as usize]);
                x += 2;
            }
            out.push('\n');
            y += 2;
        }
        println!();
        print!("{out}");
        println!();
    }
}
