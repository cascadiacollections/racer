//! The track editor and the TRACKS list (DESIGN §9.11). Engine-free.
//!
//! ```text
//!  title ─TRACKS─► Tracks ─EDITOR─► Edit ◄──► Menu ─► Save / Name / Seed
//!                   │  ▲                │ TEST DRIVE      │ FROM SEED
//!                   │  └──── TRACKS ────┘                 ▼
//!                   └ RACE ► custom race     builder thread: the fork's
//!  pause / results ─FORK THIS TRACK─► Edit   track, PROVE, VERIFY
//! ```
//!
//! - [`model`]: the draft, its runs, every edit, undo and redo.
//! - [`proof`]: PROVE and VERIFY on the builder thread, and their words.
//! - [`store`]: the slots file and the draft on disk.
//! - [`clip`]: codes on the clipboard.
//! - [`view`]: where everything is on screen and what a touch means.
//! - [`draw`]: the pages, painted.
//!
//! [`Editor`] is driven by [`Act`]s (buttons, and keys and pad buttons
//! mapped to them by [`Editor::key`]) and answers with [`Out`]s for the
//! game to carry out: files to write, the clipboard, a race to start. It
//! never blocks: proving and forking go to the builder thread. The DEMO
//! never reaches it (the demo starts from the title and any input ends
//! it), so the demo never reads or writes a layout file.

pub mod clip;
pub mod draw;
pub mod model;
pub mod proof;
pub mod store;
pub mod view;

use std::collections::BTreeMap;

use racer_sim::layout::{bundled, Layout, Provenance, Standing};
use racer_sim::track::{Surface, Track};

use crate::builder::{Build, Key, Proved};
use crate::custom::Custom;
use crate::input::{bind, pressed, Event, Key as K, Pad};
use crate::layout::Layout as Screen;
use model::{Draft, DriftMark, Edit, History, Side};
use proof::{Outcome, ProofState, Prover};
use store::{DraftFile, DraftRead, Slots, StoreFile, MAX_SLOTS};
pub use view::{Act, Item, Tool};

/// How long a note stays up, seconds.
pub const NOTE_SECS: f64 = 3.0;
/// Rows the TRACKS list shows a page.
pub const LIST_ROWS: usize = 9;
/// Job numbers for checking listed layouts' proofs: apart from the
/// editor's own.
const LIST_JOBS: u64 = 1 << 62;

/// An editor page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// The TRACKS list: bundled courses and slots.
    Tracks,
    /// Editing: map, strip, tools.
    Edit,
    /// The editor's menu.
    Menu,
    /// Save to a slot.
    Save,
    /// Name the layout.
    Name,
    /// Fork a generated track by seed.
    Seed,
}

/// Something the game must do for the editor.
#[derive(Clone, Debug)]
pub enum Out {
    /// Write a file (`None`: delete it).
    Store(StoreFile, Option<String>),
    /// Put text on the clipboard.
    Copy(String),
    /// Read the clipboard and hand it to [`Editor::pasted`].
    Paste,
    /// Start a custom race.
    Race(Box<Custom>),
    /// Back to the title.
    Title,
    /// Build this generated track for FROM SEED; hand it to
    /// [`Editor::forked`] when it is built.
    Build(Key),
    /// A click.
    Click,
}

/// One row of the TRACKS list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    /// Bundled course `i`.
    Course(usize),
    /// Slot `n`.
    Slot(u32),
}

/// A two-tap confirmation in waiting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Armed {
    /// Save over slot `n`.
    Overwrite(u32),
    /// Delete slot `n`.
    Delete(u32),
}

/// The editor.
pub struct Editor {
    /// The page shown.
    pub page: Page,
    /// The draft.
    pub draft: Draft,
    /// Undo and redo.
    pub history: History,
    /// Where the draft's proof stands.
    pub prover: Prover,
    /// The selected run.
    pub sel: usize,
    /// The tool in hand.
    pub tool: Tool,
    /// What a key or pad has focused: an option, a menu item, a slot.
    pub focus: usize,
    /// Where the draft came from.
    pub origin: Option<String>,
    /// The slot the draft was last saved to.
    pub slot: Option<u32>,
    /// The slots file.
    pub slots: Slots,
    /// The bundled courses (proven again here if their stamps are stale).
    pub courses: Vec<Layout>,
    /// A message and the clock time it goes at.
    note: Option<(String, f64)>,
    /// A note that stays until the draft changes.
    pub sticky: Option<String>,
    /// A newer build's draft is on disk: never write the draft file.
    draft_locked: bool,
    /// The name being typed.
    pub name_buf: String,
    /// The seed being typed.
    pub seed_buf: String,
    /// FROM SEED's track index, from 0.
    pub seed_index: u32,
    /// FROM SEED's road.
    pub seed_branch: u8,
    /// FROM SEED waiting for the builder.
    pub pending_fork: Option<Key>,
    /// The TRACKS list's selected row.
    pub list_sel: usize,
    /// The TRACKS list's page.
    pub list_page: usize,
    /// Listed layouts whose proofs are being checked: job to entry.
    checking: BTreeMap<u64, Entry>,
    next_check: u64,
    /// Listed layouts whose proofs did not hold.
    pub failed: Vec<Entry>,
    /// The page to go back to from Name, Seed and Save.
    back_to: Page,
    /// A two-tap confirmation in waiting.
    pub armed: Option<Armed>,
    /// The clock, seconds, as of the last call.
    now: f64,
}

impl Default for Editor {
    fn default() -> Self {
        Editor::new()
    }
}

impl Editor {
    /// An editor with a blank draft and no slots.
    pub fn new() -> Editor {
        Editor {
            page: Page::Tracks,
            draft: Draft::blank(),
            history: History::default(),
            prover: Prover::default(),
            sel: 0,
            tool: Tool::Bend,
            focus: 0,
            origin: None,
            slot: None,
            slots: Slots::default(),
            courses: bundled::layouts(),
            note: None,
            sticky: None,
            draft_locked: false,
            name_buf: String::new(),
            seed_buf: String::new(),
            seed_index: 0,
            seed_branch: 0,
            pending_fork: None,
            list_sel: 0,
            list_page: 0,
            checking: BTreeMap::new(),
            next_check: 0,
            failed: Vec::new(),
            back_to: Page::Tracks,
            armed: None,
            now: 0.0,
        }
    }

    /// Read the files at launch: the slots file's text and the draft's
    /// (`None` where there is no file). A draft saved by this build trusts
    /// its stamp; one that does not read is dropped with a note; one from a
    /// newer build is kept on disk, never written over.
    pub fn load(&mut self, slots: Option<&str>, draft: Option<&str>) {
        self.slots = Slots::decode(slots);
        self.sticky = self.slots.note.map(str::to_string);
        match DraftFile::decode(draft) {
            DraftRead::None => {}
            DraftRead::Draft(d) => {
                let layout = if d.layout.standing() == Standing::Proven {
                    d.layout
                } else {
                    // Not this build's proof: keep the road, drop the proof.
                    d.layout.unproven()
                };
                self.prover = Prover::loaded(&layout, true).0;
                self.draft = Draft::new(layout);
                self.origin = d.origin;
                self.slot = d.slot;
            }
            DraftRead::Newer => {
                self.draft_locked = true;
                self.sticky = Some("A NEWER BUILD'S DRAFT IS KEPT".into());
            }
            DraftRead::Damaged => {
                self.sticky = Some("THE DRAFT WAS DAMAGED: DROPPED".into());
            }
        }
    }

    /// The note up now, if any.
    pub fn note(&self) -> Option<&str> {
        match &self.note {
            Some((n, until)) if *until > self.now => Some(n),
            _ => self.sticky.as_deref(),
        }
    }

    fn say(&mut self, text: impl Into<String>) {
        self.note = Some((text.into(), self.now + NOTE_SECS));
    }

    /// Set the clock (seconds), for notes.
    pub fn tick(&mut self, now: f64) {
        self.now = now;
    }

    /// Whether the page changes by itself: work under way, or a note that
    /// will go.
    pub fn animating(&self) -> bool {
        self.busy() || self.note.as_ref().is_some_and(|n| n.1 > self.now)
    }

    /// Whether anything is being worked on.
    pub fn busy(&self) -> bool {
        self.prover.busy() || self.pending_fork.is_some() || !self.checking.is_empty()
    }

    /// Open the TRACKS list, checking any listed proof this build does not
    /// trust (a stale or bad stamp) on the builder thread.
    pub fn open_tracks(&mut self, build: &mut dyn Build) {
        self.page = Page::Tracks;
        self.armed = None;
        self.focus = 0;
        for e in self.entries() {
            let Some(l) = self.entry_layout(e).cloned() else {
                continue;
            };
            let needs = matches!(l.standing(), Standing::Stale | Standing::BadStamp);
            if needs && !self.checking(e) && !self.failed.contains(&e) {
                let job = LIST_JOBS + self.next_check;
                self.next_check += 1;
                build.prove(job, l.unproven());
                self.checking.insert(job, e);
            }
        }
    }

    /// Open the editing page on the draft.
    pub fn open_edit(&mut self) {
        self.page = Page::Edit;
        self.armed = None;
        self.sel = self.sel.min(self.draft.run_count() - 1);
    }

    /// The rows of the TRACKS list: the courses, then the slots.
    pub fn entries(&self) -> Vec<Entry> {
        (0..self.courses.len())
            .map(Entry::Course)
            .chain(self.slots.slots().iter().map(|s| Entry::Slot(s.n)))
            .collect()
    }

    /// The layout a row holds, if this build reads it.
    pub fn entry_layout(&self, e: Entry) -> Option<&Layout> {
        match e {
            Entry::Course(i) => self.courses.get(i),
            Entry::Slot(n) => self.slots.get(n)?.layout.as_ref(),
        }
    }

    /// Whether a row's proof is being checked.
    pub fn checking(&self, e: Entry) -> bool {
        self.checking.values().any(|c| *c == e)
    }

    /// The draft file as it should now be, or nothing to write.
    fn save_draft(&self) -> Option<Out> {
        if self.draft_locked {
            return None;
        }
        let d = DraftFile {
            layout: self.draft.layout.clone(),
            slot: self.slot,
            origin: self.origin.clone(),
        };
        d.encode().map(|t| Out::Store(StoreFile::Draft, Some(t)))
    }

    fn save_slots(&self) -> Option<Out> {
        self.slots
            .encode()
            .map(|t| Out::Store(StoreFile::Slots, Some(t)))
    }

    /// Replace the draft (a fork, a paste, a layout opened, a blank): one
    /// undoable step. `trusted`: its stamp may be believed (bundled, or
    /// saved by this build); otherwise a proof it carries is checked.
    pub fn replace(
        &mut self,
        layout: Layout,
        origin: Option<String>,
        slot: Option<u32>,
        trusted: bool,
        build: &mut dyn Build,
    ) -> Vec<Out> {
        self.history.replace(&mut self.draft, Draft::new(layout));
        self.origin = origin;
        self.slot = slot;
        self.sel = 0;
        self.sticky = None;
        let (p, needs) = Prover::loaded(&self.draft.layout, trusted);
        self.prover = p;
        if needs {
            self.prover.verify(&self.draft.layout, build);
        }
        self.page = Page::Edit;
        self.armed = None;
        self.save_draft().into_iter().collect()
    }

    /// FORK THIS TRACK: `track` (the one just raced) as the draft.
    pub fn fork_track(
        &mut self,
        track: &Track,
        origin: Option<String>,
        build: &mut dyn Build,
    ) -> Vec<Out> {
        let l = Layout::from_track(track);
        let mut out = self.replace(l, origin, None, false, build);
        self.say("FORKED: EDIT IT, THEN PROVE IT");
        out.push(Out::Click);
        out
    }

    /// FORK THIS TRACK on a custom race: its layout itself.
    pub fn fork_layout(&mut self, c: &Custom, build: &mut dyn Build) -> Vec<Out> {
        let mut out = self.replace(c.layout.clone(), c.origin.clone(), None, true, build);
        self.say("FORKED: EDIT IT, THEN PROVE IT");
        out.push(Out::Click);
        out
    }

    /// FROM SEED's track is built.
    pub fn forked(&mut self, key: Key, track: &Track, build: &mut dyn Build) -> Vec<Out> {
        if self.pending_fork != Some(key) {
            return Vec::new();
        }
        self.pending_fork = None;
        let origin = Provenance {
            run_seed: key.0,
            index: key.1,
            branch: key.2,
            epoch: racer_sim::SIM_EPOCH,
        };
        self.fork_track(track, Some(origin.to_string()), build)
    }

    /// The clipboard's text, after [`Out::Paste`].
    pub fn pasted(&mut self, text: &str, build: &mut dyn Build) -> Vec<Out> {
        use racer_sim::layout::CodeError;
        match clip::paste(text) {
            Ok(p) => {
                let mut out = self.replace(p.layout, p.origin, None, false, build);
                self.say(if self.prover.busy() {
                    "PASTED: VERIFYING ITS PROOF"
                } else {
                    "PASTED A DRAFT: PROVE IT TO RACE TIMED"
                });
                out.push(Out::Click);
                out
            }
            Err(e) => {
                self.say(match e {
                    CodeError::Prefix => "NO CODE ON THE CLIPBOARD",
                    CodeError::Checksum => "THAT CODE HAS A TYPO IN IT",
                    CodeError::Format(_) => "A CODE FROM A NEWER BUILD",
                    _ => "THAT CODE IS CUT OFF OR DAMAGED",
                });
                Vec::new()
            }
        }
    }

    /// Proofs that came back from the builder.
    pub fn proofs(&mut self, done: Vec<(u64, Proved)>) -> Vec<Out> {
        let mut out = Vec::new();
        for (job, result) in done {
            if let Some(e) = self.checking.remove(&job) {
                out.extend(self.checked(e, result));
                continue;
            }
            let before = self.draft.layout.clone();
            match self.prover.finished(job, result, &before) {
                Some(Outcome::Proven(l)) | Some(Outcome::Reproved(l)) => {
                    self.draft.layout = l;
                    let s = match &self.prover.state {
                        ProofState::Proven(s) => s.line(),
                        _ => String::new(),
                    };
                    self.say(format!("PROVEN  {s}"));
                    out.extend(self.save_draft());
                }
                Some(Outcome::Confirmed) => self.say("PROOF CHECKED: IT RACES TIMED"),
                Some(Outcome::Failed) => {
                    self.draft.layout.proof = None;
                    out.extend(self.save_draft());
                }
                None => {}
            }
        }
        out
    }

    /// A listed layout's proof was checked.
    fn checked(&mut self, e: Entry, result: Proved) -> Vec<Out> {
        let Some(old) = self.entry_layout(e).cloned() else {
            return Vec::new();
        };
        let new = match result {
            Ok((proven, _)) => Layout {
                proof: proven.proof,
                ..old.clone()
            },
            Err(_) => {
                self.failed.push(e);
                old.unproven()
            }
        };
        match e {
            Entry::Course(i) => {
                self.courses[i] = new;
                Vec::new()
            }
            Entry::Slot(n) => {
                let origin = self.slots.get(n).and_then(|s| s.origin.clone());
                if self.slots.put(n, &new, origin) {
                    self.save_slots().into_iter().collect()
                } else {
                    Vec::new()
                }
            }
        }
    }

    /// Apply an edit, with its undo step, and keep the draft on disk.
    fn edit(&mut self, e: Edit) -> Vec<Out> {
        if !self.history.apply(&mut self.draft, &e) {
            return Vec::new();
        }
        if self.draft.layout.proof.is_none() {
            self.prover.edited();
        }
        self.sticky = None;
        self.armed = None;
        self.sel = self.sel.min(self.draft.run_count() - 1);
        self.save_draft().into_iter().collect()
    }

    /// Whether the draft races timed: proven by this build, not being
    /// checked.
    pub fn timed(&self) -> bool {
        matches!(self.prover.state, ProofState::Proven(_))
            && self.draft.layout.standing() == Standing::Proven
    }

    /// The race a test drive or a listed layout starts: timed only when
    /// proven by this build.
    fn race(&self, layout: &Layout, timed: bool, origin: Option<String>, test: bool) -> Vec<Out> {
        let layout = if timed && layout.standing() == Standing::Proven {
            layout.clone()
        } else {
            layout.unproven()
        };
        Custom::of(layout, origin, test)
            .ok()
            .map(|c| Out::Race(Box::new(c)))
            .into_iter()
            .collect()
    }

    fn test_drive(&mut self) -> Vec<Out> {
        self.page = Page::Edit;
        let timed = self.timed();
        self.race(&self.draft.layout, timed, self.origin.clone(), true)
    }

    /// COPY CODE: the share code when proven, the draft code otherwise.
    fn copy(&mut self) -> Vec<Out> {
        let proven = self.timed();
        let layout = if proven {
            self.draft.layout.clone()
        } else {
            self.draft.layout.unproven()
        };
        let Some(text) = clip::copy_text(&layout) else {
            return Vec::new();
        };
        self.say(if proven {
            "SHARE CODE COPIED"
        } else {
            "DRAFT CODE COPIED: PROVE IT TO SHARE"
        });
        vec![Out::Copy(text), Out::Click]
    }

    /// Do `a` (a button pressed, or a key mapped to one).
    pub fn act(&mut self, a: Act, build: &mut dyn Build, now: f64) -> Vec<Out> {
        self.now = now;
        let mut out = match self.page {
            Page::Edit => self.act_edit(a, build),
            Page::Menu => self.act_menu(a, build),
            Page::Save => self.act_save(a),
            Page::Name => self.act_name(a),
            Page::Seed => self.act_seed(a),
            Page::Tracks => self.act_tracks(a, build),
        };
        if !matches!(a, Act::Map | Act::Strip) && !out.iter().any(|o| matches!(o, Out::Click)) {
            out.push(Out::Click);
        }
        out
    }

    fn goto(&mut self, page: Page) {
        if matches!(page, Page::Name | Page::Seed | Page::Save) {
            self.back_to = if self.page == Page::Tracks {
                Page::Tracks
            } else {
                Page::Menu
            };
        }
        self.page = page;
        self.focus = 0;
        self.armed = None;
        self.history.seal();
    }

    /// The edit an option button makes on run `run`.
    fn option_edit(tool: Tool, i: u8, run: usize) -> Option<Edit> {
        Some(match (tool, i) {
            (Tool::Surface, i) => Edit::Surface {
                run,
                surface: Surface::ALL.get(i as usize).copied()?,
            },
            (Tool::Drift, i) => Edit::Drift {
                run,
                mark: [DriftMark::None, DriftMark::Hint, DriftMark::Required]
                    .get(i as usize)
                    .copied()?,
            },
            (Tool::Objects, 0) => Edit::Posts {
                run,
                side: Side::Left,
            },
            (Tool::Objects, 1) => Edit::Posts {
                run,
                side: Side::Right,
            },
            (Tool::Objects, 2..=4) => Edit::Rock {
                run,
                lane: (i - 2) as usize,
            },
            (Tool::Objects, 5) => Edit::ClearObjects { run },
            (Tool::Cars, 0..=2) => Edit::Car {
                run,
                lane: i as usize,
            },
            (Tool::Cars, 3) => Edit::ClearCars { run },
            (Tool::Gate, 0) => Edit::Gate { run },
            (Tool::Gate, 1) => Edit::MoveGate { run, segs: -5 },
            (Tool::Gate, 2) => Edit::MoveGate { run, segs: 5 },
            (Tool::Run, 0) => Edit::Split { run },
            (Tool::Run, 1) => Edit::Join { run },
            (Tool::Run, 2) => Edit::Add { run },
            (Tool::Run, 3) => Edit::Delete { run },
            (Tool::Run, 4) => Edit::Length { run, steps: -1 },
            (Tool::Run, 5) => Edit::Length { run, steps: 1 },
            _ => return None,
        })
    }

    fn act_edit(&mut self, a: Act, build: &mut dyn Build) -> Vec<Out> {
        let k = self.sel;
        match a {
            Act::Tool(t) => {
                self.tool = t;
                self.focus = 0;
                self.history.seal();
                Vec::new()
            }
            Act::Minus | Act::Plus => {
                let s = if a == Act::Plus { 1 } else { -1 };
                let e = match self.tool {
                    Tool::Bend => Edit::Bend { run: k, steps: s },
                    Tool::Hill => Edit::Hill { run: k, steps: s },
                    Tool::Width => Edit::Width { run: k, steps: s },
                    _ => return Vec::new(),
                };
                self.edit(e)
            }
            Act::Opt(i) => {
                let Some(e) = Editor::option_edit(self.tool, i, k) else {
                    return Vec::new();
                };
                self.focus = i as usize;
                let gate = self.draft.gates_on(k).first().copied();
                let out = self.edit(e.clone());
                match e {
                    // Follow a moved gate onto the run it moved to.
                    Edit::MoveGate { .. } => {
                        if let Some(&g) = gate.and_then(|g| self.draft.layout.checkpoints.get(g)) {
                            self.sel = self.draft.run_of((g as usize).saturating_sub(1));
                        }
                    }
                    Edit::Add { .. } => self.sel = (k + 1).min(self.draft.run_count() - 1),
                    _ => {}
                }
                out
            }
            Act::PrevRun => {
                self.select(k.saturating_sub(1));
                Vec::new()
            }
            Act::NextRun => {
                self.select((k + 1).min(self.draft.run_count() - 1));
                Vec::new()
            }
            Act::Undo | Act::Redo => {
                let done = if a == Act::Undo {
                    self.history.undo(&mut self.draft)
                } else {
                    self.history.redo(&mut self.draft)
                };
                if !done {
                    return Vec::new();
                }
                // A proof travels with the draft it belongs to.
                self.prover = Prover::loaded(&self.draft.layout, true).0;
                self.sel = self.sel.min(self.draft.run_count() - 1);
                self.save_draft().into_iter().collect()
            }
            Act::Prove => self.prove(build),
            Act::Test => self.test_drive(),
            Act::Menu | Act::Back => {
                self.goto(Page::Menu);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn prove(&mut self, build: &mut dyn Build) -> Vec<Out> {
        self.page = Page::Edit;
        match self.prover.state {
            ProofState::Proven(_) => self.say("ALREADY PROVEN"),
            ProofState::Proving(_) | ProofState::Verifying(_) => {}
            _ => {
                self.prover.prove(&self.draft.layout, build);
            }
        }
        Vec::new()
    }

    /// Pick run `k`.
    pub fn select(&mut self, k: usize) {
        if k != self.sel {
            self.sel = k;
            self.history.seal();
            self.armed = None;
        }
    }

    /// A finger on the map or the strip at `(x, y)`: pick what is under it.
    pub fn touch(&mut self, on: Act, x: f32, y: f32, screen: &Screen) {
        if self.page != Page::Edit {
            return;
        }
        let e = view::EditRects::of(screen);
        let n = self.draft.layout.segments.len();
        let seg = match on {
            Act::Map => {
                let pts = view::plan(&self.draft.layout.segments);
                let fit = view::Fit::new(&pts, e.map, draw::MAP_PAD);
                view::map_pick(&pts, &fit, x, y)
            }
            Act::Strip => Some(view::strip_pick(e.strip, n, x)),
            _ => None,
        };
        if let Some(seg) = seg {
            self.select(self.draft.run_of(seg));
        }
    }

    fn act_menu(&mut self, a: Act, build: &mut dyn Build) -> Vec<Out> {
        let Act::Item(item) = a else {
            if a == Act::Back {
                self.goto(Page::Edit);
            }
            return Vec::new();
        };
        match item {
            Item::TestDrive => self.test_drive(),
            Item::Prove => self.prove(build),
            Item::Save => {
                self.goto(Page::Save);
                self.focus = self.slot.or(self.slots.free()).unwrap_or(1) as usize - 1;
                Vec::new()
            }
            Item::Copy => {
                self.page = Page::Edit;
                self.copy()
            }
            Item::Name => {
                self.name_buf = self.draft.layout.name.clone();
                self.goto(Page::Name);
                Vec::new()
            }
            Item::Biome => {
                // The next biome, in place: scenery only, so a proof stays.
                let b = self.draft.layout.biome.next();
                let out = self.edit(Edit::Biome(b));
                self.say(format!("SCENERY: {}", b.name()));
                out
            }
            Item::Paste => {
                self.page = Page::Edit;
                vec![Out::Paste]
            }
            Item::Seed => {
                self.goto(Page::Seed);
                Vec::new()
            }
            Item::Blank => {
                let out = self.replace(Draft::blank().layout, None, None, true, build);
                self.say("A BLANK ROAD: UNDO BRINGS THE OLD ONE BACK");
                out
            }
            Item::Discard => {
                self.history.replace(&mut self.draft, Draft::blank());
                self.origin = None;
                self.slot = None;
                self.prover = Prover::default();
                self.sel = 0;
                self.page = Page::Edit;
                self.say("DRAFT DISCARDED: UNDO BRINGS IT BACK");
                if self.draft_locked {
                    Vec::new()
                } else {
                    vec![Out::Store(StoreFile::Draft, None)]
                }
            }
            Item::Leave => {
                self.open_tracks(build);
                Vec::new()
            }
        }
    }

    fn act_save(&mut self, a: Act) -> Vec<Out> {
        match a {
            Act::Back => {
                self.goto(Page::Menu);
                Vec::new()
            }
            Act::Slot(n) => {
                let n = n as u32;
                self.focus = (n as usize).saturating_sub(1);
                if !self.slots.editable(n) {
                    self.say(if self.slots.writable {
                        "THAT SLOT HOLDS WHAT THIS BUILD CAN'T READ"
                    } else {
                        "SAVED LAYOUTS CAN'T BE WRITTEN"
                    });
                    return Vec::new();
                }
                let other = self.slots.get(n).is_some() && self.slot != Some(n);
                if other && self.armed != Some(Armed::Overwrite(n)) {
                    self.armed = Some(Armed::Overwrite(n));
                    self.say(format!("SLOT {n} IS TAKEN: TAP AGAIN TO REPLACE"));
                    return Vec::new();
                }
                self.armed = None;
                let layout = self.draft.layout.clone();
                if !self.slots.put(n, &layout, self.origin.clone()) {
                    return Vec::new();
                }
                self.slot = Some(n);
                self.page = Page::Edit;
                self.say(format!("SAVED IN SLOT {n}"));
                self.save_slots()
                    .into_iter()
                    .chain(self.save_draft())
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    fn act_name(&mut self, a: Act) -> Vec<Out> {
        match a {
            Act::Back => {
                self.page = self.back_to;
                Vec::new()
            }
            Act::Key(8) => {
                self.name_buf.pop();
                Vec::new()
            }
            Act::Key(13) => {
                let name = self
                    .name_buf
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                let out = self.edit(Edit::Name(name));
                self.history.seal();
                self.page = Page::Edit;
                out
            }
            Act::Key(c) => {
                let c = c.to_ascii_uppercase();
                let ok = racer_sim::layout::NAME_CHARSET.contains(&c)
                    && self.name_buf.len() < racer_sim::layout::NAME_MAX
                    && !(c == b' ' && (self.name_buf.is_empty() || self.name_buf.ends_with(' ')));
                if ok {
                    self.name_buf.push(c as char);
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn act_seed(&mut self, a: Act) -> Vec<Out> {
        match a {
            Act::Back => {
                self.page = self.back_to;
                Vec::new()
            }
            Act::Key(8) => {
                self.seed_buf.pop();
                Vec::new()
            }
            Act::Key(13) => {
                let Ok(seed) = self.seed_buf.parse::<u64>() else {
                    self.say("TYPE THE SEED FROM A RUN'S RESULTS");
                    return Vec::new();
                };
                let key = (seed, self.seed_index, self.seed_branch);
                self.pending_fork = Some(key);
                vec![Out::Build(key)]
            }
            Act::Key(c @ b'0'..=b'9') => {
                if self.seed_buf.len() < 20 {
                    self.seed_buf.push(c as char);
                }
                Vec::new()
            }
            Act::Index(d) => {
                let tracks = racer_sim::gen::difficulty::TRACKS as i64;
                self.seed_index = (self.seed_index as i64 + d as i64).clamp(0, tracks - 1) as u32;
                if self.seed_index == 0 {
                    self.seed_branch = 0;
                }
                Vec::new()
            }
            Act::Branch => {
                if self.seed_index > 0 {
                    self.seed_branch ^= 1;
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// The origin and slot of a listed row.
    fn entry_origin(&self, e: Entry) -> (Option<String>, Option<u32>) {
        match e {
            Entry::Course(i) => (Some(bundled::COURSES[i].origin.to_string()), None),
            Entry::Slot(n) => (self.slots.get(n).and_then(|s| s.origin.clone()), Some(n)),
        }
    }

    fn act_tracks(&mut self, a: Act, build: &mut dyn Build) -> Vec<Out> {
        let entries = self.entries();
        let pages = entries.len().div_ceil(LIST_ROWS).max(1);
        let chosen = entries.get(self.list_sel).copied();
        let unread = "THIS BUILD CAN'T READ THAT LAYOUT";
        match a {
            Act::Back => vec![Out::Title],
            Act::Row(i) => {
                let at = self.list_page * LIST_ROWS + i as usize;
                if at < entries.len() {
                    self.list_sel = at;
                    self.armed = None;
                }
                Vec::new()
            }
            Act::Page(d) => {
                self.list_page =
                    (self.list_page as i64 + d as i64).clamp(0, pages as i64 - 1) as usize;
                self.list_sel = (self.list_page * LIST_ROWS).min(entries.len().saturating_sub(1));
                Vec::new()
            }
            // RACE
            Act::Item(Item::TestDrive) => {
                let Some(e) = chosen else { return Vec::new() };
                let Some(l) = self.entry_layout(e).cloned() else {
                    self.say(unread);
                    return Vec::new();
                };
                let (origin, _) = self.entry_origin(e);
                let timed = !self.checking(e);
                self.race(&l, timed, origin, false)
            }
            // EDIT: the row's layout as the draft.
            Act::Item(Item::Name) => {
                let Some(e) = chosen else { return Vec::new() };
                let Some(l) = self.entry_layout(e).cloned() else {
                    self.say(unread);
                    return Vec::new();
                };
                let (origin, slot) = self.entry_origin(e);
                let mut out = self.replace(l, origin, slot, true, build);
                self.say("OPENED: UNDO BRINGS THE OLD DRAFT BACK");
                out.push(Out::Click);
                out
            }
            // DELETE
            Act::Item(Item::Discard) => {
                let Some(Entry::Slot(n)) = chosen else {
                    self.say("COURSES CAN'T BE DELETED");
                    return Vec::new();
                };
                if !self.slots.editable(n) {
                    self.say("THAT SLOT CAN'T BE CHANGED");
                    return Vec::new();
                }
                if self.armed != Some(Armed::Delete(n)) {
                    self.armed = Some(Armed::Delete(n));
                    self.say(format!("TAP DELETE AGAIN TO EMPTY SLOT {n}"));
                    return Vec::new();
                }
                self.armed = None;
                self.slots.remove(n);
                if self.slot == Some(n) {
                    self.slot = None;
                }
                self.list_sel = self.list_sel.min(self.entries().len().saturating_sub(1));
                self.say(format!("SLOT {n} EMPTIED"));
                self.save_slots().into_iter().collect()
            }
            // EDITOR
            Act::Item(Item::Leave) => {
                self.open_edit();
                Vec::new()
            }
            Act::Item(Item::Paste) => {
                self.page = Page::Edit;
                vec![Out::Paste]
            }
            Act::Item(Item::Seed) => {
                self.goto(Page::Seed);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// The keys and pad buttons of event `e`, as the page's [`Act`]s; some
    /// only move the focus.
    pub fn key(&mut self, e: &Event) -> Option<Act> {
        let p = |keys: &[K], pads: &[Pad]| pressed(e, keys, pads);
        let back = p(&[K::Escape], &[Pad::East, Pad::Back]);
        let enter = p(&[K::Enter], &[Pad::South]);
        let typed = match e {
            Event::Key(K::Char(c)) => Some(*c),
            _ => None,
        };
        match self.page {
            Page::Name | Page::Seed => {
                if back {
                    return Some(Act::Back);
                }
                if enter {
                    return Some(Act::Key(13));
                }
                if matches!(e, Event::Key(K::Backspace)) {
                    return Some(Act::Key(8));
                }
                if self.page == Page::Name && matches!(e, Event::Key(K::Space)) {
                    return Some(Act::Key(b' '));
                }
                if self.page == Page::Seed {
                    if matches!(e, Event::Key(K::Left) | Event::Pad(Pad::Left)) {
                        return Some(Act::Index(-1));
                    }
                    if matches!(e, Event::Key(K::Right) | Event::Pad(Pad::Right)) {
                        return Some(Act::Index(1));
                    }
                    if p(&[K::Up, K::Down], &[Pad::Up, Pad::Down]) {
                        return Some(Act::Branch);
                    }
                }
                typed.map(Act::Key)
            }
            Page::Edit => self.key_edit(e, typed, back, enter),
            Page::Menu => {
                if back {
                    return Some(Act::Back);
                }
                let n = Item::ALL.len();
                if p(bind::MENU_UP, bind::PAD_UP) {
                    // Up a row, or round to the column's last item.
                    self.focus = if self.focus >= 2 {
                        self.focus - 2
                    } else {
                        let last = (n - 1) / 2 * 2 + (self.focus & 1);
                        if last < n {
                            last
                        } else {
                            last - 2
                        }
                    };
                } else if p(bind::MENU_DOWN, bind::PAD_DOWN) {
                    self.focus = if self.focus + 2 < n {
                        self.focus + 2
                    } else {
                        self.focus & 1
                    };
                } else if p(bind::MENU_LEFT, bind::PAD_LEFT) || p(bind::MENU_RIGHT, bind::PAD_RIGHT)
                {
                    if self.focus ^ 1 < n {
                        self.focus ^= 1;
                    }
                } else if enter || p(&[K::Space], &[]) {
                    return Some(Act::Item(Item::ALL[self.focus]));
                } else if typed == Some(b'C') {
                    return Some(Act::Item(Item::Copy));
                } else if typed == Some(b'V') {
                    return Some(Act::Item(Item::Paste));
                }
                None
            }
            Page::Save => {
                if back {
                    return Some(Act::Back);
                }
                let n = MAX_SLOTS as usize;
                if p(bind::MENU_LEFT, bind::PAD_LEFT) {
                    self.focus = (self.focus + n - 1) % n;
                } else if p(bind::MENU_RIGHT, bind::PAD_RIGHT) {
                    self.focus = (self.focus + 1) % n;
                } else if p(bind::MENU_UP, bind::PAD_UP) {
                    self.focus = (self.focus + n - 4) % n;
                } else if p(bind::MENU_DOWN, bind::PAD_DOWN) {
                    self.focus = (self.focus + 4) % n;
                } else if enter || p(&[K::Space], &[]) {
                    return Some(Act::Slot(self.focus as u8 + 1));
                }
                None
            }
            Page::Tracks => {
                if back {
                    return Some(Act::Back);
                }
                let n = self.entries().len();
                if p(bind::MENU_UP, bind::PAD_UP) && self.list_sel > 0 {
                    self.list_sel -= 1;
                    self.list_page = self.list_sel / LIST_ROWS;
                } else if p(bind::MENU_DOWN, bind::PAD_DOWN) && self.list_sel + 1 < n {
                    self.list_sel += 1;
                    self.list_page = self.list_sel / LIST_ROWS;
                } else if enter || p(&[K::Space], &[]) {
                    return Some(Act::Item(Item::TestDrive));
                } else {
                    return match typed {
                        Some(b'E') => Some(Act::Item(Item::Name)),
                        Some(b'X') => Some(Act::Item(Item::Discard)),
                        Some(b'V') => Some(Act::Item(Item::Paste)),
                        Some(b'N') => Some(Act::Item(Item::Leave)),
                        _ => None,
                    };
                }
                None
            }
        }
    }

    fn key_edit(&mut self, e: &Event, typed: Option<u8>, back: bool, enter: bool) -> Option<Act> {
        let p = |keys: &[K], pads: &[Pad]| pressed(e, keys, pads);
        if back || p(&[K::P], &[Pad::Start]) || typed == Some(b'M') {
            return Some(Act::Menu);
        }
        if p(bind::MENU_LEFT, bind::PAD_LEFT) {
            return Some(Act::PrevRun);
        }
        if p(bind::MENU_RIGHT, bind::PAD_RIGHT) {
            return Some(Act::NextRun);
        }
        let n = self.tool.options().len().max(1);
        if p(bind::MENU_UP, bind::PAD_UP) {
            if self.tool.stepper() {
                return Some(Act::Plus);
            }
            self.focus = (self.focus + 1) % n;
            return None;
        }
        if p(bind::MENU_DOWN, bind::PAD_DOWN) {
            if self.tool.stepper() {
                return Some(Act::Minus);
            }
            self.focus = (self.focus + n - 1) % n;
            return None;
        }
        if enter || p(&[K::Space], &[]) {
            return (!self.tool.stepper()).then_some(Act::Opt(self.focus as u8));
        }
        let step_tool = |d: i64, t: Tool| {
            let i = Tool::ALL.iter().position(|x| *x == t).unwrap_or(0) as i64;
            Tool::ALL[(i + d).rem_euclid(Tool::ALL.len() as i64) as usize]
        };
        if typed == Some(b'Q') || p(&[], &[Pad::L1]) {
            return Some(Act::Tool(step_tool(-1, self.tool)));
        }
        if typed == Some(b'E') || p(&[], &[Pad::R1]) {
            return Some(Act::Tool(step_tool(1, self.tool)));
        }
        match typed {
            Some(b'Z') => Some(Act::Undo),
            Some(b'Y') => Some(Act::Redo),
            Some(b'T') => Some(Act::Test),
            Some(b'R') => Some(Act::Prove),
            Some(c @ b'1'..=b'6') if !self.tool.stepper() => Some(Act::Opt(c - b'1')),
            _ if p(&[], &[Pad::West]) => Some(Act::Undo),
            _ if p(&[], &[Pad::North]) => Some(Act::Redo),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
