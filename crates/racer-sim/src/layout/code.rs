//! Codes: a layout as one line of text. DESIGN §9.5.
//!
//! ```text
//!   RL 0 4 .............................................. 7 Q
//!   ── ─────────────────── base32 ────────────────────────────
//!   prefix   format byte · packed fields · CRC-16
//! ```
//!
//! Two prefixes, one body. **`RL`** is the share code: a *proven* layout,
//! carrying its timers and stamp; it is the only code the game shares or
//! races with a timer. **`RD`** is a draft code: the same road with no
//! proof, for files, the tools and (Phase 2) the editor's clipboard; it is
//! driven freely, never timed. [`Layout::to_code`] writes whichever fits.
//!
//! After the prefix, Crockford base32 (`0`–`9` and the letters but `I`,
//! `L`, `O`, `U`; case does not matter, `O` reads as `0`, `I` and `L` as
//! `1`, spaces and hyphens are skipped) of the format byte
//! ([`LAYOUT_FORMAT`]), a bit-packed body and a CRC-16 of both
//! (`codec.rs`, Salt Line's). The body, most significant bit first:
//!
//! | Field | Bits |
//! |---|---|
//! | segment count | 12 |
//! | name length, then each character's index in `NAME_CHARSET` | 4, then 6 each |
//! | curve channel, then pitch channel (numeric, below) | |
//! | half width, surface and flags channels (runs, below) | |
//! | gate count − 1, then the first gate and each later gap − 1 | 3, then γ each |
//! | object count, then per object: segment step, zigzag x step, kind | γ, then γ + γ + 1 |
//! | traffic count, then per car: zigzag steps of start, speed and lane | γ, then γ + γ + γ |
//! | proven, then each timer, the epoch and the stamp | 1, then γ each, γ, 32 |
//! | biome, format 2 only (1–4: not [`Biome::Meadow`]) | 3 |
//!
//! then zero bits to the end of the byte (γ is Elias gamma: one bit for 0,
//! three for 1–2, five for 3–6). "Step" is the difference from the
//! object or car before (the first from zero): post rows and packs cost a
//! few bits each.
//!
//! **Numeric channels** (curve, pitch): the first value, then each later
//! one as a first difference or as a second difference (the change in
//! slope), whichever codes shorter, one bit saying which. Bends ease in and
//! out with a smoothstep and hills are bumps, so both turn into runs of
//! small numbers and long runs of zeros; the residuals are zigzagged and
//! written as Salt Line writes ground: plain Rice at the best `k`, or zero
//! runs and the values between, each with its own `k`, again whichever is
//! shorter.
//!
//! **Runs channels** (half width, surface, flags): run-length, since they
//! change a handful of times a track. The run count − 1, then each run's
//! value and (but the last's, which is implied) its length − 1, all γ. Two
//! runs side by side never share a value, so each road has one spelling.
//!
//! **Formats.** A [`Biome::Meadow`] layout is written in format 1
//! ([`LAYOUT_FORMAT_PLAIN`]), with no biome field, so every code made
//! before biomes existed is still the code of the same layout, byte for
//! byte, and reads as a meadow. Any other biome is written in format 2
//! ([`LAYOUT_FORMAT`]): format 1's body and three bits of biome. Each
//! layout has one spelling: format 2 refuses a meadow. The biome is not in
//! the layout's digest or its proof stamp, so repainting a proven layout
//! keeps its proof.
//!
//! **Totality.** [`Layout::from_code`] never panics and never allocates
//! more than a bound: text longer than [`MAX_CODE_CHARS`] is refused before
//! anything is decoded, every count is read from a fixed-width field or
//! checked against its limit before it is used, every read past the end is
//! an error, and a decoded layout passes [`Layout::structure`] (so it
//! always builds a [`crate::track::Track`]). Any text is a layout or a
//! [`CodeError`].

use super::{
    valid_name, Layout, Proof, AUTHORED_FLAGS, MAX_GATES, MAX_OBJECTS, MAX_SEGMENTS, MAX_TRAFFIC,
    NAME_CHARSET, NAME_MAX,
};
use crate::biome::Biome;
use crate::codec::{
    best_k, frame, get_gamma, get_rice, put_gamma, put_rice, unframe, unzigzag, zigzag,
    Base32Error, BitReader, BitWriter, FrameError,
};
use crate::track::{Object, ObjectKind, Segment, Surface};
use crate::traffic::TrafficCar;

/// What a share code (a proven layout) starts with.
pub const SHARE_PREFIX: &str = "RL";
/// What a draft code (an unproven layout) starts with.
pub const DRAFT_PREFIX: &str = "RD";
/// Either prefix, for finding a code in text.
pub const CODE_PREFIXES: [&str; 2] = [SHARE_PREFIX, DRAFT_PREFIX];

/// The newest format the bytes are in: format 1's body and a biome. A
/// decoder reads this and [`LAYOUT_FORMAT_PLAIN`] and refuses any other: a
/// later format is a new number, and this decoder's job is to say it
/// cannot read it, not to guess. Independent of `SIM_EPOCH` (DESIGN §9.4).
pub const LAYOUT_FORMAT: u8 = 2;

/// Format 1, the first: no biome field. Still written for a
/// [`Biome::Meadow`] layout, so a meadow's code is the code it always was.
pub const LAYOUT_FORMAT_PLAIN: u8 = 1;

/// Bits of the biome field (format 2).
const BIOME_BITS: u32 = 3;

/// The longest text [`Layout::from_code`] looks at. The longest code any
/// structurally valid layout can have ([`MAX_SEGMENTS`] segments of noise
/// in every channel, every object and car) is under 100,000 characters;
/// typical tracks are a few hundred (DESIGN §9.5).
pub const MAX_CODE_CHARS: usize = 131_072;

/// Bits of the segment count.
const COUNT_BITS: u32 = 12;

/// Why a code could not be written or read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CodeError {
    /// Encoding: the layout has a structural problem
    /// ([`Layout::structure`]).
    Unencodable,
    /// Decoding: the text starts with neither prefix.
    Prefix,
    /// Decoding: longer than [`MAX_CODE_CHARS`].
    TooLong,
    /// Decoding: a character that is not base32.
    Character,
    /// Decoding: the last character carries bits past the last whole byte
    /// that are not zero: a cut-off or garbled code.
    Padding,
    /// Decoding: a format this build does not read.
    Format(u8),
    /// Decoding: the checksum does not match: a typo, or a cut-off code.
    Checksum,
    /// Decoding: the checksum matched but the body is not a layout (a field
    /// out of range, ends early, carries bits past its end, or a proof
    /// under a draft prefix or none under a share prefix).
    Malformed,
}

impl std::fmt::Display for CodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            CodeError::Unencodable => write!(f, "the layout cannot be written as a code"),
            CodeError::Prefix => write!(f, "not a layout code (no RL or RD)"),
            CodeError::TooLong => write!(f, "too long to be a layout code"),
            CodeError::Character => write!(f, "a character that is not in a code"),
            CodeError::Padding => write!(f, "cut off or garbled"),
            CodeError::Format(v) => write!(f, "a newer code format ({v})"),
            CodeError::Checksum => write!(f, "checksum mismatch: a typo or a cut-off code"),
            CodeError::Malformed => write!(f, "not a layout"),
        }
    }
}

impl Layout {
    /// The layout as a code: [`SHARE_PREFIX`] with its proof if it has one,
    /// else [`DRAFT_PREFIX`]. Whether the proof still stands is not
    /// checked here ([`Layout::standing`]); a share code is only *made*
    /// from a fresh proof by the game and the tools. Fails only for a
    /// structural problem.
    pub fn to_code(&self) -> Result<String, CodeError> {
        let prefix = if self.proof.is_some() {
            SHARE_PREFIX
        } else {
            DRAFT_PREFIX
        };
        Ok(frame(prefix, self.to_bytes()?))
    }

    /// The bytes inside the code: the format byte and the packed body,
    /// without the prefix, base32 or checksum.
    pub fn to_bytes(&self) -> Result<Vec<u8>, CodeError> {
        self.structure().map_err(|_| CodeError::Unencodable)?;
        Ok(if self.biome == Biome::Meadow {
            [vec![LAYOUT_FORMAT_PLAIN], encode_body(self, None)].concat()
        } else {
            let biome = self.biome.code() as u32;
            [vec![LAYOUT_FORMAT], encode_body(self, Some(biome))].concat()
        })
    }

    /// The layout [`Layout::to_bytes`] wrote. Total.
    pub fn from_bytes(bytes: &[u8]) -> Result<Layout, CodeError> {
        match bytes.split_first() {
            None => Err(CodeError::Malformed),
            Some((&format @ (LAYOUT_FORMAT_PLAIN | LAYOUT_FORMAT), body)) => {
                decode_body(body, format).ok_or(CodeError::Malformed)
            }
            Some((&format, _)) => Err(CodeError::Format(format)),
        }
    }

    /// The layout a code holds. Total: any text is a layout or an error. A
    /// share code's layout has a proof, a draft code's none; neither is
    /// checked beyond its structure (see [`Layout::standing`] and
    /// [`Layout::check`]).
    pub fn from_code(text: &str) -> Result<Layout, CodeError> {
        if text.len() > MAX_CODE_CHARS {
            return Err(CodeError::TooLong);
        }
        let text = text.trim();
        let (prefix, proven) = if text
            .get(..SHARE_PREFIX.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(SHARE_PREFIX))
        {
            (SHARE_PREFIX, true)
        } else {
            (DRAFT_PREFIX, false)
        };
        let bytes = unframe(prefix, text).map_err(|e| match e {
            FrameError::Prefix => CodeError::Prefix,
            FrameError::Base32(Base32Error::Character) => CodeError::Character,
            FrameError::Base32(Base32Error::Padding) => CodeError::Padding,
            FrameError::Short => CodeError::Malformed,
            FrameError::Checksum => CodeError::Checksum,
        })?;
        let layout = Layout::from_bytes(&bytes)?;
        if layout.proof.is_some() != proven {
            return Err(CodeError::Malformed);
        }
        Ok(layout)
    }
}

// ------------------------------------------------------------------ body

/// The packed body; `biome` is format 2's field, written as given (the
/// tests write numbers no biome has).
pub(super) fn encode_body(l: &Layout, biome: Option<u32>) -> Vec<u8> {
    let mut b = BitWriter::default();
    let n = l.segments.len();
    b.put(n as u32, COUNT_BITS);
    b.put(l.name.len() as u32, 4);
    for c in l.name.bytes() {
        let i = NAME_CHARSET.iter().position(|&x| x == c).unwrap_or(0);
        b.put(i as u32, 6);
    }
    let curve: Vec<i64> = l.segments.iter().map(|s| s.curve_milli as i64).collect();
    let pitch: Vec<i64> = l.segments.iter().map(|s| s.pitch_milli as i64).collect();
    put_numeric(&mut b, &curve);
    put_numeric(&mut b, &pitch);
    let width: Vec<u32> = l.segments.iter().map(|s| s.half_width_cm as u32).collect();
    let surface: Vec<u32> = l.segments.iter().map(|s| s.surface as u32).collect();
    let flags: Vec<u32> = l.segments.iter().map(|s| s.flags as u32).collect();
    put_runs(&mut b, &width);
    put_runs(&mut b, &surface);
    put_runs(&mut b, &flags);

    b.put(l.checkpoints.len() as u32 - 1, 3);
    let mut prev = None;
    for &c in &l.checkpoints {
        put_gamma(&mut b, prev.map_or(c, |p: u32| c - p - 1));
        prev = Some(c);
    }

    put_gamma(&mut b, l.objects.len() as u32);
    let (mut seg, mut x) = (0u32, 0i64);
    for o in &l.objects {
        put_gamma(&mut b, o.seg - seg);
        put_gamma(&mut b, zigzag(o.x_cm as i64 - x));
        b.put(o.kind as u32, 1);
        (seg, x) = (o.seg, o.x_cm as i64);
    }

    put_gamma(&mut b, l.traffic.len() as u32);
    let (mut z, mut v, mut lane) = (0i64, 0i64, 0i64);
    for c in &l.traffic {
        put_gamma(&mut b, zigzag(c.z0_dm as i64 - z));
        put_gamma(&mut b, zigzag(c.speed_dms as i64 - v));
        put_gamma(&mut b, zigzag(c.lane_cm as i64 - lane));
        (z, v, lane) = (c.z0_dm as i64, c.speed_dms as i64, c.lane_cm as i64);
    }

    match &l.proof {
        None => b.put(0, 1),
        Some(p) => {
            b.put(1, 1);
            for &a in &p.allot_ticks {
                put_gamma(&mut b, a.min(u32::MAX - 1));
            }
            put_gamma(&mut b, p.epoch.min(u32::MAX - 1));
            b.put(p.stamp >> 16, 16);
            b.put(p.stamp & 0xFFFF, 16);
        }
    }
    if let Some(v) = biome {
        b.put(v, BIOME_BITS);
    }
    b.finish()
}

fn decode_body(bytes: &[u8], format: u8) -> Option<Layout> {
    let mut r = BitReader::new(bytes);
    let n = r.get(COUNT_BITS)? as usize;
    if n == 0 || n > MAX_SEGMENTS {
        return None;
    }
    let len = r.get(4)? as usize;
    if len > NAME_MAX {
        return None;
    }
    let mut name = String::with_capacity(len);
    for _ in 0..len {
        name.push(*NAME_CHARSET.get(r.get(6)? as usize)? as char);
    }
    if !valid_name(&name) {
        return None;
    }
    let curve = get_numeric(&mut r, n)?;
    let pitch = get_numeric(&mut r, n)?;
    let width = get_runs(&mut r, n)?;
    let surface = get_runs(&mut r, n)?;
    let flags = get_runs(&mut r, n)?;
    let mut segments = Vec::with_capacity(n);
    for i in 0..n {
        let f = u8::try_from(flags[i]).ok()?;
        if f & !AUTHORED_FLAGS != 0 {
            return None;
        }
        segments.push(Segment {
            curve_milli: i16::try_from(curve[i]).ok()?,
            pitch_milli: i16::try_from(pitch[i]).ok()?,
            half_width_cm: u16::try_from(width[i]).ok()?,
            surface: *Surface::ALL.get(surface[i] as usize)?,
            flags: f,
        });
    }

    let gates = r.get(3)? as usize + 1;
    debug_assert!(gates <= MAX_GATES);
    let mut checkpoints = Vec::with_capacity(gates);
    for k in 0..gates {
        let g = get_gamma(&mut r)? as u64;
        let at = match checkpoints.last() {
            None => g,
            Some(&p) => p as u64 + g + 1,
        };
        if at >= n as u64 || (k == 0 && at == 0) {
            return None;
        }
        checkpoints.push(at as u32);
    }

    let count = get_gamma(&mut r)? as usize;
    if count > MAX_OBJECTS {
        return None;
    }
    let mut objects = Vec::with_capacity(count);
    let (mut seg, mut x) = (0u64, 0i64);
    for _ in 0..count {
        seg += get_gamma(&mut r)? as u64;
        x += unzigzag(get_gamma(&mut r)?);
        if seg >= n as u64 {
            return None;
        }
        let kind = match r.get(1)? {
            0 => ObjectKind::Post,
            _ => ObjectKind::Rock,
        };
        objects.push(Object {
            seg: seg as u32,
            x_cm: i16::try_from(x).ok()?,
            kind,
        });
    }

    let count = get_gamma(&mut r)? as usize;
    if count > MAX_TRAFFIC {
        return None;
    }
    let mut traffic = Vec::with_capacity(count);
    let (mut z, mut v, mut lane) = (0i64, 0i64, 0i64);
    for _ in 0..count {
        z += unzigzag(get_gamma(&mut r)?);
        v += unzigzag(get_gamma(&mut r)?);
        lane += unzigzag(get_gamma(&mut r)?);
        traffic.push(TrafficCar {
            z0_dm: u32::try_from(z).ok()?,
            speed_dms: u16::try_from(v).ok()?,
            lane_cm: i16::try_from(lane).ok()?,
        });
    }

    let proof = match r.get(1)? {
        0 => None,
        _ => {
            let allot_ticks = (0..gates)
                .map(|_| get_gamma(&mut r))
                .collect::<Option<Vec<u32>>>()?;
            let epoch = get_gamma(&mut r)?;
            let stamp = (r.get(16)? << 16) | r.get(16)?;
            Some(Proof {
                allot_ticks,
                epoch,
                stamp,
            })
        }
    };
    let biome = if format == LAYOUT_FORMAT {
        // One spelling per layout: a meadow is format 1.
        match Biome::from_code(r.get(BIOME_BITS)? as u8)? {
            Biome::Meadow => return None,
            b => b,
        }
    } else {
        Biome::Meadow
    };
    if !r.rest_is_padding() {
        return None;
    }
    let layout = Layout {
        name,
        segments,
        checkpoints,
        objects,
        traffic,
        proof,
        biome,
    };
    layout.structure().ok()?;
    Some(layout)
}

// -------------------------------------------------------------- channels

/// Differences of `order` 1 or 2 after the first value, zigzagged. A
/// second difference starts from a slope of zero.
fn residuals(v: &[i64], order: u32) -> Vec<u32> {
    let mut out = Vec::with_capacity(v.len().saturating_sub(1));
    let mut slope = 0;
    for w in v.windows(2) {
        let d = w[1] - w[0];
        out.push(if order == 1 {
            zigzag(d)
        } else {
            zigzag(d - slope)
        });
        slope = d;
    }
    out
}

/// A numeric channel: the first value, then the shorter of its first and
/// second differences.
fn put_numeric(b: &mut BitWriter, v: &[i64]) {
    put_gamma(b, zigzag(v[0]));
    let bits = |z: &[u32]| {
        let mut t = BitWriter::default();
        put_residuals(&mut t, z);
        t.bits()
    };
    let (first, second) = (residuals(v, 1), residuals(v, 2));
    if bits(&second) < bits(&first) {
        b.put(1, 1);
        put_residuals(b, &second);
    } else {
        b.put(0, 1);
        put_residuals(b, &first);
    }
}

/// Read `n` values written by [`put_numeric`].
fn get_numeric(r: &mut BitReader, n: usize) -> Option<Vec<i64>> {
    let mut out = Vec::with_capacity(n);
    out.push(unzigzag(get_gamma(r)?));
    let second = r.get(1)? == 1;
    let mut slope = 0i64;
    for z in get_residuals(r, n - 1)? {
        let d = if second {
            slope + unzigzag(z)
        } else {
            unzigzag(z)
        };
        // Keep the running sums small: a value past i16 is malformed
        // anyway, and this stops an overflow on a hostile code.
        if d.abs() > 1 << 20 {
            return None;
        }
        let v = *out.last()? + d;
        if v.abs() > 1 << 20 {
            return None;
        }
        out.push(v);
        slope = d;
    }
    Some(out)
}

/// The residuals as zero runs and the values between them: a run of
/// zeros, then (unless that run reached the end) one non-zero value, less
/// one, and again. Salt Line's `layout/code.rs`.
fn runs_and_values(z: &[u32]) -> (Vec<u32>, Vec<u32>) {
    let (mut runs, mut values) = (Vec::new(), Vec::new());
    let mut i = 0;
    while i < z.len() {
        let run = z[i..].iter().take_while(|&&v| v == 0).count();
        runs.push(run as u32);
        i += run;
        if let Some(&v) = z.get(i) {
            values.push(v - 1);
            i += 1;
        }
    }
    (runs, values)
}

/// Write residuals the shorter of two ways, after one bit saying which:
/// each Rice-coded at the best `k`, or as zero runs and the values between
/// them, each with its own best `k`.
fn put_residuals(b: &mut BitWriter, z: &[u32]) {
    let (k, plain) = best_k(z);
    let (runs, values) = runs_and_values(z);
    let ((run_k, run_bits), (value_k, value_bits)) = (best_k(&runs), best_k(&values));
    if run_bits + value_bits + 4 < plain {
        b.put(1, 1);
        b.put(run_k, 4);
        b.put(value_k, 4);
        for (i, &run) in runs.iter().enumerate() {
            put_rice(b, run, run_k);
            if let Some(&v) = values.get(i) {
                put_rice(b, v, value_k);
            }
        }
    } else {
        b.put(0, 1);
        b.put(k, 4);
        for &v in z {
            put_rice(b, v, k);
        }
    }
}

/// Read `n` residuals written by [`put_residuals`].
fn get_residuals(r: &mut BitReader, n: usize) -> Option<Vec<u32>> {
    let mut out = Vec::with_capacity(n);
    if r.get(1)? == 0 {
        let k = r.get(4)?;
        for _ in 0..n {
            out.push(get_rice(r, k)?);
        }
        return Some(out);
    }
    let (run_k, value_k) = (r.get(4)?, r.get(4)?);
    while out.len() < n {
        let run = get_rice(r, run_k)? as usize;
        if run > n - out.len() {
            return None;
        }
        out.resize(out.len() + run, 0);
        if out.len() < n {
            out.push(get_rice(r, value_k)?.checked_add(1)?);
        }
    }
    Some(out)
}

/// A runs channel: run count − 1, then each run's value and (but the
/// last's) its length − 1.
fn put_runs(b: &mut BitWriter, v: &[u32]) {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for &x in v {
        match runs.last_mut() {
            Some((value, len)) if *value == x => *len += 1,
            _ => runs.push((x, 1)),
        }
    }
    put_gamma(b, runs.len() as u32 - 1);
    let last = runs.len() - 1;
    for (i, &(value, len)) in runs.iter().enumerate() {
        put_gamma(b, value);
        if i < last {
            put_gamma(b, len - 1);
        }
    }
}

/// Read `n` values written by [`put_runs`]. Neighbouring runs must differ.
fn get_runs(r: &mut BitReader, n: usize) -> Option<Vec<u32>> {
    let count = get_gamma(r)? as usize + 1;
    if count > n {
        return None;
    }
    let mut out = Vec::with_capacity(n);
    let mut prev = None;
    for i in 0..count {
        let value = get_gamma(r)?;
        if prev == Some(value) {
            return None;
        }
        prev = Some(value);
        let len = if i + 1 < count {
            get_gamma(r)? as usize + 1
        } else {
            n.checked_sub(out.len())?
        };
        if len == 0 || out.len() + len > n || (i + 1 < count && out.len() + len == n) {
            return None;
        }
        out.resize(out.len() + len, value);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{base32, crc16};

    #[test]
    fn another_format_is_named_not_guessed() {
        let mut bytes = vec![LAYOUT_FORMAT + 1, 0, 0];
        let crc = crc16(&bytes);
        bytes.extend(crc.to_be_bytes());
        let code = format!("{DRAFT_PREFIX}{}", base32(&bytes));
        assert_eq!(
            Layout::from_code(&code),
            Err(CodeError::Format(LAYOUT_FORMAT + 1))
        );
        assert_eq!(Layout::from_bytes(&[]), Err(CodeError::Malformed));
    }

    #[test]
    fn runs_read_back_and_refuse_neighbours_alike() {
        for v in [vec![600], vec![600, 600, 450, 450, 600], vec![0, 1, 2, 3]] {
            let mut b = BitWriter::default();
            put_runs(&mut b, &v);
            let bytes = b.finish();
            assert_eq!(get_runs(&mut BitReader::new(&bytes), v.len()), Some(v));
        }
        // Two runs of 5: count 1, value 5, length 0, value 5.
        let mut b = BitWriter::default();
        for x in [1, 5, 0, 5] {
            put_gamma(&mut b, x);
        }
        assert_eq!(get_runs(&mut BitReader::new(&b.finish()), 2), None);
    }

    #[test]
    fn numeric_channels_read_back_either_order() {
        let ramp: Vec<i64> = (0..50).map(|i| i * 3 - 70).collect();
        let bend: Vec<i64> = (0..60)
            .map(|i| if (10..50).contains(&i) { 120 } else { 0 })
            .collect();
        for v in [ramp, bend, vec![-32768], vec![32767, -32768, 32767]] {
            let mut b = BitWriter::default();
            put_numeric(&mut b, &v);
            let bytes = b.finish();
            assert_eq!(get_numeric(&mut BitReader::new(&bytes), v.len()), Some(v));
        }
    }
}
