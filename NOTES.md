# Build notes

Where this build had to decide something the M1 plan did not, or where a
plan number produced a bad result. Numbers that were changed are reported
here rather than quietly retuned.

DESIGN.md is authoritative for mechanics; this file records the why behind
the numbers. "The plan" is the approved M1 plan the first build was written
against.

---

## 1. Gate numbers at the M1 commit

`cargo run --release -p racer-sim --bin batch -- --seeds 200` (K = 16, 16
threads, this machine: AMD Ryzen AI 7 PRO 350):

- 1,800 tracks: 1,009 first draw, 736 later draw, 31 repaired, 24 fallback
  (1.33%, gate ≤ 2%), 0 unvalidated (gate 0).
- Human run failure: 39 of 1,000 holdout runs (3.9%, band 3–15%), lost on
  track index [1, 3, 14, 10, 11].
- Per index (drawn and repaired tracks), perfect bot's time left, min/mean/
  max: 6.9/8.8/11.3, 6.7/9.6/16.4, 6.9/9.2/13.3, 6.6/9.4/13.7, 6.2/9.3/16.8 s
  (band 6–18). Skill gap: means 10.4, 12.4, 11.4, 11.8, 11.7% (band 8–30;
  maxima 14.7–23.5%).
- Drift-required corners on accepted tracks: 1,362 verified, 1,343 first
  time and 19 after tightening; the grip-only bot ran out of time on 202 and
  lost 1.50/2.43/6.11 s (min/mean/max) on the rest; 41 candidates rejected
  for a corner that stayed undemanding.
- Rejected candidates: 1 undrivable, 363 crash-prone, 18 perfect bot failed,
  242 perfect band, 337 skill gap, 41 drift corner, 1,059 margins.
- 35 s wall for the whole batch.

### Validation cost per track

Mean, per accepted track including every rejected candidate before it, on
one thread: 0.43M ticks / 70 ms (index 0, 1 loadout), 0.40M / 70 ms (1, 1),
0.90M / 164 ms (2, 2 loadouts), 1.69M / 321 ms (3, 4), 3.57M / 701 ms (4, 8).
Worst single track 14.5M ticks, 2.7 s. The plan's estimate of ~1.8M ticks a
track holds for one candidate at index 4; candidates that fail cost extra.
K is not reduced in the batch.

On a phone M2 should build both sides of the next fork while the current
track is driven (a minute of wall time for at most ~1.4 s of work here), and
may reduce K on device; the batch is where K's effect is measured.

### Determinism

`cargo run --release -p racer-sim --bin determinism`: 200 runs, 0 replay
divergences, digest `0x4a5c1bcc8fc9fdfb`, identical across runs and with the
process pinned to one core.

## 2. Changed a plan number, with a reason

### Spin: v×0.45 for 60 ticks → v×0.15 for 90 ticks, 8 m/s² drag

At the plan's spin a crash cost 1.5–2 s, inside the margin plus slack, so
a crash almost never lost a run and the human run failure sat near 0%. A
spin now costs 2–4 s, about what an arcade spin-out costs, and a crash on a
later track usually loses it.

### Human lapses: 0.5%/s → 1%/s, plus overcooked corners

The plan's noise (aim σ, onset jitter, 0.5%/s lapses) made a human that is
consistent to a few tenths of a second and almost never crashes on an
accepted track. Two changes give it a person's tail: lapses twice as often,
and *overcooking*: each corner, with 4% chance per trial, it carries 15% too
much speed in and runs wide. Both are constants in `bot/human.rs`, for M2
playtests to set.

### Human caution: 0.85 of grip, as planned; braking planned at 60%

Kept. A more cautious human (0.8) widened the skill gap and cut rejections
but dropped the run failure to 1.8%.

### Human sees traffic to `min(120 m, sight)`, not `min(1.2 s·v, 120 m, sight)`

With the plan's look-ahead applied to traffic, the human closing at
25–30 m/s on a slow car had under two seconds to change lanes and hit
traffic in most trials on some tracks. That calibrated a crash *into* the
timer. A car on a straight is visible far beyond the bend-planning horizon,
so the 1.2 s rule now applies to the road only.

### Steering: PD → cascaded with a capped sideways speed

The plan says PD. With 150 ms of delay a PD lane change into the outside of
a bend overshot into the posts there. The outer loop now asks for a capped
sideways speed (3.5 m/s human, 4.5 m/s perfect) and the inner loop for the
acceleration to reach it. Still quantised to seven levels with hysteresis.

### Calibration on the *weakest* reachable loadout

The plan says "p75 split (base loadout for that index)". Every reachable
loadout at index ≥ 1 already holds that track's own upgrade, so there is no
upgrade-free car to calibrate on. The timer is calibrated on the loadout the
perfect bot is slowest on, so every other loadout has at least the time the
weakest needs; verification still covers them all. Slack is 0.5 s; the
cumulative allotment is rounded *up* to the quarter second.

### Reachable loadouts: at most 8, not 16

A track's own upgrade comes with it, so track `(i, b)` is met with
`2^(i−1)` loadouts at most (1, 1, 2, 4, 8).

### Crash-prone tracks are rejected

Not in the plan. If more than a quarter of the 16 calibration trials hit
something, the p75 would include a crash and the timer would be generous
around a flaw. Such tracks are redrawn.

### Fallback bends at most 15 milliradians a segment (270 m radius)

With 66 (a 60 m radius) the engine-upgraded car's corners bound, the human
overcooked them, and the fallback itself sometimes failed the floor. At 15 no
corner binds for any loadout, so the fallback is flat out and always passes.

### Drift-corner timing window

The plan says the grip-only perfect bot must "fail or lose ≥ 1.5 s". The
loss is measured from 30 segments before the corner to 40 after it, so the
braking for it and the boost out of it both count; "fail" is running out of
time before the corner is through.

### Perfect bot: beam search over drift only

The plan beam-searches drift and lane choices. With traffic on a closed-form
schedule the earliest-clear lane is known exactly, so lanes are chosen
greedily; the beam (width 4) covers drift per corner. Searching lanes too
would multiply the cost for little.

### Batch: five holdout runs per run seed

200 runs gave a failure estimate too noisy to gate on (± ~1.2 points at
3%); 1,000 runs (five per seed, cheap next to building the tracks) steady it.

### Ported code, trimmed

`dmath.rs` lost Salt Line's artillery angle grid (`dir_from_ticks`,
`angle_ticks_from_dir`) and its rstest cases; the crate's only dev
dependencies are proptest and insta.

## 3. Open tuning concerns

- **The run-failure floor is the tight gate.** 3.9% against a 3% floor.
  The per-track floor selects against failure: a track must keep the margin
  in 15 of 16 trials on *every* reachable loadout (128 trials at index 4),
  so a track with a 3% per-drive failure passes about half the time, and
  accepted tracks fail rarely. Stronger loadouts also bank time across a
  track, since allotments are cumulative and set for the weakest. Tightening
  calibration does not help: slack 0.25 s, or 0.4 s rounded to the nearest
  quarter, moved the failure rate by at most 0.3 points while fallbacks rose
  to 3.5–4%. Only a fallible human moves it. If a sim change trips this
  gate, re-centre it with playtest data (M2) rather than add noise.
- **Rejection rate.** 44% of first draws fail a gate; the commonest
  causes are margins (a fallible human on a track with one bad spot), the
  skill gap below 8% and crash-prone spots. The generator could aim better
  (fewer post-lined carousels, more crests on easy tracks) to spend less
  time redrawing.
- **Human fidelity** (plan risk): aim σ, delay, lapses and overcooking are
  guesses until M2 playtests on the Pixel, A16 and tablet.
- **Skill gap sits low in its band** (means 10–12%). The band could be
  re-centred once real players set the human bot's numbers.
- **Upgrades only help.** The weakest-loadout calibration assumes nothing
  is ever worse than base; upgrades that trade (§4.2) will need a
  per-upgrade report and maybe per-loadout calibration.

## 4. M2a renderer (feature/renderer)

Measured on the Pixel 10 (57220DLCR001SH), release export with
`--features profiling`, DEMO driving, 80 two-second windows: raster (paint
the canvas and lay out its buttons) mean 0.48 ms, worst window p95
0.74 ms; whole `process` (sim ticks and the bot included) mean 2.0 ms;
frames 16.67 ms, 60.0 fps, vsync-bound. Both sides of every fork were
built before DEMO reached the fork.

Decisions the plan left open:

- **Camera**: 8 m up, 5 m back, focal 60 px; the horizon follows from
  the car (14 rows above the HUD), so a taller phone shows more sky
  rather than a smaller car. Hills are drawn at twice their height
  (`Camera::hills`): at true scale the sim's 5-10 m hills flattened into a
  few rows and crests did not read. Presentation only.
- **Touch**: a finger on L or R that slides onto the other re-binds (one
  thumb steers); everything else is Salt Line's capture on down.
- **Palette**: a new sixteen-colour road palette, not Salt Line's.
- **Keystore**: there is no `~/.android/debug.keystore` here; the
  Justfile falls back to the editor's
  `~/.local/share/godot/keystores/debug.keystore`.

---

## Sim epochs

`SIM_EPOCH` (src/lib.rs) names the digest over the pinned track digests
(`tests/snapshots.rs`) and the golden-run corpus (`tests/golden_runs.txt`);
`tests/golden_runs.rs` fails if the sim's outcomes move without a bump. When
it fails: bump `SIM_EPOCH`, regenerate the corpus
(`cargo test -p racer-sim --test golden_runs -- --ignored regenerate`),
append `(epoch, digest)` to `EPOCHS` (never edit an entry that has reached
`main`), and add a line here.

- **Epoch 1:** the first M1 sim. Epoch digest: `0x7038998bdfbbe7ab`.
