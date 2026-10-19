# Racer — Design Spec

Working codename `racer`; the real name is picked after the feel slice (§7).
A portrait, retro pseudo-3D "behind the car" racer in which every generated
track is **proven drivable** by a human-like driver before it is shown.
Deterministic Rust simulation core, Godot 4 presentation layer via gdext,
built the way Salt Line is built.

**Status: Milestone 1** (sim and solver spine) is built: everything below
not marked *(M2)*. M2 adds the renderer, the portrait layout and input
devices, and the phone builds.

---

## 1. Run structure

A run is five generated tracks of about a minute each. A track is split by
3–4 checkpoint gates; the last gate is the finish line.

### 1.1 The timer

The arcade checkpoint timer is the only fail state. The race starts with the
first section's allotment; each gate crossed adds the next section's, and
leftover time carries forward. Time running out before the next gate loses
the run. The time left as each gate is crossed is that gate's *margin*.

Time is an integer tick count (120 a second), never an accumulated float.

### 1.2 Forks

After each track the route forks between two tracks, each carrying a
different upgrade (§4). Branch 0 is the easier road, branch 1 the harder
(§2.5). Taking a side takes its upgrade for that track and every track
after; upgrades stack. Five tracks means four forks: nine distinct tracks
per run seed, at most eight reachable loadouts on the last.

```
Racing --finish--> Forking --fork(b)--> Racing ... --finish track 5--> Won
   \--timer out--> Lost
```

Both sides of a fork depend only on `(run_seed, index, branch)`, so the game
can build them while the player drives the track before (M2).

## 2. Tracks

### 2.1 Representation

A track is a list of 4 m segments driven from first to last. Each segment
stores its shape as small integers:

| field | unit | meaning |
|---|---|---|
| `curve_milli` | milliradians per segment | how far the road turns; + is right. 100 is a 40 m radius, 10 a 400 m sweep |
| `pitch_milli` | thousandths | grade; + climbs |
| `half_width_cm` | cm | 600 normally (three 4 m lanes), 450 on narrows |
| `surface` | enum | asphalt, gravel, ice, puddle (§3.4) |
| `flags` | bits | `DRIFT_REQ`, `DRIFT_HINT`, `CREST` |

plus checkpoint gates (segment indices), timer allotments (ticks), roadside
objects (post, rock: segment, offset, kind) and traffic (start position,
speed, lane, all integers).

Integers make a track identical on every platform by construction and give
it a cheap exact digest. Some data is *derived* on construction and never
hashed: elevation (the integrated pitch), the sight distance from every
segment (a running-maximum-slope line-of-sight scan from 1.1 m eye height,
capped at 160 m), and the `CREST` flags (summits with under 60 m of sight on
their approach).

The renderer (M2) draws segments as the classic pseudo-3D road: screen
offset accumulates curvature, height accumulates pitch. The sim needs no
geometry beyond that: a bend is a sideways push on the car (§3.2), a hill is
gravity along the road and a limit on how far ahead a driver can see.

### 2.2 Seeding

`track_seed(run_seed, index, branch) = mix64(run_seed ^ key·GOLDEN)` where
`key = index << 8 | branch` (Salt Line's `screen_seed`, re-keyed). Within a
seed, each layer — geometry, narrows, surfaces, objects, traffic — draws from
its own named substream, and each solver attempt from its own seed, so adding
a draw to one layer never reshuffles another.

### 2.3 Generation pipeline

```
Track   := Launch Straight (Section Checkpoint){3|4} Finish
Section := Element+ Straight
Element := Straight | Sweep | Hairpin | SBend | Chicane | Crest | Dip | Carousel
```

1. **Geometry** (`gen/grammar.rs`). Elements are drawn by weight until each
   section reaches its share of ~600 segments; every gate stands on a short
   straight. Bends ease in and out with a smoothstep; hills are bump-shaped.
   Carousels (drift corners) are slotted in behind a 25-segment (100 m,
   about two seconds) straight with no crest.
2. **Overlays** (`gen/placement.rs`), each from its own substream: narrows,
   surfaces, objects, traffic (§2.6).
3. **Calibrate, verify, or climb down the ladder** (§2.4).

### 2.4 Solvability

Every track is proven before it is shown. Not "probably": by two bots, on
every loadout the player can arrive with.

**The bots** (`bot/`) share one set of hands: a cascaded steering controller
(offset error → capped sideways speed → acceleration, plus the bend's
`κ·v²` as feedforward), quantised to the player's seven steering levels, and
a lane chooser that checks each lane's time to the next traffic car or
object and follows traffic when every lane is blocked.

- *Perfect bot*: whole-track backward-pass speed profile at 93% of grip,
  zero delay, exact traffic schedule, and a width-4 beam search over which
  corners to drift. It is the ceiling.
- *Human bot*: acts on 150 ms (±4 ticks) old observations; plans its speed
  over `min(1.2 s of travel, 120 m, crest sight line)` and slows for what a
  crest might hide; sees traffic to `min(120 m, sight line)`; uses 85% of
  grip and plans braking at 60% of the brakes; aims off its line by
  σ = 0.08 of the half width; holds steering with hysteresis; lapses 1% a
  second (holds its input 0.3–0.8 s); overcooks 4% of corners by 15%; drifts
  only signed corners and long tight ones. Its noise is one PCG32 per trial.
  It is the floor.

**The ladder** (`solver.rs`), Salt Line's §2.4 construct → check → fall back:

1. *Calibrate.* Find the weakest reachable loadout (the one the perfect bot
   is slowest on). Run 16 human trials untimed on it; each gate's cumulative
   allotment is the 75th percentile of the time to reach it, plus the
   required margin `M(index)`, plus 0.5 s, rounded up to 0.25 s. A track
   where more than a quarter of these trials hit something is rejected:
   its timer would be calibrated around a crash.
2. *Ceiling.* The perfect bot must finish with 6–18 s left.
3. *Skill gap.* The human median must be 8–30% over the perfect time.
4. *Drift corners.* Every drift-required corner gets the two-bot test
   (§2.5); a corner that is not demanding enough is tightened by 15% and the
   track recalibrated, up to twice.
5. *Floor.* On every reachable loadout, 15 of 16 human trials with noise
   seeds calibration never saw must keep `M(index)` at every gate:
   2.0 s on the first track, falling by 0.25 s a track to 1.0 s.

A failing draft is redrawn (up to 8 attempts); then the best failed draft's
worst section is softened (bends eased by a quarter, hills halved, objects
cleared, traffic moved on; up to twice); then the gentle fallback (the
grammar at difficulty 0, no hills, bends of at most a 270 m radius that every
loadout takes flat out, no overlays), which is still calibrated and must
pass the floor but is exempt from the difficulty bands.

**The batch gate** (`bin/batch.rs`, CI): over 200 run seeds (1,800 tracks),
no unvalidated tracks, at most 2% fallbacks, and the human, playing 1,000
runs on holdout noise seeds with random forks, losing between 3% and 15% of
them. NOTES.md has the numbers and why the failure floor is the tight one.

### 2.5 Difficulty curve and drift

`D(index, branch)`: 0.15, 0.32, 0.50, 0.65, 0.78 by index; branch 0 is
0.06 below, branch 1 0.06 above. D raises hairpin, S-bend, chicane and crest
weights, hairpin tightness, crest height, hazard and traffic density.

Drift is optional early, then required, introduced deliberately:

- Track 1 has one *hint* carousel (`DRIFT_HINT`): drifting it pays, nothing
  requires it.
- From track 3, one *drift-required* carousel (`DRIFT_REQ`), two on the hard
  side of the last fork. 45–65 segments at 115–140 milliradians (a 29–35 m
  radius).
- Each is verified by two perfect bots: one that may drift must pass; one
  held to grip must run out of time or lose at least 1.5 s over the corner
  (measured from 30 segments before it to 40 after, so the boost out counts).
- A drift-required corner and its approach get no ice and no traffic. Gravel
  on it, which rewards drifting, comes only from track 4.

### 2.6 Hazards

- **Crests**: blind summits hide the road; the human slows for them, the
  perfect bot does not, which is much of the skill gap. Sometimes a bend
  waits just over the top.
- **Traffic**: packs of one or two slow cars (14–24 m/s) in different lanes,
  one lane always free, timed to meet the player on straights and sweeps.
  Each car is closed-form, `z = z0 + v·t`, so bots look ahead exactly and
  traffic can never desync. Traffic starts at least 120 m up the road.
- **Objects**: posts along the outside of bends (punishing running wide),
  post rows pinching a lane on straights, rocks in a lane from track 3.
  Hitting one, or traffic, spins the car (§3.3); the car glances clear.
- **Surfaces** (§3.4): gravel on bends, ice patches from track 3, puddles.
- **Narrows**: some straights on harder tracks are 9 m wide, not 12.

## 3. The car

### 3.1 Input

One byte per tick: seven steering levels (full, two-thirds, one-third each
way, straight), a brake bit and a drift bit. The throttle is automatic. Touch
zones, keys and a pad's stick all quantise to this byte (`Input::from_axis`,
`Input::from_keys`), so a replay cannot tell devices apart.

### 3.2 Motion

Four numbers: `z` along the road and speed `v`; `x` across it (+ right) and
`vx`. Semi-implicit Euler at 1/120 s. No heading, no trig.

```
v'  = ACC·(1 − v/vmax_eff) − BRAKE·b − G·pitch − scrub
vx' = steer·A(v) − κ·v² − DAMP·vx,     A(v) = μ_eff·min(1, v / 6 m/s)
```

Base car: top speed 52 m/s, acceleration 16 m/s² from rest, grip 20 m/s²,
brakes 20 m/s². Full lock holds the line exactly while `κ·v² ≤ μ`, i.e. below
`sqrt(μ/κ)`; above it the car slides outward predictably. That bound is what
the bots plan with. Off the road the car loses a quarter of its grip, 40% of
its top speed and 4 m/s²; 5 m past the edge is the barrier, which stops the
slide and scrubs speed.

### 3.3 Drift

`Grip → Entry (6 ticks, −3% speed) → Drifting → Grip`, entered by holding
drift with some steering above 14 m/s. Drifting raises the lateral cap to
1.45× grip, drops damping from 5/s to 1.5/s and scrubs 2.5 m/s². Each
second of steered drifting charges a second of boost charge; letting go
pays a mini boost (0.6 s charged: 0.6 s of +8 m/s², +6 m/s top speed) or a
super boost (1.5 s: 1.25 s of it). A hit, or the barrier while drifting,
spins the car: 90 ticks without control, 15% of its speed kept, 8 m/s² of
drag; a spinning car cannot be hit again.

### 3.4 Surfaces

| surface | grip | drift grip | top speed | drag |
|---|---|---|---|---|
| asphalt | 1.0 | 1.0 | 1.0 | 0 |
| gravel | 0.65 | 0.95 | 0.9 | 1.5 m/s² |
| ice | 0.4 | 0.42 | 1.0 | 0 |
| puddle | 0.55 | 0.6 | 0.92 | 2.0 m/s² |

Gravel holds a drift almost as well as asphalt while killing grip, which is
why it rewards drifting; ice punishes both.

## 4. Upgrades

Five, one axis each, stacking multiplicatively:

| upgrade | effect per stack |
|---|---|
| engine | top speed +5% |
| turbo | acceleration +12% |
| tyres | grip +7% |
| brakes | brakes +20% |
| charger | drift charge +30% |

Each fork offers two different ones, drawn from `(run_seed, index)`. In M1
every upgrade only helps, so a loadout is a small, legible set of numbers.

### 4.1 Reachable loadouts

The loadouts a track can be met with are its own upgrade plus one side of
every earlier fork: 1, 1, 2, 4, 8 by index (fewer when sides repeat). The
solver checks every one (§2.4).

### 4.2 Open *(later milestones)*

Upgrades that can hurt (top speed bought with grip), with a per-upgrade
batch report.

## 5. Art and the portrait layout *(M2)*

Decided, not built:

- **View**: pseudo-3D scanline road toward a horizon, curves and hills and
  crests; the car fixed bottom-centre of the road view. The projection is
  pinned in the sim's metres.
- **Portrait**: road view on top, thumb-zone controls and HUD below, one
  software-rastered buffer. Assumed logical width 180, height floating with
  the aspect: Salt Line's "fix the short side" rule, rotated.
- **Art rules**: Salt Line's — a 16-colour palette, 4× source art, nearest
  filtering everywhere.
- **Controls**: Left/Right steer, Down brakes, Drift held. Touch and
  keys/pad equally first-class.
- **Devices**: Android export with Salt Line's stripped templates and the
  `device_power` addon; determinism digests on devices.

## 6. Architecture

```
racer-sim/                # Rust, no dependencies at all, no engine
  ├─ dmath.rs             # platform-stable trig (no libm)       [Salt Line]
  ├─ rng.rs               # SplitMix64, PCG32, substreams        [Salt Line]
  ├─ hash.rs              # FNV-1a                               [Salt Line]
  ├─ track.rs             # segments, sight lines, builder, fixtures
  ├─ traffic.rs           # closed-form traffic
  ├─ input.rs             # one byte per tick
  ├─ car.rs               # motion, drift state machine, surfaces
  ├─ race.rs              # timer, gates, collisions, input log
  ├─ upgrade.rs           # upgrades, loadouts, reachable loadouts
  ├─ run.rs               # forks, stacking, track sources, autoplay
  ├─ gen/                 # grammar, difficulty, overlays, calibration
  ├─ bot/                 # profile, control, perfect, human
  ├─ solver.rs            # validate + redraw/repair/fallback ladder
  ├─ replay.rs            # log codec, Replay, verified reconstruction
  ├─ codec.rs             # bits, Rice, gamma, base32, CRC-16    [Salt Line]
  ├─ layout.rs, layout/   # layouts: model, proof, codes, courses (§9)
  ├─ viz.rs               # ASCII strip of a track
  └─ bin/                 # batch, determinism, trackviz, layout
racer-godot/              # gdext stub: one node reporting SIM_EPOCH (M1)
```

The sim builds and tests headless. Its harness is the batch (§2.4): build
1,800 tracks across 200 run seeds, prove each, play 1,000 holdout runs, gate
on the result. Tuning happens there, not by playing the game a thousand
times.

Determinism test (`bin/determinism`): 200 human-bot runs (10 run seeds × 20
players), each replayed from its seed and log alone; one digest over every
track and final state. CI compares it across Linux, macOS and Windows;
Android and iOS join in M2.

The batch and the determinism check build tracks on every core. Building is
a pure function of its key and results are gathered in key order, so thread
count cannot change an output; the library itself is single-threaded.

## 7. Independence checklist

Confirm before any public build:

- [ ] Name, title and codename have no lineage to any existing racing game
- [ ] Zero code, data, track or asset lineage from any commercial racer:
      every track is generated from a seed by this repository's grammar;
      no track layout, curve table or course data is copied or traced
- [ ] No vehicle, brand, sponsor or livery resembling a real or game car
- [ ] Palette, sprites and UI developed from reference photography and our
      own sketches, not from another game's screenshots
- [ ] Sound and music made for this game
- [ ] Code shared with Salt Line is our own (dmath, rng, hashing, the
      replay and epoch scheme, the ladder) and stays so
- [ ] Employer personal-project review submitted and cleared before publishing

## 8. Determinism, replay and versioning

### 8.1 What is persisted

A run is its seed and its log. The log is one event per change of input,
`(ticks since the previous change, input)`, per race, with a fork marker
between races. A minute of driving is a few hundred events. A replay
recomputes everything else through the same `step` calls the live game
makes, then runs each race on with its last input to its end. A live run
matches its replay at race boundaries.

### 8.2 Text form

`v1`, then `D:S` per input change — `S` is the steering (`L3`..`L1`, `C`,
`R1`..`R3`) followed by `B` if braking and `D` if drifting — and `>B` per
fork. Only canonical text is accepted, so a round trip is the identity.

### 8.3 `SIM_EPOCH`

Salt Line's scheme. `SIM_EPOCH` names a digest over the pinned generated
track digests and a golden-run corpus; a test fails if the sim's outcomes
move without a bump. `EPOCHS` is append-only, and every epoch's digest is
recorded in NOTES.md. A log from another epoch keeps its seed and nothing
else.

### 8.4 Testing

- Unit tests per module; property tests (any race replays to the same state
  hash, the codec round-trips any log, the car stays finite and inside the
  barriers for any input).
- Snapshots (insta): fixture strips and digests, generated track digests.
- Golden runs: six autoplay runs that win, lose, take both forks, brake and
  drift, replayed to their recorded hashes.
- The determinism digest and the batch gate in CI.

## 9. Layouts and the track editor

**Status: Phase 2 built (October 2026)**: Phase 1's data model, fork
from a seed, proof, share codes, bundled courses, `layout` host tools and
`--racer-layout` dev flag, and Phase 2's in-game editor (§9.11): the TRACKS
list, a top-down map and elevation strip with touch-first tools, slots and
a draft on disk, COPY/PASTE codes, PROVE on the builder thread, FORK THIS
TRACK, test drives, proof rules that scale with length (§9.4) and run
files that carry the layout (§9.6). Salt Line's §13 is the model; where
this section differs, §9.9 says why.

**Owner decisions (October 2026), fixed:**

- A layout is the quantised track itself, not a seed: it outlives the
  generator.
- Any generated track can be exported as a layout (fork from seed);
  provenance is kept beside the code, not inside it.
- Free drive needs no proof. PROVE runs the solver's validation on the
  layout and sets its timers; only proven layouts race with a timer or are
  shared as codes. A proven code carries its timers and a proof stamp.
- Codes are compact, versioned, clipboard-safe text with a checksum.
- 3–5 bundled courses ship as proven layouts.
- Generation, goldens and the determinism digest do not move.

### 9.1 Concepts

| Term | Meaning |
|---|---|
| **Layout** | One track written down as data: segments, gates, objects, traffic, a name, and (once proven) its timers and stamp. `racer_sim::layout::Layout`. |
| **Proof** | The solver's checks (§2.4) passed under the custom-race rules (§9.4); it sets the checkpoint timers. |
| **Custom race** | A run of one layout's track (`RunState::single`): timed if proven, free drive if not. |
| **Course** | A proven layout that ships with the game (§9.7). Multi-layout courses are later. |

### 9.2 The data model

A layout holds exactly the integers a `Track` is built from (§2.1), so
nothing is quantised that was not already:

| Field | Holds |
|---|---|
| `segments` | per 4 m segment: `curve_milli`, `pitch_milli`, `half_width_cm`, `surface`, `flags` (`DRIFT_REQ`, `DRIFT_HINT` only: `CREST` is derived) |
| `checkpoints` | gate segment indices, ascending; the last is the finish (1–8 gates) |
| `objects` | roadside objects: segment, `x_cm`, kind, in segment order |
| `traffic` | cars: `z0_dm`, `speed_dms`, `lane_cm` (a pack is cars close together) |
| `name` | up to 12 of `A`–`Z`, `0`–`9` and single inner spaces |
| `proof` | `Some { allot_ticks, epoch, stamp }` once proven |

**Why integers** (Salt Line's reasons, its §13.2): an edit is a whole
number of steps, so undo and redo are exact; the code can delta-code
without rounding; every device builds the same doubles. Unlike Salt Line
there is no grid to choose: a generated track is already integers, so **a
fork is the generated road exactly**. Its track, with the generated timers
put back, *equals* the generated track (same `track_hash`), and the same
inputs drive it to the same run state hash (tested).

The digest (`Layout::hash`) covers the road only, not the name or the
proof: renaming or re-proving keeps a layout's ghosts.

Limits. Structural ones (`Layout::structure`: segment, gate, object and
traffic counts, ascending gates, authored flags only, a valid name, the
proof's shape) stop a layout being built or coded. Gameplay ones
(`Layout::check`: at least 500 m, a 100 m launch before the first gate,
turns of at most 180 mrad a segment, grades of at most 0.3, half widths
3–8 m, objects within the verge, traffic starting 120 m up the road and
before the finish, at 5–40 m/s, in a lane on the road) stop it being
proven; each names the segment, object or car at fault, for the editor to
mark.

### 9.3 Fork from a seed, and provenance

`layout::fork(run_seed, index, branch)` builds the track through the
solver as a run would meet it and writes it down. The timers are not kept:
they were set for the loadouts a run reaches that track with (§4.1), not
for a custom race. The fork's `Provenance`
(`fork:<seed>:<index>:<branch>:e<epoch>`) is written beside the code, as
the `origin` line of a layout file; the epoch matters because the same key
forks to another road after a bump.

### 9.4 Proof

`Layout::prove` runs `solver::check`, the one pass of §2.4 that the ladder
also uses, under fixed **custom-race rules** (`layout::rules`):

- the **base loadout** only (a custom race is driven with it);
- a required margin of **1.5 s** at every gate (`CUSTOM_MARGIN`: the
  middle of a run's 2.0–1.0 s);
- the default effort: 16 calibration trials, then 16 verification trials
  of which 15 must keep the margin;
- strict: the perfect-bot band, the skill-gap band (8–30%) and the
  drift-corner two-bot test all apply;
- the **perfect-bot band scaled with the length** (`perfect_band`, rules
  version 2): the generated band, 6–18 s left, is set for a minute of
  road (600 segments to the finish, `BAND_REF_SEGS`). Past that its top
  edge grows in proportion to the length; below it its bottom edge
  shrinks in proportion; the other edge stays, so the band only widens and
  a minute-long layout meets exactly the generated band. Without it every
  long layout failed at the top (three generated first tracks end to end,
  7.6 km: the perfect bot keeps 21 s, the fixed band allows 18; the
  scaled one 57);
- noise seeds from the layout's digest, so proving is deterministic.

Generated tracks never use the scaling: `solver::Rules` gained the band as
a field, which the generated rules set to `PERFECT_LEFT`. Nothing generated
moved: the golden runs, the batch numbers and the determinism digest
(`0x4a5c1bcc8fc9fdfb`) are unchanged, with no `SIM_EPOCH` bump.

**The race cap scales too.** A race is capped at four minutes
(`MAX_RACE_TICKS`), which a long layout's typical driver can exceed, so
the cap is now `race::race_cap`: four minutes, or the time to the finish
at 16 m/s for a road longer than 3,840 m. No generated track is half that
long (tested), so no generated race changes.

**Rules are versioned in the stamp** (`PROOF_RULES` = 2). A stamp made
under older rules reads `Stale`, not `BadStamp` (`stamp_under` tries each
older version), and is proven again; every layout that proved under
version 1 proves under 2 to the same timers (the band only widened; the
timers do not depend on it), so the bundled courses were restamped with
their timers unchanged.

No repairs: a layout is what its author made. A drift corner that is not
demanding enough fails (`DriftCorner`) rather than being tightened; the
author removes its `DRIFT_REQ` flag or sharpens it.

**The stamp** is a 32-bit FNV digest over the layout's digest, the timers,
`SIM_EPOCH` and `PROOF_RULES`. `Layout::standing` reads it without
simulating anything: `Unproven`, `Proven`, `Stale` (another epoch: prove
again) or `BadStamp` (edited after proving, or damaged: prove again). It is
a digest, not a signature, and anyone can forge one, so **trust, then
verify**: `Layout::verify` proves again (deterministic, so a genuine proof
comes back `Confirmed`) and returns the layout with this build's timers if
they differ. Proving costs **60–80 ms** on the desktop (one core;
NOTES.md), so the dev flag verifies every code it loads; the Phase 2 game
should too, off the main thread, as it builds tracks now.

Under these rules **87% of generated tracks prove** as forked (450 tracks,
NOTES.md): every first track, 71% of the hard last ones. The rest fail
where their generated proof leaned on an upgrade or a smaller margin. A
three-minute layout proves in about 130 ms on the desktop.

### 9.5 Codes

Two prefixes, one format (`layout/code.rs`). **`RL`** is the share code
and always carries a proof; **`RD`** is a draft code, never proven, for
files, the tools and (Phase 2) the editor's clipboard. Only `RL` codes are
offered for sharing, so a shared code always races timed. After the
prefix: Crockford base32 of a format byte (`LAYOUT_FORMAT` = 1), a
bit-packed body and a CRC-16 (Salt Line's codec, `codec.rs`, unchanged).
The body:

- the curve and pitch channels: the first value, then first or second
  differences (whichever is shorter), as plain Rice or zero runs and values
  (whichever is shorter): Salt Line's residual coder;
- half width, surface and flags as run lengths (Elias gamma);
- gates as gaps; objects and traffic as steps from the one before, so post
  rows and packs cost a few bits each;
- the proof: timers, epoch and the 32-bit stamp.

**Size.** Over 450 forked tracks: drafts median **553** characters, p95
678, max 788; share codes median 572, max 814. The bundled courses are
548–646, a hand-made 240-segment layout 95. Budget: every fork under 1 KB
(tested).

**Decoding is total**: text over 128 Ki characters is refused unread, every
count is bounded before use, every read past the end is an error,
neighbouring equal runs are refused (one spelling per road), the prefix
must agree with the proof bit, and a decoded layout always passes
`structure()`, so it always builds a track. Errors are typed (`CodeError`).
Property tests round-trip random layouts (runs of equal segments, extreme
values) and feed arbitrary text, arbitrary bodies under a valid checksum
and single-character mutations of real codes; every single-character typo
of the pinned code is caught.

`SIM_EPOCH` and `LAYOUT_FORMAT` are independent, as in Salt Line: an epoch
bump does not change what a layout *is*, only whether its proof still
holds (`Stale`).

### 9.6 Custom races and `--racer-layout`

`Game::start_custom` races one layout as a run of one track
(`RunState::single`; nothing about generated runs, their state hash or
their replays changes). Timed if proven; otherwise free drive, with FREE
where the time was and the race's cap. The best finish is kept as a
ghost under `(layout digest, u32::MAX, 0)` in the existing ghost file.
RETRY COURSE races it again. The TRACKS list, the editor's TEST DRIVE and
the dev flag start them (§9.11).

**Run files of custom races** (built in Phase 2): a custom race is saved
like any run, to the same runs directory, as `run-layout-<digest>.txt`, in
run file format 2: `racer-run 2`, then the epoch, the ending, the seed
(the layout's digest), a `layout <code>` line and the log. The code is the
layout as raced: an `RL` code with the timers it raced under, or `RD` for
a free drive. `Layout::raced_track` rebuilds that track from the code
whatever its stamp's standing, and `Replay::reconstruct_single` replays it,
so the file replays exactly (tested, timed and free). A generated run is
still written as format 1, byte for byte, and both are read. `playstats`
replays custom races against the human bot on the same track with the
custom loadout and sums them in a `custom` row of their own.

`--racer-layout <code|path>` (Godot user args, `cli.rs`) starts one at
launch; a path (`res://`, `user://` or a file) is read if it exists. The
layout is verified first; one that does not read leaves the title up with
a warning on the console.

### 9.7 Bundled courses

`layout/bundled.rs`: four proven share codes compiled in, easiest first,
each a fork lightly tuned by hand, with its origin and tuning beside it:

| Course | Origin | Tuning |
|---|---|---|
| FIRST LIGHT | seed 3, track 1 | a puddle after the first gate |
| RIDGE RUN | seed 5, track 3 (hard side) | none |
| GRAVEL PASS | seed 7, track 4 (easy side) | gravel over the drift corner |
| LONG DUSK | seed 8, track 5 (easy side) | one traffic car fewer |

Tests decode each, require no problems and a `Proven` stamp, prove each
again to the timers it carries and to the generated tracks' bands, and (in
the epoch they were made) hold each code to its recipe.

### 9.8 Tools

`bin/layout.rs`, through `just layout …`: `export --seed S --index I
--branch B` (a draft code and its origin), `prove <code|file>` (the gate
report, the time, the proven code), `show <code|file>` (a summary and the
ASCII strip), `check [--verify] <files>`, `survey --seeds N` (the numbers
in §9.4 and §9.5) and `bundled` (`just layout-courses`).

### 9.9 Where this differs from Salt Line

- **A proof is stamped onto the layout.** Salt Line validates on every load
  (47 µs). Here proving is two bots over a minute of road, about 70 ms, and
  it sets the timers, which are part of the race being shared; so the code
  carries them, stamped, and an importer verifies.
- **Two prefixes.** `RL` (proven) and `RD` (draft) make "only proven
  layouts are shared" visible in the text itself.
- **No quantisation grid.** A track is already integers, so a fork is exact
  (Salt Line's forks move heights by up to 1/32).
- **No width or wind**; the loadout is fixed (base) by the proof rules.
- **Longer codes** (about 550 characters against 200): a minute of road has
  twice Salt Line's samples, in five channels, plus objects and traffic.

### 9.10 Phases and open questions

| Phase | What |
|---|---|
| **1 (built)** | data model, fork, prove/stamp/verify, codes, bundled courses, tools, `--racer-layout` |
| **2 (built)** | the in-game editor (§9.11): TRACKS list, map and strip, tools, slots and draft, COPY/PASTE, PROVE and VERIFY on the builder thread, FORK THIS TRACK, FROM SEED, test drives; length-scaled proof rules (§9.4); run file format 2 (§9.6) |
| 3 | courses of several layouts, played as a short run |
| 4 | sharing through a relay |

Phase 1's open questions, as Phase 2 settled them (owner, October 2026):

- ~~**Bands for any length.**~~ The band scales with length, in the
  custom-race rules only (§9.4).
- ~~**The custom margin and loadout.**~~ Fixed: base loadout, 1.5 s, no
  per-layout rules.
- ~~**Proving on a phone.**~~ On the builder thread, never blocking: a
  pasted proof is VERIFYING (no timer) until it comes back; listed slots
  and courses with a stale or bad stamp are checked when TRACKS opens.
- ~~**Replays of custom races.**~~ Run file format 2 (§9.6).

Open:

- **Ghosts across epochs** are dropped, like every ghost, even though the
  road is the same.
- **Proving on the Pixel** has not been timed: the builder thread keeps
  the UI live whatever it costs, but a three-minute layout may take a
  second or more there. Measure before Phase 3.
- **Traffic packs** are placed one car at a time at a run's start, at
  18 m/s; a car meets the player well after its start (it is moving). A
  "meet the player here" placement (solve for the start from a typical
  pace) would read better.
- **The map's fit.** The road is drawn north-up and fitted whole; a long,
  thin road leaves much of the map empty. Rotating to the road's long axis
  would use it.
- **Names** have no word filter (Salt Line's question too), before codes
  are shared beyond a clipboard.
- **Phone check.** The editor is tested engine-free and by golden frames;
  it has not been used on the Pixel yet (thumb reach, the clipboard on
  Android, how the map reads at that size).

### 9.11 The editor (Phase 2, built)

`racer-godot/src/editor/`, engine-free and tested: `model.rs` (the draft,
its runs, every edit, undo), `proof.rs` (PROVE/VERIFY and their words),
`store.rs` (slots and draft files), `clip.rs` (codes on the clipboard),
`view.rs` (rectangles, projections, hit tests), `draw.rs` (the pages) and
`mod.rs` (the pages' logic and keys). `app.rs` gives it a mode,
`Mode::Editor`.

**Where it lives.** The title's **TRACKS** (or `T`) opens the TRACKS list:
the four bundled courses, then the player's slots, each with its length
and a badge (PROVEN, FREE, CHECKING, FAILS, NEWER). RACE races the
selected row (timed only if proven by this build); EDIT opens it as the
draft; DELETE empties a slot on a second tap (courses cannot be deleted);
EDITOR opens the draft; PASTE and SEED as below. **FORK TRACK** on the
pause menu and on the results (and TIME UP) opens the track just raced as
the draft, its provenance (`fork:<seed>:<index>:<branch>:e<epoch>`) kept
beside it; on a custom race it forks the layout itself. A paused run that
is forked is saved as QUIT first. The results now show the run's seed, so
any of its tracks can be forked later with FROM SEED.

**The editing page** is the portrait buffer split as the race view is:
the **top-down map** in the road-view area (the heading integrated from
the curve with the sim's platform-stable trig, as trackviz draws it;
surfaces coloured, drift corners marked, gates, finish, start, objects and
cars beside the road), with the **elevation strip** under it; everything
touchable is in the bottom 110 rows, the thumb zone: the run picker
(`<` RUN 6/22 `>`), two rows of tools (BEND, HILL, WIDTH, SURF, DRIFT; OBJ,
CARS, GATE, RUN, MENU), the tool's options, and UNDO, REDO, TEST, PROVE. A
tap or drag on the map or the strip picks the run under the finger.

**Runs.** A road is edited a run at a time: a stretch cut where the road
changes between straight, left and right (`auto_runs`), and re-cut with
RUN's SPLIT, JOIN, ADD (a 100 m straight), DEL and LEN-/LEN+ (20 m,
resampling the run and moving what stands on it). Runs are the editor's,
not the code's.

**Tools.** BEND and HILL step a run's peak (5 mrad a segment; 1% of grade)
in one canonical shape each, a smoothstep-eased arc and one sine period of
grade (the road comes back down), so stepping up and down again is exact
and a generated bend is snapped to that shape when first touched. WIDTH
steps the half width by 0.5 m (3–8 m). SURF paves the run, DRIFT marks it
NONE, HINT or MUST (required, held to the two-bot test). OBJ places post
rows outside either edge or a rock in a lane, or clears the run; CARS
places a car (18 m/s) in a lane at the run's start, or clears them; GATE
adds a gate at the run's end or removes the run's gates (one always
stays) and moves one by 20 m (the selection follows it). Every edit to the
road drops the proof; renaming keeps it. **UNDO/REDO** keep 64 whole-draft
snapshots; a run of presses of one stepped tool on one run is one step;
replacing the draft (a fork, a paste, a layout opened, NEW BLANK, DISCARD)
is a step too, so UNDO brings the old draft back.

**Warnings are notes, never blocks.** What would stop a proof
(`Layout::check`: too sharp at 480 m, a car too early...) is shown over
the map with a count; a long road gets a note that it is slow to prove.
Anything can be driven: TEST races the draft in the pseudo-3D view (free
drive unless proven), and its results and pause go back to the editor
(EDITOR instead of CONTINUE and QUIT, no FORK of the draft itself).

**PROVE** sends the road to the builder thread (`Build::prove`, the same
worker that builds the next fork's tracks) and returns at once; the badge
reads PROVING… and the UI goes on. The answer carries a job number and is
dropped unless it is for the road on screen (an edit meanwhile makes it
stale). Passed: PROVEN with the timer, the perfect and the typical times;
failed: the gate's verdict in plain words (GATE 2 IS TOO TIGHT FOR A
TYPICAL DRIVER, TOO EASY: A PERFECT RUN HAS 25.0S LEFT, A DRIFT CORNER IS
TOO GENTLE: SHARPEN IT OR UNMARK IT, ...). Only a proven draft races timed
or copies as a share code.

**COPY / PASTE.** COPY CODE puts the `RL` share code on the system
clipboard when the draft is proven by this build, otherwise its `RD` draft
code. PASTE CODE reads the clipboard (`Effect::ReadClipboard`, the
adapter's `DisplayServer` clipboard, back through `Game::pasted`) and
searches it for a code: wrapped across lines, quoted, indented, spaced,
hyphenated or lower-cased, with words before or after it (`clip::paste`).
The layout is accepted as the draft; a proof it carries is not trusted:
VERIFYING proves it again on the worker, and it races timed only once that
comes back (with this build's timers if they differ); a proof that fails
leaves it untimed. A paste that holds no code says why (no code, a typo, a
newer format, cut off).

**Slots and the draft** (`store.rs`, Salt Line's §13.8 rules):
`user://layouts/layouts.txt` is a `racer-layouts 1` header and one
`slot <n> <code> [origin <text>]` line per slot (24 slots); lines this
build cannot read are kept verbatim and their slots held; a code in a
newer format keeps its slot, never saved over or deleted; a file from a
newer build, or unreadable (too large, NULs), is never written. SAVE asks
for a second tap to replace another layout's slot. `user://layouts/draft.txt`
(`racer-draft 1`, the slot, the origin, the code) is rewritten after every
edit and deleted by DISCARD; a draft from a newer build is kept and never
written over (a note says so); a damaged one is dropped with a note. Both
are written by the adapter through a temporary file and a rename
(`Effect::Store`), and a failure comes back as a note (`Game::stored`); a
launch that finds only the temporary copy reads it.

**FROM SEED** is a keypad for a run seed (the number on the results), the
track (1–5) and the road (easy or hard); FORK asks the builder thread for
that generated track and opens it as the draft when it comes.

**NAME** is a keypad of the name's 37 characters (12 at most).

**Keys and a pad.** On the editing page: Left/Right (A/D, d-pad) pick the
run, Up/Down (W/S) step the tool or move between its options, Enter (A)
applies an option, Q/E (L1/R1) change tool, `1`–`6` pick an option, Z/Y
(X/Y on a pad) undo and redo, T test, R prove, M/Esc/P (Start) the menu.
The menu, slots and TRACKS move a focus with the arrows; C copies and V
pastes from the menu; the keypads take typed letters and digits,
Backspace and Enter. Mouse clicks are touches (`emulate_touch_from_mouse`).

**DEMO** starts only from the title and ends on any input, so it never
reaches the editor or TRACKS; a test runs a whole DEMO cycle and requires
no layouts file, draft or clipboard effect and the editor unchanged.
