//! The editor through the game: buttons by touch, keys and pad, files,
//! the clipboard, the builder thread, test drives and forks. Engine-free.

use std::cell::Cell;
use std::rc::Rc;

use super::*;
use crate::app::{Effect, Game, Mode};
use crate::builder::{Inline, Key as BKey};
use crate::input::{Event, Intents, Key as K};
use crate::raster::Canvas;
use crate::sprites::Art;
use crate::touch::{Action, Button};
use racer_sim::layout::bundled;
use racer_sim::track::fixtures;

/// Generated tracks are the drag strip, two seconds a section on track 1
/// (lost at once) unless `allot` says otherwise.
fn game_held(allot: f64) -> (Game, Rc<Cell<bool>>) {
    let b = Inline::new(move |_k: BKey| {
        let mut t = fixtures::drag_strip();
        let n = t.checkpoints().len();
        t.set_allotments(vec![racer_sim::ticks(allot); n]);
        t
    });
    let hold = b.hold.clone();
    (Game::new(Box::new(b)), hold)
}

fn game_with(allot: f64) -> Game {
    game_held(allot).0
}

fn game() -> Game {
    game_with(2.0)
}

/// One frame with `events`, drawn, its buttons kept for the next.
fn step(g: &mut Game, events: Vec<Event>) -> (Vec<Effect>, Vec<Button>, Canvas) {
    let i = Intents {
        events,
        window: (1080, 2160),
        entropy: 9,
        ..Intents::default()
    };
    let fx = g.tick(1.0 / 30.0, &i);
    let mut c = Canvas::new(1, 1);
    let b = crate::screens::frame(g, &mut c, Art::shared());
    g.set_buttons(b.clone());
    (fx, b, c)
}

/// Where button `a` is, if on screen.
fn find(b: &[Button], a: Action) -> Option<(f32, f32)> {
    b.iter().rev().find(|x| x.action == a).map(|x| {
        (
            (x.rect.x + x.rect.w / 2) as f32,
            (x.rect.y + x.rect.h / 2) as f32,
        )
    })
}

/// Tap button `a` (it must be on screen): down, then up. The effects.
fn tap(g: &mut Game, a: Action) -> Vec<Effect> {
    let (_, b, _) = step(g, vec![]);
    let (x, y) = find(&b, a).unwrap_or_else(|| panic!("{a:?} not on screen: {b:?}"));
    let (mut fx, _, _) = step(g, vec![Event::FingerDown { index: 0, x, y }]);
    fx.extend(step(g, vec![Event::FingerUp { index: 0 }]).0);
    fx
}

fn edit(a: Act) -> Action {
    Action::Edit(a)
}

fn keys(g: &mut Game, ks: &[K]) -> Vec<Effect> {
    let mut fx = Vec::new();
    for &k in ks {
        fx.extend(step(g, vec![Event::Key(k)]).0);
    }
    fx
}

/// The texts written to `file`, in order (`None` for a delete).
fn stored(fx: &[Effect], file: StoreFile) -> Vec<Option<String>> {
    fx.iter()
        .filter_map(|e| match e {
            Effect::Store { file: f, text } if *f == file => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// Open the editor's editing page from the title.
fn open_editor(g: &mut Game) {
    step(g, vec![]);
    tap(g, Action::Tracks);
    assert_eq!((g.mode, g.editor.page), (Mode::Editor, Page::Tracks));
    tap(g, edit(Act::Item(Item::Leave)));
    assert_eq!(g.editor.page, Page::Edit);
}

/// Let the builder's work come back.
fn settle(g: &mut Game) -> Vec<Effect> {
    let mut fx = Vec::new();
    for _ in 0..3 {
        fx.extend(step(g, vec![]).0);
    }
    fx
}

#[test]
fn the_title_opens_the_tracks_list_and_the_editor_and_back_leads_home() {
    let mut g = game();
    open_editor(&mut g);
    assert_eq!(g.editor.entries().len(), bundled::COURSES.len());
    tap(&mut g, edit(Act::Menu));
    assert_eq!(g.editor.page, Page::Menu);
    step(&mut g, vec![Event::Back]);
    assert_eq!(g.editor.page, Page::Edit);
    tap(&mut g, edit(Act::Menu));
    tap(&mut g, edit(Act::Item(Item::Leave)));
    assert_eq!(g.editor.page, Page::Tracks);
    tap(&mut g, edit(Act::Back));
    assert_eq!(g.mode, Mode::Title);
    // The keyboard way in: T on the title.
    keys(&mut g, &[K::Char(b'T')]);
    assert_eq!(g.mode, Mode::Editor);
    keys(&mut g, &[K::Escape]);
    assert_eq!(g.mode, Mode::Title);
}

#[test]
fn edits_by_touch_are_kept_on_disk_undone_and_saved_to_a_slot() {
    let mut g = game();
    open_editor(&mut g);
    let before = g.editor.draft.clone();
    tap(&mut g, edit(Act::NextRun));
    assert_eq!(g.editor.sel, 1);
    tap(&mut g, edit(Act::Tool(Tool::Bend)));
    let fx = tap(&mut g, edit(Act::Plus));
    assert_eq!(g.editor.draft.bend(1), 5);
    let drafts = stored(&fx, StoreFile::Draft);
    assert_eq!(drafts.len(), 1, "the draft is written after the edit");
    match DraftFile::decode(drafts[0].as_deref()) {
        DraftRead::Draft(d) => assert_eq!(d.layout, g.editor.draft.layout),
        other => panic!("{other:?}"),
    }
    tap(&mut g, edit(Act::Tool(Tool::Surface)));
    tap(&mut g, edit(Act::Opt(1)));
    assert_eq!(g.editor.draft.surface(1), Surface::Gravel);
    tap(&mut g, edit(Act::Undo));
    tap(&mut g, edit(Act::Undo));
    assert_eq!(g.editor.draft, before);
    tap(&mut g, edit(Act::Redo));
    assert_eq!(g.editor.draft.bend(1), 5);
    // Save: MENU, SAVE, the first free slot.
    tap(&mut g, edit(Act::Menu));
    tap(&mut g, edit(Act::Item(Item::Save)));
    assert_eq!(g.editor.page, Page::Save);
    let fx = tap(&mut g, edit(Act::Slot(1)));
    let slots = stored(&fx, StoreFile::Slots);
    let s = Slots::decode(slots[0].as_deref());
    assert_eq!(
        s.get(1).unwrap().layout.as_ref(),
        Some(&g.editor.draft.layout)
    );
    assert_eq!(g.editor.slot, Some(1));
    // Saving over another slot asks for a second tap.
    g.editor.slot = None;
    tap(&mut g, edit(Act::Menu));
    tap(&mut g, edit(Act::Item(Item::Save)));
    let fx = tap(&mut g, edit(Act::Slot(1)));
    assert!(stored(&fx, StoreFile::Slots).is_empty(), "armed, not saved");
    assert!(g.editor.note().unwrap().contains("TAP AGAIN"));
    let fx = tap(&mut g, edit(Act::Slot(1)));
    assert_eq!(stored(&fx, StoreFile::Slots).len(), 1);
    // And the files read back at the next launch.
    let mut again = game();
    let draft = DraftFile {
        layout: g.editor.draft.layout.clone(),
        slot: g.editor.slot,
        origin: None,
    }
    .encode();
    again.load_layouts(slots[0].as_deref(), draft.as_deref());
    assert_eq!(again.editor.draft.layout, g.editor.draft.layout);
    assert_eq!(again.editor.slot, Some(1));
    assert_eq!(again.editor.entries().len(), bundled::COURSES.len() + 1);
}

#[test]
fn a_tap_on_the_map_or_the_strip_picks_the_run_under_it() {
    let mut g = game();
    open_editor(&mut g);
    let course = bundled::layouts().remove(1);
    let out = g.editor.replace(
        course,
        None,
        None,
        true,
        &mut Inline::new(|_| fixtures::drag_strip()),
    );
    drop(out);
    step(&mut g, vec![]);
    let e = view::EditRects::of(&g.layout);
    let pts = view::plan(&g.editor.draft.layout.segments);
    let fit = view::Fit::new(&pts, e.map, draw::MAP_PAD);
    let k = g.editor.draft.run_count() / 2;
    let (a, b) = g.editor.draft.run_range(k);
    let mid = (a + b) / 2;
    let (x, y) = fit.px(pts[mid]);
    step(
        &mut g,
        vec![Event::FingerDown {
            index: 0,
            x: x as f32,
            y: y as f32,
        }],
    );
    step(&mut g, vec![Event::FingerUp { index: 0 }]);
    assert_eq!(g.editor.sel, k);
    // Dragging along the strip follows the finger.
    let n = g.editor.draft.layout.segments.len();
    let x0 = view::strip_x(e.strip, 0, n) as f32 + 0.5;
    let x1 = view::strip_x(e.strip, n - 1, n) as f32;
    let y = (e.strip.y + 4) as f32;
    step(&mut g, vec![Event::FingerDown { index: 1, x: x0, y }]);
    assert_eq!(g.editor.sel, 0);
    step(&mut g, vec![Event::FingerMoved { index: 1, x: x1, y }]);
    assert_eq!(g.editor.sel, g.editor.draft.run_count() - 1);
    step(&mut g, vec![Event::FingerUp { index: 1 }]);
}

#[test]
fn prove_runs_on_the_builder_thread_and_the_ui_keeps_going() {
    let (mut g, hold) = game_held(2.0);
    hold.set(true);
    open_editor(&mut g);
    let course = bundled::layouts().remove(0);
    let mut b = Inline::new(|_| fixtures::drag_strip());
    g.editor
        .replace(course.unproven(), None, None, true, &mut b);
    tap(&mut g, edit(Act::Prove));
    assert!(matches!(g.editor.prover.state, ProofState::Proving(_)));
    // Frames go on while the worker works: selecting, a test drive's
    // free drive.
    tap(&mut g, edit(Act::NextRun));
    assert_eq!(g.editor.sel, 1);
    let (_, _, c) = step(&mut g, vec![]);
    assert!(c.palette_only());
    assert!(!g.editor.timed(), "not proven yet");
    // The worker answers.
    hold.set(false);
    let fx = settle(&mut g);
    assert!(matches!(g.editor.prover.state, ProofState::Proven(_)));
    assert_eq!(g.editor.draft.layout, course, "the bundled timers");
    assert_eq!(
        stored(&fx, StoreFile::Draft).len(),
        1,
        "the proof kept on disk"
    );
    // Proven: COPY gives the share code.
    tap(&mut g, edit(Act::Menu));
    let fx = tap(&mut g, edit(Act::Item(Item::Copy)));
    let copied: Vec<&String> = fx
        .iter()
        .filter_map(|e| match e {
            Effect::CopyText(t) => Some(t),
            _ => None,
        })
        .collect();
    assert_eq!(copied, vec![&course.to_code().unwrap()]);
    // Any edit to the road takes the proof away: COPY gives a draft code.
    tap(&mut g, edit(Act::Tool(Tool::Width)));
    tap(&mut g, edit(Act::Plus));
    assert_eq!(g.editor.prover.state, ProofState::Unproven);
    tap(&mut g, edit(Act::Menu));
    let fx = tap(&mut g, edit(Act::Item(Item::Copy)));
    assert!(fx
        .iter()
        .any(|e| matches!(e, Effect::CopyText(t) if t.starts_with("RD"))));
}

#[test]
fn an_edit_while_proving_drops_the_late_answer() {
    let (mut g, hold) = game_held(2.0);
    hold.set(true);
    open_editor(&mut g);
    let mut b = Inline::new(|_| fixtures::drag_strip());
    g.editor.replace(
        bundled::layouts().remove(0).unproven(),
        None,
        None,
        true,
        &mut b,
    );
    tap(&mut g, edit(Act::Prove));
    tap(&mut g, edit(Act::Tool(Tool::Hill)));
    tap(&mut g, edit(Act::Plus));
    hold.set(false);
    settle(&mut g);
    assert_eq!(g.editor.prover.state, ProofState::Unproven);
    assert!(g.editor.draft.layout.proof.is_none());
}

#[test]
fn a_pasted_code_is_accepted_wrapped_and_its_proof_verified_on_the_worker() {
    let mut g = game();
    open_editor(&mut g);
    let course = bundled::layouts().remove(2);
    let code = course.to_code().unwrap();
    let wrapped = format!(
        "here you go:\n{}\n{}\n",
        &code[..60],
        code[60..].to_ascii_lowercase()
    );
    tap(&mut g, edit(Act::Menu));
    let fx = tap(&mut g, edit(Act::Item(Item::Paste)));
    assert!(fx.contains(&Effect::ReadClipboard));
    let fx = g.pasted(&wrapped);
    assert!(!stored(&fx, StoreFile::Draft).is_empty());
    assert!(matches!(g.editor.prover.state, ProofState::Verifying(_)));
    assert!(!g.editor.timed(), "no timer until it is verified");
    settle(&mut g);
    assert!(matches!(g.editor.prover.state, ProofState::Proven(_)));
    assert!(g.editor.timed());
    assert_eq!(g.editor.draft.layout, course);
    // UNDO brings the old draft back.
    tap(&mut g, edit(Act::Undo));
    assert_eq!(g.editor.draft, Draft::blank());
    // Garbage changes nothing, and says why.
    let before = g.editor.draft.clone();
    g.pasted("not a code at all");
    assert_eq!(g.editor.draft, before);
    assert_eq!(g.editor.note(), Some("NO CODE ON THE CLIPBOARD"));
    let mut typo = code.clone().into_bytes();
    typo[40] = if typo[40] == b'2' { b'3' } else { b'2' };
    g.pasted(std::str::from_utf8(&typo).unwrap());
    assert_eq!(g.editor.note(), Some("THAT CODE HAS A TYPO IN IT"));
    // A proof that does not hold: pasted, verified, no timer.
    let mut tight = course.clone();
    // A wall of rocks across the road: no typical driver gets by cleanly.
    let wall = tight.checkpoints[0] + 20;
    for x in [-5, -3, -1, 1, 3, 5] {
        tight.objects.push(racer_sim::track::Object {
            seg: wall,
            x_cm: x * 100,
            kind: racer_sim::track::ObjectKind::Rock,
        });
    }
    tight.objects.sort_by_key(|o| o.seg);
    let hash = tight.hash();
    let p = tight.proof.as_mut().unwrap();
    p.stamp = racer_sim::layout::stamp(hash, &p.allot_ticks, p.epoch);
    g.pasted(&tight.to_code().unwrap());
    settle(&mut g);
    assert!(!g.editor.timed());
}

#[test]
fn fork_this_track_opens_the_raced_track_in_the_editor() {
    // From the results: a generated run lost on its first track.
    let mut g = game();
    step(&mut g, vec![]);
    keys(&mut g, &[K::Enter]);
    while g.mode != Mode::Results {
        step(&mut g, vec![]);
    }
    let seed = g.seed;
    let track = g.run.as_ref().unwrap().race().track().clone();
    tap(&mut g, Action::ForkTrack);
    assert_eq!((g.mode, g.editor.page), (Mode::Editor, Page::Edit));
    let mut want = Layout::from_track(&track);
    want.proof = None;
    assert_eq!(g.editor.draft.layout, want);
    assert_eq!(
        g.editor.origin.as_deref(),
        Some(&*format!("fork:{seed:#018x}:0:0:e{}", racer_sim::SIM_EPOCH))
    );
    // From the pause menu, mid-race: by keys, Up twice then confirm on the
    // results; the pause menu by touch.
    let mut g = game_with(60.0);
    step(&mut g, vec![]);
    keys(&mut g, &[K::Enter]);
    step(&mut g, vec![]);
    assert_eq!(g.mode, Mode::Racing);
    keys(&mut g, &[K::Escape]);
    assert!(g.paused);
    let fx = tap(&mut g, Action::ForkTrack);
    assert_eq!(g.mode, Mode::Editor);
    assert!(
        fx.iter()
            .any(|e| matches!(e, Effect::SaveRun { text, .. } if text.contains("ending quit"))),
        "the abandoned run is kept, as QUIT keeps it"
    );
}

#[test]
fn a_test_drive_races_the_draft_and_comes_back_to_the_editor() {
    let mut g = game();
    open_editor(&mut g);
    tap(&mut g, edit(Act::Test));
    assert_eq!(g.mode, Mode::Racing);
    let c = g.custom.as_ref().unwrap();
    assert!(c.test && !c.timed, "a draft is free drive");
    assert_eq!(c.layout, g.editor.draft.layout);
    // Pause: EDITOR instead of QUIT, no FORK of the draft itself.
    keys(&mut g, &[K::Escape]);
    let (_, b, _) = step(&mut g, vec![]);
    assert!(find(&b, Action::ForkTrack).is_none());
    tap(&mut g, Action::Quit);
    assert_eq!((g.mode, g.editor.page), (Mode::Editor, Page::Edit));
    // Driven to the end: the results go back to the editor too.
    tap(&mut g, edit(Act::Test));
    while g.mode == Mode::Racing {
        step(&mut g, vec![]);
    }
    assert_eq!(g.mode, Mode::Results);
    tap(&mut g, Action::Continue);
    assert_eq!((g.mode, g.editor.page), (Mode::Editor, Page::Edit));
}

#[test]
fn from_seed_forks_on_the_builder_thread() {
    let mut g = game();
    open_editor(&mut g);
    tap(&mut g, edit(Act::Menu));
    tap(&mut g, edit(Act::Item(Item::Seed)));
    assert_eq!(g.editor.page, Page::Seed);
    keys(
        &mut g,
        &[K::Char(b'4'), K::Char(b'2'), K::Char(b'7'), K::Backspace],
    );
    assert_eq!(g.editor.seed_buf, "42");
    tap(&mut g, edit(Act::Index(1)));
    tap(&mut g, edit(Act::Branch));
    assert_eq!((g.editor.seed_index, g.editor.seed_branch), (1, 1));
    keys(&mut g, &[K::Enter]);
    settle(&mut g);
    assert_eq!(g.editor.page, Page::Edit);
    assert_eq!(g.editor.pending_fork, None);
    assert_eq!(
        g.editor.origin.as_deref(),
        Some(&*format!("fork:{:#018x}:1:1:e{}", 42, racer_sim::SIM_EPOCH))
    );
    assert_eq!(
        g.editor.draft.layout.segments,
        fixtures::drag_strip().segments()
    );
}

#[test]
fn keys_and_a_pad_drive_the_editor() {
    use crate::input::Pad;
    let mut g = game();
    open_editor(&mut g);
    keys(&mut g, &[K::Right, K::Right]);
    assert_eq!(g.editor.sel, 2);
    keys(&mut g, &[K::Char(b'E'), K::Char(b'Q'), K::Char(b'E')]);
    assert_eq!(g.editor.tool, Tool::Hill);
    keys(&mut g, &[K::Up, K::Up]);
    assert_eq!(g.editor.draft.hill(2), 20);
    keys(&mut g, &[K::Char(b'Z')]);
    assert_eq!(g.editor.draft.hill(2), 0, "two presses, one step");
    step(&mut g, vec![Event::Pad(Pad::R1)]);
    step(&mut g, vec![Event::Pad(Pad::R1)]);
    assert_eq!(g.editor.tool, Tool::Surface);
    step(&mut g, vec![Event::Pad(Pad::Up)]);
    step(&mut g, vec![Event::Pad(Pad::South)]);
    assert_eq!(g.editor.draft.surface(2), Surface::Gravel);
    step(&mut g, vec![Event::Pad(Pad::West)]);
    assert_eq!(g.editor.draft.surface(2), Surface::Asphalt, "X undoes");
    // NAME by keys.
    keys(&mut g, &[K::Escape]);
    assert_eq!(g.editor.page, Page::Menu);
    for _ in 0..2 {
        keys(&mut g, &[K::Down]);
    }
    keys(&mut g, &[K::Enter]);
    assert_eq!(g.editor.page, Page::Name);
    keys(
        &mut g,
        &[
            K::Char(b'M'),
            K::Char(b'Y'),
            K::Space,
            K::Char(b'R'),
            K::Char(b'D'),
            K::Enter,
        ],
    );
    assert_eq!(g.editor.draft.layout.name, "MY RD");
    assert_eq!(g.editor.page, Page::Edit);
}

#[test]
fn a_listed_proof_from_older_rules_is_checked_on_the_worker_and_rewritten() {
    // FIRST LIGHT as the previous proof rules stamped it, in slot 4.
    let old = concat!(
        "RL04MSP544A9745J0REKW9ZZZZZ00HP0Z0ZH492FHFR20H7RQW1141Y1XST01Y4H",
        "C9Y40FC824Z207P44G7P03WSTJ4A441J0FY068G2192CR7G9E7B2R1YG5GSEAJGC",
        "Q5R70JWEP5G3ZZ9MC0JZW4GGAFZG1G4G869K21ZZM9H4RC493J7WG0ZK4CH7WR0Z",
        "G412FSG1Z0820ZG03YCHZZZX0CH021J4SGFZ8K68GR8G681ZZ93CB0GCQ6W7ZZ1K",
        "EBGR8P6R5ZZZ028803JC3R4K3G0GG48W044G3G14401S650G840G00QZT8008084",
        "23ZZZKG5ZZZZW00ZZSFJR888A4G90120008CC8H2CRMK96PJJJSFZY3ZZ1DPJS9B",
        "6PJ54RKH2080J09022H5462399YVZFZZZZR0BVZZ9ZMWQDJWPB5JJPPCSH208048",
        "JJJ31GRE31TE3ZDZYZZZZZG06N015JW0X4GTW0F750JA06Y06J06H2R1E005F092",
        "8J4H4928J4P00S2R02JH00A6DCFCPXN00QNW",
    );
    let (mut g, hold) = game_held(2.0);
    g.load_layouts(Some(&format!("racer-layouts 1\nslot 4 {old}\n")), None);
    hold.set(true);
    step(&mut g, vec![]);
    tap(&mut g, Action::Tracks);
    let e = Entry::Slot(4);
    assert!(g.editor.checking(e), "a stale stamp is checked");
    let (_, _, c) = step(&mut g, vec![]);
    assert!(c.palette_only());
    hold.set(false);
    let fx = settle(&mut g);
    assert!(!g.editor.checking(e));
    let slots = stored(&fx, StoreFile::Slots);
    let s = Slots::decode(slots[0].as_deref());
    assert_eq!(
        s.get(4).unwrap().layout.as_ref(),
        Some(&bundled::layouts()[0])
    );
    // RACE it: timed now.
    g.editor.list_sel = bundled::COURSES.len();
    tap(&mut g, edit(Act::Item(Item::TestDrive)));
    assert_eq!(g.mode, Mode::Racing);
    assert!(g.custom.as_ref().unwrap().timed);
}

#[test]
fn a_slot_is_deleted_only_on_the_second_tap_and_courses_never() {
    let mut g = game();
    let course = bundled::layouts().remove(3);
    let mut s = Slots::default();
    s.put(2, &course, None);
    g.load_layouts(s.encode().as_deref(), None);
    step(&mut g, vec![]);
    tap(&mut g, Action::Tracks);
    tap(&mut g, edit(Act::Row(0)));
    tap(&mut g, edit(Act::Item(Item::Discard)));
    assert_eq!(g.editor.note(), Some("COURSES CAN'T BE DELETED"));
    tap(&mut g, edit(Act::Row(4)));
    let fx = tap(&mut g, edit(Act::Item(Item::Discard)));
    assert!(stored(&fx, StoreFile::Slots).is_empty());
    let fx = tap(&mut g, edit(Act::Item(Item::Discard)));
    assert_eq!(
        stored(&fx, StoreFile::Slots),
        vec![Some("racer-layouts 1\n".to_string())]
    );
    // EDIT a course: it becomes the draft, proven (bundled: trusted).
    tap(&mut g, edit(Act::Row(1)));
    tap(&mut g, edit(Act::Item(Item::Name)));
    assert_eq!(g.editor.page, Page::Edit);
    assert_eq!(g.editor.draft.layout, bundled::layouts()[1]);
    assert!(g.editor.timed());
}

#[test]
fn the_demo_never_reads_or_writes_a_layout_or_the_clipboard() {
    let mut g = game_with(60.0);
    let s = {
        let mut s = Slots::default();
        s.put(1, &bundled::layouts()[0], None);
        s.encode()
    };
    g.load_layouts(s.as_deref(), None);
    let draft = g.editor.draft.clone();
    let slots = g.editor.slots.clone();
    step(&mut g, vec![]);
    let (x, y) = find(&step(&mut g, vec![]).1, Action::Demo).unwrap();
    let mut fx = step(&mut g, vec![Event::FingerDown { index: 0, x, y }]).0;
    fx.extend(step(&mut g, vec![Event::FingerUp { index: 0 }]).0);
    assert!(g.demo());
    for _ in 0..30 * 40 {
        fx.extend(step(&mut g, vec![]).0);
    }
    assert!(!g.can_fork());
    // Any input ends the demo; it never lands in the editor.
    fx.extend(keys(&mut g, &[K::Char(b'T')]));
    assert_eq!(g.mode, Mode::Title);
    assert!(fx.iter().all(|e| !matches!(
        e,
        Effect::Store { .. } | Effect::CopyText(_) | Effect::ReadClipboard
    )));
    assert_eq!(g.editor.draft, draft);
    assert_eq!(g.editor.slots, slots);
}

#[test]
fn a_newer_draft_on_disk_is_never_written_over() {
    let mut g = game();
    g.load_layouts(None, Some("racer-draft 9\nwhatever\n"));
    open_editor(&mut g);
    assert_eq!(g.editor.note(), Some("A NEWER BUILD'S DRAFT IS KEPT"));
    tap(&mut g, edit(Act::Tool(Tool::Bend)));
    let fx = tap(&mut g, edit(Act::Plus));
    assert!(stored(&fx, StoreFile::Draft).is_empty());
    // Damaged: dropped with a note, and written over.
    let mut g = game();
    g.load_layouts(None, Some("racer-draft 1\nslot x\n"));
    assert_eq!(g.editor.note(), Some("THE DRAFT WAS DAMAGED: DROPPED"));
    open_editor(&mut g);
    let fx = tap(&mut g, edit(Act::Plus));
    assert_eq!(stored(&fx, StoreFile::Draft).len(), 1);
}

#[test]
fn every_editor_page_lays_out_its_buttons_inside_the_buffer_in_palette_colours() {
    let mut g = game();
    open_editor(&mut g);
    for page in [
        Page::Tracks,
        Page::Edit,
        Page::Menu,
        Page::Save,
        Page::Name,
        Page::Seed,
    ] {
        g.editor.page = page;
        for tool in Tool::ALL {
            g.editor.tool = tool;
            let (_, b, c) = step(&mut g, vec![]);
            assert!(c.palette_only(), "{page:?}");
            for x in &b {
                let r = x.rect;
                assert!(
                    r.x >= 0 && r.y >= 0 && r.x + r.w <= c.w && r.y + r.h <= g.layout.bottom(),
                    "{page:?} {x:?}"
                );
            }
            if page != Page::Edit {
                break;
            }
        }
    }
}
