//! QR payload handling. wuzapi emits the pairing QR as a `data:image/png;base64,…`
//! URL, not as the raw pairing text, so the text is recovered by decoding the
//! image; that lets callers redraw the code at any size (for example in a
//! terminal).

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use qrcode::QrCode as Encoder;
use serde::{Deserialize, Serialize};

/// A pairing QR code, as emitted by wuzapi.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QrCode {
    /// The image exactly as wuzapi sent it (`data:image/png;base64,…`).
    pub data_url: String,
    /// The text encoded in the image, if it could be decoded. Render this
    /// with [`QrCode::render_terminal`] or any QR library.
    pub text: Option<String>,
}

impl QrCode {
    /// Build from wuzapi's data URL, decoding the embedded PNG.
    pub fn from_data_url(data_url: &str) -> Self {
        QrCode {
            data_url: data_url.to_string(),
            text: decode_data_url(data_url),
        }
    }

    /// The code as dense Unicode half-blocks with a quiet zone, or `None` if
    /// the text could not be recovered from the image.
    pub fn render_terminal(&self) -> Option<String> {
        render_terminal(self.text.as_deref()?)
    }
}

/// Render arbitrary text as a terminal QR code, two module rows per text line
/// (upper-half blocks). Black and white are set with explicit ANSI colors, so
/// the code has the correct polarity on light and dark terminal themes alike.
pub fn render_terminal(text: &str) -> Option<String> {
    const QUIET: usize = 4;
    let code = Encoder::new(text.as_bytes()).ok()?;
    let width = code.width();
    let colors = code.to_colors();
    let dark = |x: usize, y: usize| {
        let (Some(x), Some(y)) = (x.checked_sub(QUIET), y.checked_sub(QUIET)) else {
            return false;
        };
        x < width && y < width && colors[y * width + x] == qrcode::Color::Dark
    };
    // Foreground paints the upper half block, background the lower half.
    let fg = |dark: bool| if dark { "30" } else { "97" };
    let bg = |dark: bool| if dark { "40" } else { "107" };
    let side = width + 2 * QUIET;
    Some(
        (0..side)
            .step_by(2)
            .map(|y| {
                let row: String = (0..side)
                    .map(|x| format!("\x1b[{};{}m\u{2580}", fg(dark(x, y)), bg(dark(x, y + 1))))
                    .collect();
                format!("{row}\x1b[0m")
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn decode_data_url(data_url: &str) -> Option<String> {
    let (_, b64) = data_url.split_once(";base64,")?;
    let bytes = STANDARD.decode(b64.trim()).ok()?;
    decode_png(&bytes)
}

fn decode_png(bytes: &[u8]) -> Option<String> {
    let mut decoder = png::Decoder::new(bytes);
    // Expand palettes and 1/2/4-bit samples to 8-bit so pixels are addressable.
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    let channels = info.color_type.samples();
    let (width, height) = (info.width as usize, info.height as usize);
    let luma = |x: usize, y: usize| -> u8 {
        let Some(px) = buf
            .get(y * info.line_size + x * channels..)
            .and_then(|rest| rest.get(..channels))
        else {
            return 255;
        };
        match info.color_type {
            png::ColorType::Rgb | png::ColorType::Rgba => {
                ((u16::from(px[0]) * 30 + u16::from(px[1]) * 59 + u16::from(px[2]) * 11) / 100)
                    as u8
            }
            _ => px[0],
        }
    };
    let mut image = rqrr::PreparedImage::prepare_from_greyscale(width, height, luma);
    image
        .detect_grids()
        .into_iter()
        .find_map(|grid| grid.decode().ok().map(|(_, content)| content))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Draw `text` as a grayscale PNG data URL, like wuzapi does.
    pub(crate) fn fake_wuzapi_qr(text: &str) -> String {
        draw(text, false)
    }

    /// `one_bit` mimics wuzapi's real output: a 1-bit grayscale PNG.
    fn draw(text: &str, one_bit: bool) -> String {
        let code = Encoder::new(text.as_bytes()).unwrap();
        let modules = code.width();
        let scale = 6;
        let quiet = 4;
        let side = (modules + 2 * quiet) * scale;
        let colors = code.to_colors();
        let pixels: Vec<u8> = (0..side)
            .flat_map(|y| (0..side).map(move |x| (x, y)))
            .map(|(x, y)| {
                let (mx, my) = (x / scale, y / scale);
                let inside =
                    mx >= quiet && my >= quiet && mx < modules + quiet && my < modules + quiet;
                match inside.then(|| colors[(my - quiet) * modules + (mx - quiet)]) {
                    Some(qrcode::Color::Dark) => 0,
                    _ => 255,
                }
            })
            .collect();
        let (depth, data) = if one_bit {
            let packed = pixels
                .chunks(side)
                .flat_map(|row| {
                    row.chunks(8).map(|bits| {
                        bits.iter()
                            .enumerate()
                            .fold(0u8, |acc, (i, px)| acc | (u8::from(*px > 127) << (7 - i)))
                    })
                })
                .collect();
            (png::BitDepth::One, packed)
        } else {
            (png::BitDepth::Eight, pixels)
        };
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, side as u32, side as u32);
            enc.set_color(png::ColorType::Grayscale);
            enc.set_depth(depth);
            enc.write_header().unwrap().write_image_data(&data).unwrap();
        }
        format!("data:image/png;base64,{}", STANDARD.encode(out))
    }

    #[test]
    fn decodes_the_pairing_text_from_a_png_data_url() {
        let text = "2@FAKEPAYLOAD,ZmFrZQ==,Zm9v,YmFy";
        let qr = QrCode::from_data_url(&fake_wuzapi_qr(text));
        assert_eq!(qr.text.as_deref(), Some(text));
        let art = qr.render_terminal().unwrap();
        assert!(art.contains("\u{2580}") && art.contains("\x1b[30;40m"));
        // Version 3 symbols are 29 modules wide: (29 + 8) rows, two per line.
        assert!(art.lines().count() >= 15);
    }

    #[test]
    fn decodes_one_bit_pngs_like_the_real_thing() {
        let text = "2@ONEBIT,Zm9v,YmFy,YmF6";
        let qr = QrCode::from_data_url(&draw(text, true));
        assert_eq!(qr.text.as_deref(), Some(text));
    }

    #[test]
    fn undecodable_payloads_keep_the_raw_url() {
        for url in [
            "",
            "data:image/png;base64,@@@",
            "data:image/png;base64,AAAA",
        ] {
            let qr = QrCode::from_data_url(url);
            assert_eq!(qr.text, None);
            assert_eq!(qr.data_url, url);
            assert_eq!(qr.render_terminal(), None);
        }
    }
}
