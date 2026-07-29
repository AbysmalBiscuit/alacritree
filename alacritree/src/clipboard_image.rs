//! Turning a clipboard bitmap into a file on disk that something else can open.
//!
//! Nothing here knows about the clipboard or about sessions: it takes pixels,
//! and it returns a path.  That is what keeps it testable without a window.

use std::fmt;

use arboard::ImageData;

/// A clipboard owner can advertise any dimensions it likes, and encoding runs
/// on the UI thread during a keystroke.  64 MP is far past any screenshot.
const MAX_PIXELS: usize = 64 * 1024 * 1024;

#[derive(Debug)]
pub enum EncodeError {
    TooLarge { pixels: usize },
    Inconsistent { expected: usize, actual: usize },
    Encoding(png::EncodingError),
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { pixels } => {
                write!(f, "{pixels} pixels is past the {MAX_PIXELS} limit")
            },
            Self::Inconsistent { expected, actual } => {
                write!(f, "dimensions imply {expected} bytes, got {actual}")
            },
            Self::Encoding(e) => write!(f, "{e}"),
        }
    }
}

/// `Compression::Fast` buys latency on a keypress at the cost of a larger file
/// that nothing keeps.
pub fn encode_png(image: &ImageData<'_>) -> Result<Vec<u8>, EncodeError> {
    let pixels = image.width.saturating_mul(image.height);
    if pixels > MAX_PIXELS {
        return Err(EncodeError::TooLarge { pixels });
    }
    let expected = pixels.saturating_mul(4);
    if image.bytes.len() != expected {
        return Err(EncodeError::Inconsistent { expected, actual: image.bytes.len() });
    }

    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, image.width as u32, image.height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().map_err(EncodeError::Encoding)?;
    writer.write_image_data(&image.bytes).map_err(EncodeError::Encoding)?;
    writer.finish().map_err(EncodeError::Encoding)?;
    Ok(out)
}

/// The file a set of PNG bytes belongs in.  Content-addressed, so pasting the
/// same screenshot twice reuses one file, and the full 64-bit digest rather
/// than the scratchpad's truncated one, since here a collision would paste the
/// wrong image instead of merely colliding a label.
pub fn file_name(png: &[u8]) -> String {
    format!("clipboard-{:016x}.png", crate::digest::stable_digest(png))
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;

    fn image(width: usize, height: usize) -> ImageData<'static> {
        let bytes = (0..width * height * 4).map(|i| (i % 251) as u8).collect::<Vec<_>>();
        ImageData { width, height, bytes: Cow::Owned(bytes) }
    }

    #[test]
    fn an_image_survives_the_encode_round_trip() {
        let source = image(7, 5);
        let png = encode_png(&source).expect("encodes");

        let decoder = png::Decoder::new(std::io::Cursor::new(&png));
        let mut reader = decoder.read_info().expect("valid png");
        let mut out = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut out).expect("one frame");

        assert_eq!((info.width, info.height), (7, 5));
        assert_eq!(info.color_type, png::ColorType::Rgba);
        assert_eq!(&out[..info.buffer_size()], source.bytes.as_ref());
    }

    /// A clipboard owner can advertise any dimensions it likes.  Reject before
    /// allocating, because this runs on the UI thread during a keystroke.
    #[test]
    fn an_absurdly_large_image_is_rejected_before_allocating() {
        let huge = ImageData { width: usize::MAX, height: 4, bytes: Cow::Owned(Vec::new()) };
        assert!(matches!(encode_png(&huge), Err(EncodeError::TooLarge { .. })));
    }

    #[test]
    fn a_byte_count_disagreeing_with_the_dimensions_is_rejected() {
        let lying = ImageData { width: 4, height: 4, bytes: Cow::Owned(vec![0; 8]) };
        assert!(matches!(encode_png(&lying), Err(EncodeError::Inconsistent { .. })));
    }

    /// The name is the deduplication key: equal bytes must land on one file.
    #[test]
    fn the_file_name_is_a_function_of_the_content() {
        assert_eq!(file_name(b"same"), file_name(b"same"));
        assert_ne!(file_name(b"one"), file_name(b"two"));
    }

    #[test]
    fn the_file_name_is_sixteen_hex_digits_and_inert() {
        let name = file_name(b"payload");
        let hex =
            name.strip_prefix("clipboard-").and_then(|r| r.strip_suffix(".png")).expect("shape");
        assert_eq!(hex.len(), 16);
        assert!(hex.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(crate::file_drop::is_terminal_safe(&name));
    }
}
