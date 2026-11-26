//! A screenshot at a size an agent can point into.
//!
//! A phone's screen is taller than the longest side most models look at, so the model would see a
//! shrunken picture and its coordinates would be off by a factor nobody told it. Ghost shrinks the
//! picture itself, by a whole number, and says so: a point in the picture times that number is a
//! point on the device.

use crate::Error;

/// The longest side a picture for an agent may have.
pub const LONGEST_SIDE: u32 = 1568;

/// A picture for an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// How many device pixels one pixel of this picture stands for, along each side.
    pub factor: u32,
}

fn unreadable() -> Error {
    Error::Device("The device's screenshot could not be read.".into())
}

/// The whole number to divide a `width` by `height` screen by so that it fits.
pub fn factor_for(width: u32, height: u32) -> u32 {
    width.max(height).div_ceil(LONGEST_SIDE).max(1)
}

/// Shrinks a PNG screenshot so its longest side fits, averaging the pixels it merges. A picture
/// that already fits is returned as it came.
pub fn for_agent(png: &[u8]) -> Result<Picture, Error> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|_| unreadable())?;
    let size = reader.output_buffer_size().ok_or_else(unreadable)?;
    let mut pixels = vec![0u8; size];
    let frame = reader.next_frame(&mut pixels).map_err(|_| unreadable())?;
    let (width, height) = (frame.width, frame.height);
    let factor = factor_for(width, height);
    if factor == 1 {
        return Ok(Picture {
            png: png.to_vec(),
            width,
            height,
            factor,
        });
    }
    let samples = frame.color_type.samples();
    let line = frame.line_size;
    let (out_width, out_height) = (width / factor, height / factor);
    if out_width == 0 || out_height == 0 || line < width as usize * samples {
        return Err(unreadable());
    }
    let step = factor as usize;
    let area = (step * step) as u32;
    let mut small = Vec::with_capacity(out_width as usize * out_height as usize * samples);
    for row in 0..out_height as usize {
        for column in 0..out_width as usize {
            for sample in 0..samples {
                let mut sum = 0u32;
                for y in 0..step {
                    let start = (row * step + y) * line + column * step * samples + sample;
                    for x in 0..step {
                        sum += u32::from(pixels[start + x * samples]);
                    }
                }
                small.push(((sum + area / 2) / area) as u8);
            }
        }
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, out_width, out_height);
        encoder.set_color(frame.color_type);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().map_err(|_| unreadable())?;
        writer.write_image_data(&small).map_err(|_| unreadable())?;
        writer.finish().map_err(|_| unreadable())?;
    }
    Ok(Picture {
        png: out,
        width: out_width,
        height: out_height,
        factor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(
        width: u32,
        height: u32,
        color: png::ColorType,
        pixel: impl Fn(u32, u32) -> Vec<u8>,
    ) -> Vec<u8> {
        let mut data = Vec::new();
        for y in 0..height {
            for x in 0..width {
                data.extend(pixel(x, y));
            }
        }
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(color);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&data).unwrap();
        writer.finish().unwrap();
        out
    }

    fn decode(png: &[u8]) -> (u32, u32, Vec<u8>) {
        let mut reader = png::Decoder::new(std::io::Cursor::new(png))
            .read_info()
            .unwrap();
        let mut pixels = vec![0u8; reader.output_buffer_size().unwrap()];
        let frame = reader.next_frame(&mut pixels).unwrap();
        (frame.width, frame.height, pixels)
    }

    #[test]
    fn factors_are_whole_numbers_that_make_the_longest_side_fit() {
        assert_eq!(factor_for(1080, 2400), 2);
        assert_eq!(factor_for(2400, 1080), 2);
        assert_eq!(factor_for(1440, 3200), 3);
        assert_eq!(factor_for(720, 1568), 1);
        assert_eq!(factor_for(720, 1569), 2);
        assert_eq!(factor_for(0, 0), 1);
    }

    #[test]
    fn a_small_picture_is_returned_untouched() {
        let original = png(4, 6, png::ColorType::Rgba, |_, _| vec![1, 2, 3, 255]);
        let picture = for_agent(&original).unwrap();
        assert_eq!(picture.png, original);
        assert_eq!((picture.width, picture.height, picture.factor), (4, 6, 1));
    }

    #[test]
    fn a_tall_picture_is_halved_by_averaging() {
        // Left half black, right half white, with one grey column pair in the middle.
        let original = png(1080, 2400, png::ColorType::Rgba, |x, _| {
            let value = match x {
                0..=537 => 0,
                538 => 0,
                539 => 255,
                _ => 255,
            };
            vec![value, value, value, 255]
        });
        let picture = for_agent(&original).unwrap();
        assert_eq!(
            (picture.width, picture.height, picture.factor),
            (540, 1200, 2)
        );
        let (width, height, pixels) = decode(&picture.png);
        assert_eq!((width, height), (540, 1200));
        let at = |x: usize, y: usize| pixels[(y * 540 + x) * 4];
        assert_eq!(at(0, 0), 0);
        assert_eq!(at(539, 1199), 255);
        // Columns 538 and 539 merged: half black, half white.
        assert_eq!(at(269, 600), 128);
        assert_eq!(pixels[3], 255, "alpha is kept");
    }

    #[test]
    fn rgb_without_alpha_and_odd_sizes_work() {
        let original = png(1081, 3201, png::ColorType::Rgb, |x, y| {
            vec![(x % 256) as u8, (y % 256) as u8, 7]
        });
        let picture = for_agent(&original).unwrap();
        assert_eq!(picture.factor, 3);
        assert_eq!((picture.width, picture.height), (360, 1067));
        let (_, _, pixels) = decode(&picture.png);
        assert_eq!(pixels.len(), 360 * 1067 * 3);
        // The first block averages x in 0..3 and y in 0..3.
        assert_eq!(&pixels[..3], [1, 1, 7]);
    }

    #[test]
    fn junk_is_an_error_not_a_panic() {
        assert!(for_agent(b"").is_err());
        assert!(for_agent(b"not a png at all").is_err());
        let mut cut = png(2000, 2000, png::ColorType::Rgba, |_, _| vec![0, 0, 0, 255]);
        cut.truncate(cut.len() / 2);
        assert!(for_agent(&cut).is_err());
    }
}
