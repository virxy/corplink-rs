use anyhow::Result;
use qrcode::render::unicode::Dense1x2;
use qrcode::{EcLevel, QrCode};

#[derive(Clone)]
pub struct TerminalQrCode {
    code: QrCode,
}

impl TerminalQrCode {
    pub fn from_bytes<D: AsRef<[u8]>>(data: D) -> Result<TerminalQrCode> {
        // EcLevel::L (~7%) is plenty for on-screen scanning and keeps
        // the QR small for long Lark deep-link URLs.
        let code = QrCode::with_error_correction_level(data.as_ref(), EcLevel::L)?;
        Ok(TerminalQrCode { code })
    }

    pub fn print(&self) {
        // Dense1x2 (▀▄█ ' '): 1 character = 2 vertical QR modules.
        // Monospace chars are roughly 2:1 height:width, so this maps
        // each module to ~1×1 physical pixels — the QR stays visually
        // square. Quadrant-block (1 char = 2×2 modules) is 4× denser
        // but stretches the QR vertically by 2× under the same
        // aspect ratio, so we trade footprint for fidelity.
        // quiet_zone(false) skips the standard 4-module padding —
        // on-screen scanners don't enforce it, claws back 8 cols + 4 rows.
        let image = self
            .code
            .render::<Dense1x2>()
            .quiet_zone(false)
            .dark_color(Dense1x2::Light)
            .light_color(Dense1x2::Dark)
            .build();
        println!();
        print!("{image}");
        println!();
    }
}
