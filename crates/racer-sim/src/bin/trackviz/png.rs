//! A minimal PNG writer: RGB, 8 bits, no compression.
//!
//! The sim takes no dependencies, and its tools follow suit. A PNG is a
//! signature and a list of chunks, each `length, type, data, CRC-32`. The
//! image data is a zlib stream; zlib allows "stored" blocks that hold bytes
//! as they are, so an encoder needs no compressor, only the two checksums
//! (CRC-32 over each chunk, Adler-32 over the raw data). The files are big
//! (3 bytes a pixel) but this is a debugging picture, not an asset.

/// Encode `rgb` (row-major, 3 bytes a pixel) as a PNG file.
pub fn encode(width: u32, height: u32, rgb: &[u8]) -> Vec<u8> {
    assert_eq!(rgb.len(), (width * height * 3) as usize);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();

    let mut ihdr = Vec::new();
    ihdr.extend(width.to_be_bytes());
    ihdr.extend(height.to_be_bytes());
    // 8-bit depth, colour type 2 (RGB), default compression, filter and no
    // interlace.
    ihdr.extend([8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);

    // Each scanline starts with its filter type; 0 is "none".
    let row = width as usize * 3;
    let mut raw = Vec::with_capacity((row + 1) * height as usize);
    for y in 0..height as usize {
        raw.push(0);
        raw.extend(&rgb[y * row..(y + 1) * row]);
    }
    chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    chunk(&mut out, b"IEND", &[]);
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend((data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend(kind);
    out.extend(data);
    let crc = crc32(&out[start..]);
    out.extend(crc.to_be_bytes());
}

/// A zlib stream of stored (uncompressed) deflate blocks.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    // CMF 0x78: deflate, 32 KiB window. FLG 0x01 makes the header a multiple
    // of 31, as zlib requires.
    let mut out = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = data.chunks(65_535).collect();
    for (i, b) in blocks.iter().enumerate() {
        out.push(u8::from(i + 1 == blocks.len()));
        let len = b.len() as u16;
        out.extend(len.to_le_bytes());
        out.extend((!len).to_le_bytes());
        out.extend(*b);
    }
    out.extend(adler32(data).to_be_bytes());
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}
