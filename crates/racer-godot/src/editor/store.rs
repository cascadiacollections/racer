//! The layouts a player keeps: the slots file and the draft (DESIGN
//! §9.11), Salt Line's rules (its §13.8). Engine-free text in, text out;
//! the adapter reads and writes the files (`user://layouts/`), each
//! replaced atomically through a temporary file and a rename.
//!
//! **Slots**, [`SLOTS_FILE`]: a `racer-layouts 1` header, then one line per
//! kept layout, `slot <n> <code> [origin <text>]`, `n` from 1 to
//! [`MAX_SLOTS`]. Plain text the player owns: a layout is already a line of
//! text. A line this build cannot read (another tag, a slot out of range or
//! taken twice) is kept verbatim and written back after the slots; a code
//! it cannot decode (a newer `LAYOUT_FORMAT`) keeps its slot, which is
//! never saved over or deleted. A file from a newer build, or one that
//! cannot be read at all (too large, NULs in it), is never written.
//!
//! **The draft**, [`DRAFT_FILE`]: the layout being edited, written after
//! every edit and cleared once saved or discarded:
//!
//! ```text
//! racer-draft 1
//! slot 3            (or `slot -`: not saved yet)
//! origin fork:…     (or `origin -`)
//! code RD…          (RL when the draft is proven)
//! ```
//!
//! A draft from a newer build is kept unread and never written over; a
//! damaged one is dropped with a note.

use racer_sim::layout::Layout;

/// The slots file's name, in `user://layouts`.
pub const SLOTS_FILE: &str = "layouts.txt";
/// The draft file's name, in `user://layouts`.
pub const DRAFT_FILE: &str = "draft.txt";
/// The slots file's format, its header's second token.
pub const SLOTS_FORMAT: u32 = 1;
/// The draft file's format.
pub const DRAFT_FORMAT: u32 = 1;
/// Slots, numbered from 1.
pub const MAX_SLOTS: u32 = 24;
/// A file longer than this is not read, and never written over.
pub const MAX_FILE: usize = 1 << 20;

/// Which file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreFile {
    /// [`SLOTS_FILE`].
    Slots,
    /// [`DRAFT_FILE`].
    Draft,
}

impl StoreFile {
    /// Its file name.
    pub fn name(self) -> &'static str {
        match self {
            StoreFile::Slots => SLOTS_FILE,
            StoreFile::Draft => DRAFT_FILE,
        }
    }
}

/// One kept layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    /// 1 to [`MAX_SLOTS`].
    pub n: u32,
    /// The code as written.
    pub code: String,
    /// Where it came from, verbatim.
    pub origin: Option<String>,
    /// The layout, if this build reads the code.
    pub layout: Option<Layout>,
}

/// The slots file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slots {
    slots: Vec<Slot>,
    /// Lines this build does not read, written back as they were.
    kept: Vec<String>,
    /// Slot numbers named by kept lines: never offered as free.
    held: Vec<u32>,
    /// Whether this build may write the file.
    pub writable: bool,
    /// What the player should be told about the file, if anything.
    pub note: Option<&'static str>,
}

impl Default for Slots {
    fn default() -> Self {
        Slots {
            slots: Vec::new(),
            kept: Vec::new(),
            held: Vec::new(),
            writable: true,
            note: None,
        }
    }
}

/// Whether `text` can be read at all.
fn readable(text: &str) -> bool {
    text.len() <= MAX_FILE && !text.contains('\0')
}

/// `<tag> <n>`: the format number of a header line.
fn header(line: Option<&str>, tag: &str) -> Option<u32> {
    line?
        .trim()
        .strip_prefix(tag)?
        .strip_prefix(' ')?
        .parse()
        .ok()
}

impl Slots {
    /// Read the slots file's text; `None` is no file.
    pub fn decode(text: Option<&str>) -> Slots {
        let Some(text) = text else {
            return Slots::default();
        };
        if !readable(text) {
            return Slots {
                writable: false,
                note: Some("SAVED LAYOUTS UNREADABLE: KEPT"),
                ..Slots::default()
            };
        }
        let mut lines = text.lines();
        let mut s = Slots::default();
        match header(lines.next(), "racer-layouts") {
            Some(SLOTS_FORMAT) => {}
            Some(v) if v > SLOTS_FORMAT => {
                return Slots {
                    writable: false,
                    note: Some("SAVED LAYOUTS FROM A NEWER BUILD"),
                    ..Slots::default()
                };
            }
            _ => {
                // No header we know: keep every line, read none.
                s.kept = text.lines().map(str::to_string).collect();
                s.kept.retain(|l| !l.trim().is_empty());
                s.note = (!s.kept.is_empty()).then_some("SAVED LAYOUTS DAMAGED: KEPT");
                return s;
            }
        }
        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            match parse_slot(line) {
                Some(slot) if !s.taken(slot.n) => s.slots.push(slot),
                parsed => {
                    if let Some(n) = parsed.map(|p| p.n).or_else(|| slot_number(line)) {
                        s.held.push(n);
                    }
                    s.kept.push(line.to_string());
                }
            }
        }
        s.slots.sort_by_key(|x| x.n);
        s
    }

    /// The file's text, or `None` when this build must not write it.
    pub fn encode(&self) -> Option<String> {
        if !self.writable {
            return None;
        }
        let mut out = format!("racer-layouts {SLOTS_FORMAT}\n");
        for s in &self.slots {
            out.push_str(&format!("slot {} {}", s.n, s.code));
            if let Some(o) = &s.origin {
                out.push_str(&format!(" origin {o}"));
            }
            out.push('\n');
        }
        for k in &self.kept {
            out.push_str(k);
            out.push('\n');
        }
        Some(out)
    }

    /// Every slot, in order.
    pub fn slots(&self) -> &[Slot] {
        &self.slots
    }

    /// Slot `n`.
    pub fn get(&self, n: u32) -> Option<&Slot> {
        self.slots.iter().find(|s| s.n == n)
    }

    /// Whether slot `n` holds anything, read or not.
    pub fn taken(&self, n: u32) -> bool {
        self.get(n).is_some() || self.held.contains(&n)
    }

    /// The lowest free slot.
    pub fn free(&self) -> Option<u32> {
        (1..=MAX_SLOTS).find(|&n| !self.taken(n))
    }

    /// Whether slot `n` may be saved over or deleted: in range, writable,
    /// and not holding something this build cannot read.
    pub fn editable(&self, n: u32) -> bool {
        self.writable
            && (1..=MAX_SLOTS).contains(&n)
            && !self.held.contains(&n)
            && self.get(n).is_none_or(|s| s.layout.is_some())
    }

    /// Keep `layout` in slot `n`. Refused (false) unless
    /// [`Slots::editable`] or the layout cannot be written as a code.
    pub fn put(&mut self, n: u32, layout: &Layout, origin: Option<String>) -> bool {
        if !self.editable(n) {
            return false;
        }
        let Ok(code) = layout.to_code() else {
            return false;
        };
        self.slots.retain(|s| s.n != n);
        self.slots.push(Slot {
            n,
            code,
            origin,
            layout: Some(layout.clone()),
        });
        self.slots.sort_by_key(|s| s.n);
        true
    }

    /// Empty slot `n`. Refused unless it holds a layout this build reads.
    pub fn remove(&mut self, n: u32) -> bool {
        if !self.editable(n) || self.get(n).is_none() {
            return false;
        }
        self.slots.retain(|s| s.n != n);
        true
    }
}

/// The slot number a line names, if it is a `slot` line at all.
fn slot_number(line: &str) -> Option<u32> {
    let mut t = line.split_whitespace();
    (t.next()? == "slot").then_some(())?;
    t.next()?.parse().ok()
}

/// A `slot <n> <code> [origin <text>]` line.
fn parse_slot(line: &str) -> Option<Slot> {
    let mut t = line.split_whitespace();
    if t.next()? != "slot" {
        return None;
    }
    let n: u32 = t.next()?.parse().ok()?;
    if !(1..=MAX_SLOTS).contains(&n) {
        return None;
    }
    let code = t.next()?.to_string();
    let origin = match t.next() {
        None => None,
        Some("origin") => Some(t.collect::<Vec<_>>().join(" ")).filter(|o| !o.is_empty()),
        Some(_) => return None,
    };
    if !racer_sim::layout::CODE_PREFIXES
        .iter()
        .any(|p| code.starts_with(p))
    {
        return None;
    }
    let layout = Layout::from_code(&code).ok();
    Some(Slot {
        n,
        code,
        origin,
        layout,
    })
}

/// The draft as kept on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftFile {
    /// The layout.
    pub layout: Layout,
    /// The slot it was last saved to, if any.
    pub slot: Option<u32>,
    /// Where it came from.
    pub origin: Option<String>,
}

/// What reading the draft file found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftRead {
    /// No draft.
    None,
    /// A draft.
    Draft(DraftFile),
    /// A draft from a newer build: kept unread, never written over.
    Newer,
    /// Damaged: dropped.
    Damaged,
}

impl DraftFile {
    /// The file's text.
    pub fn encode(&self) -> Option<String> {
        let code = self.layout.to_code().ok()?;
        Some(format!(
            "racer-draft {DRAFT_FORMAT}\nslot {}\norigin {}\ncode {code}\n",
            self.slot.map_or("-".to_string(), |n| n.to_string()),
            self.origin.as_deref().unwrap_or("-"),
        ))
    }

    /// Read the draft file's text; `None` is no file.
    pub fn decode(text: Option<&str>) -> DraftRead {
        let Some(text) = text else {
            return DraftRead::None;
        };
        if text.trim().is_empty() {
            return DraftRead::None;
        }
        if !readable(text) {
            return DraftRead::Damaged;
        }
        let mut lines = text.lines();
        match header(lines.next(), "racer-draft") {
            Some(DRAFT_FORMAT) => {}
            Some(v) if v > DRAFT_FORMAT => return DraftRead::Newer,
            _ => return DraftRead::Damaged,
        }
        let mut field = |key: &str| -> Option<String> {
            Some(
                lines
                    .next()?
                    .strip_prefix(key)?
                    .strip_prefix(' ')?
                    .trim()
                    .to_string(),
            )
        };
        let (Some(slot), Some(origin), Some(code)) =
            (field("slot"), field("origin"), field("code"))
        else {
            return DraftRead::Damaged;
        };
        let slot = match slot.as_str() {
            "-" => None,
            n => match n.parse::<u32>() {
                Ok(n) if (1..=MAX_SLOTS).contains(&n) => Some(n),
                _ => return DraftRead::Damaged,
            },
        };
        let origin = (origin != "-" && !origin.is_empty()).then_some(origin);
        match Layout::from_code(&code) {
            Ok(layout) => DraftRead::Draft(DraftFile {
                layout,
                slot,
                origin,
            }),
            // A code format this build does not read is a newer build's.
            Err(racer_sim::layout::CodeError::Format(_)) => DraftRead::Newer,
            Err(_) => DraftRead::Damaged,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use racer_sim::layout::bundled;

    /// `l`'s code as a later `LAYOUT_FORMAT` would frame it.
    fn newer_code(l: &Layout) -> String {
        let mut bytes = l.to_bytes().unwrap();
        bytes[0] = racer_sim::layout::LAYOUT_FORMAT + 1;
        racer_sim::codec::frame("RL", bytes)
    }

    fn two() -> (Layout, Layout) {
        let mut l = bundled::layouts();
        (l.remove(0), l.remove(0).unproven())
    }

    #[test]
    fn slots_round_trip_in_order_with_their_origins() {
        let (a, b) = two();
        let mut s = Slots::decode(None);
        assert_eq!(s.free(), Some(1));
        assert!(s.put(3, &a, Some("fork:0x0000000000000003:0:0:e1".into())));
        assert!(s.put(1, &b, None));
        assert_eq!(s.free(), Some(2));
        let text = s.encode().unwrap();
        assert!(text.starts_with("racer-layouts 1\nslot 1 RD"), "{text}");
        assert!(text.contains("\nslot 3 RL"));
        assert!(text.ends_with(" origin fork:0x0000000000000003:0:0:e1\n"));
        let back = Slots::decode(Some(&text));
        assert_eq!(back, s);
        assert_eq!(back.get(3).unwrap().layout.as_ref(), Some(&a));
        // Overwrite and delete.
        let mut s = back;
        assert!(s.put(3, &b, None));
        assert_eq!(s.get(3).unwrap().origin, None);
        assert!(s.remove(1));
        assert!(!s.remove(1), "already empty");
        assert!(!s.put(0, &a, None) && !s.put(MAX_SLOTS + 1, &a, None));
    }

    #[test]
    fn lines_it_cannot_read_are_kept_and_their_slots_never_offered() {
        let (a, _) = two();
        let code = a.to_code().unwrap();
        // A newer code format: a format byte this build lacks.
        let newer = format!("slot 2 {} origin later:kind", newer_code(&a));
        let text = format!(
            "racer-layouts 1\nslot 1 {code}\n{newer}\nslot 1 {code}\nmap 9 something\nslot 99 {code}\n\nslot 4 garbage\n"
        );
        let mut s = Slots::decode(Some(&text));
        assert_eq!(s.slots().len(), 2, "{s:?}");
        assert!(
            s.get(2).unwrap().layout.is_none(),
            "unreadable, its slot kept"
        );
        assert!(!s.editable(2) && !s.put(2, &a, None) && !s.remove(2));
        assert!(s.get(4).is_none() && s.taken(4) && !s.editable(4));
        assert_eq!(s.free(), Some(3), "1, 2 and 4 are taken");
        // The doubled slot 1 is held: never offered, never written over.
        assert!(!s.editable(1));
        let out = s.encode().unwrap();
        for kept in [
            &format!("slot 1 {code}"),
            "map 9 something",
            &format!("slot 99 {code}"),
        ] {
            assert!(out.contains(kept), "{kept} in {out}");
        }
        assert!(out.contains(&newer));
        // Written and read again, nothing is lost or reordered.
        assert_eq!(Slots::decode(Some(&out)).encode().unwrap(), out);
        assert!(s.put(3, &a, None));
    }

    #[test]
    fn a_newer_or_unreadable_file_is_never_written() {
        for text in ["racer-layouts 2\nslot 1 RLX\n", "racer-layouts 1\n\0\0\0"] {
            let s = Slots::decode(Some(text));
            assert!(!s.writable && s.encode().is_none(), "{text:?}");
            assert!(s.note.is_some());
            assert_eq!(s.free(), Some(1));
            assert!(!s.editable(1));
        }
        let big = "x".repeat(MAX_FILE + 1);
        assert!(!Slots::decode(Some(&big)).writable);
        // A file without our header keeps every line under a new one.
        let s = Slots::decode(Some("garbage\nslot 1 RL\n"));
        assert!(s.writable && s.note.is_some());
        assert_eq!(s.encode().unwrap(), "racer-layouts 1\ngarbage\nslot 1 RL\n");
        assert_eq!(Slots::decode(Some("")), Slots::default());
    }

    #[test]
    fn the_draft_round_trips_and_damage_or_newer_builds_are_told_apart() {
        let (a, b) = two();
        for d in [
            DraftFile {
                layout: a.clone(),
                slot: Some(5),
                origin: Some("fork:0x0000000000000003:0:0:e1".into()),
            },
            DraftFile {
                layout: b.clone(),
                slot: None,
                origin: None,
            },
        ] {
            let text = d.encode().unwrap();
            assert!(text.starts_with("racer-draft 1\nslot "));
            assert_eq!(DraftFile::decode(Some(&text)), DraftRead::Draft(d));
        }
        let good = DraftFile {
            layout: b,
            slot: None,
            origin: None,
        }
        .encode()
        .unwrap();
        assert_eq!(DraftFile::decode(None), DraftRead::None);
        assert_eq!(DraftFile::decode(Some("  \n")), DraftRead::None);
        assert_eq!(
            DraftFile::decode(Some("racer-draft 2\nwhatever\n")),
            DraftRead::Newer
        );
        // Torn at every length: never a panic, never a wrong draft.
        for cut in 0..good.len() {
            match DraftFile::decode(Some(&good[..cut])) {
                DraftRead::Draft(d) => assert_eq!(Some(good.clone()), d.encode()),
                DraftRead::None | DraftRead::Damaged => {}
                DraftRead::Newer => panic!("cut {cut}"),
            }
        }
        for bad in [
            good.replace("slot -", "slot 99"),
            good.replace("slot -", "slot x"),
            good.replace("code RD", "code RX"),
            good.replace("racer-draft 1", "racer-draft x"),
            format!("{good}\0"),
        ] {
            assert_eq!(DraftFile::decode(Some(&bad)), DraftRead::Damaged, "{bad}");
        }
        let code_at = good.find("code ").unwrap() + 5;
        let newer = format!("{}{}\n", &good[..code_at], newer_code(&a));
        assert_eq!(DraftFile::decode(Some(&newer)), DraftRead::Newer);
    }
}
