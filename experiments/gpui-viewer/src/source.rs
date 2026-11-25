//! Shared opaque RGBA8 input for both viewers.

use std::io::{self, Read};

/// Bound allocations for arbitrary raw inputs and prohibit a zero-sized frame.
pub fn rgba_len(width: u32, height: u32) -> Option<usize> {
    if !(16..=4096).contains(&width) || !(16..=4096).contains(&height) {
        return None;
    }
    (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)
        .filter(|&bytes| bytes <= 64 * 1024 * 1024)
}

pub struct Pattern {
    width: usize,
    height: usize,
    background: Vec<u8>,
}

impl Pattern {
    pub fn new(width: u32, height: u32) -> Option<Self> {
        let mut background = vec![0; rgba_len(width, height)?];
        let (width, height) = (width as usize, height as usize);
        for (index, pixel) in background.chunks_exact_mut(4).enumerate() {
            let (x, y) = (index % width, index / width);
            let shade = if (x / 64 + y / 64) % 2 == 0 { 20 } else { 46 };
            pixel.copy_from_slice(&[
                shade + (100 * x / width) as u8,
                shade + (95 * y / height) as u8,
                87,
                255,
            ]);
        }
        Some(Self {
            width,
            height,
            background,
        })
    }

    pub fn write_rgba(&self, sequence: u64, output: &mut [u8]) {
        output.copy_from_slice(&self.background);
        let box_w = (self.width / 6).max(1);
        let box_h = (self.height / 5).max(1);
        let x = (sequence.wrapping_mul(13) % ((self.width - box_w) as u64)) as usize;
        let y = (sequence.wrapping_mul(7) % ((self.height - box_h) as u64)) as usize;
        let stride = self.width * 4;
        for row in output.chunks_exact_mut(stride).skip(y).take(box_h) {
            for pixel in row[x * 4..(x + box_w) * 4].chunks_exact_mut(4) {
                pixel.copy_from_slice(&[247, 124, 44, 255]);
            }
        }
        // Encode all sequence bits in eight RGBA pixels. This distinguishes even
        // frames whose moving rectangle happens to return to the same position.
        for (index, byte) in sequence.to_le_bytes().into_iter().enumerate() {
            output[index * 4..index * 4 + 4].copy_from_slice(&[byte, 240 - index as u8, 215, 255]);
        }
    }
}

/// Read one whole frame. A clean EOF occurs only before the first byte.
pub fn read_rgba(reader: &mut impl Read, output: &mut [u8]) -> io::Result<bool> {
    if output.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "empty RGBA frame",
        ));
    }
    loop {
        match reader.read(&mut output[..1]) {
            Ok(0) => return Ok(false),
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    reader.read_exact(&mut output[1..])?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_is_repeatable_and_frame_ids_do_not_alias() {
        let pattern = Pattern::new(48, 32).unwrap();
        let mut a = vec![0; rgba_len(48, 32).unwrap()];
        let mut b = a.clone();
        pattern.write_rgba(300, &mut a);
        pattern.write_rgba(301, &mut b);
        assert_ne!(a, b);
        assert!(a.chunks_exact(4).all(|pixel| pixel[3] == 255));
        pattern.write_rgba(300, &mut b);
        assert_eq!(a, b);
    }

    #[test]
    fn clean_eof_and_truncated_frame_are_distinct() {
        let mut pixels = [0; 4];
        assert!(read_rgba(&mut &b"rgba"[..], &mut pixels).unwrap());
        assert_eq!(&pixels, b"rgba");
        assert!(!read_rgba(&mut &b""[..], &mut pixels).unwrap());
        assert_eq!(
            read_rgba(&mut &b"rgb"[..], &mut pixels).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
}
