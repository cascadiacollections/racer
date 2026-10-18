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
  ├─ viz.rs               # ASCII strip of a track
  └─ bin/                 # batch, determinism, trackviz
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
