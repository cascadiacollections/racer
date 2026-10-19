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

## Device power and 120 Hz (2026-10-18)

- `addons/device_power` is vendored from the godot-device-power v0.1.0 release
  zip (see its `UPSTREAM.md`); `godot/platform_services.gd` passes
  `DevicePower.recommended_fps_cap()` to `Racer.set_platform_fps_cap`, and the
  race runs at `tighter_cap(project, platform)`; menus still idle at 30.
- The Android export is now a Gradle build so the addon's Kotlin plugin is
  bundled: run `just android-template` once (unpacks Godot's
  `android_source.zip` into the untracked `android/build`).
- Its `Surface.setFrameRate` vote lifts Android's 60 fps default for games.
  Pixel 10 (Android 16), release + profiling, DEMO: charger 120.0 fps
  (8.33 ms, p95 8.33), panel 120 Hz; `dumpsys battery unplug` 60.0 fps with the
  panel at 60 Hz; Severe heat 30 fps, back to 120 after the 30 s cool-down.
  Frame work 2.6 ms mean, ~5 ms p95: inside the 8.3 ms budget at 120.
- The Gradle APK is 76 MB: the official engine library is stored
  uncompressed (71 MB). Salt Line's stripped engine template (about 23 MB) is
  the fix (since done: docs/godot-templates.md).

## Steering ramp and the gesture inset (2026-10-18)

- Keys and touch buttons no longer jump to full lock: `input::SteerRamp`,
  stepped per sim tick, holds level 1 for 10 ticks and level 2 for 14 more,
  reaching full lock at 200 ms; releasing or reversing restarts it. The stick
  is not ramped. The shaped byte is what the sim and replays see. Values are a
  first guess from the M2a note (full lock reached the verge in ~1.5 s at
  150 km/h); tune by feel.
- On phones the controls sit above Android's gesture area: the node passes
  the larger of the display's bottom safe-area gap and 32 dp (Pixel 10: 84 px
  = 14 buffer rows) as `Intents::bottom_inset`; `Layout::with_inset` lifts the
  pad's buttons (capped at `INSET_MAX` = 16 rows, the road gives up the rows)
  and the full-screen screens place things from `Layout::bottom()`. Desktop
  has no inset.

## Determinism on the phone (2026-10-18)

`just android-digest` (ported from Salt Line) cross-compiles the
`determinism` bin for arm64, runs it on the attached phone through
`tools/android_digest.sh`, and fails unless its digest equals the host's.
Pixel 10 (Tensor G5, Android 16): `0x4a5c1bcc8fc9fdfb` over 200 runs and
432,547 logged inputs, 0 replay divergences: identical to Linux, macOS and
Windows. CI cannot run an arm64 phone; the `sim-android` job runs the same corpus
on an x86_64 Android emulator (API 35, aosp_atd) and the `determinism` job
requires its digest to match the OS matrix.

## Playtest capture (2026-10-18)

The bot's numbers (reaction, noise, the 3–15% run-failure band) are guesses
until compared with real drivers. Every player run that ends (won, out of
time, or quit from pause; never DEMO) is saved as a run file
(`replay::encode_run`: epoch, ending, seed, the input log) to the app's
external files directory, `.../files/Documents/runs/` on Android, from
`OS.get_system_dir(SYSTEM_DIR_DOCUMENTS, false)` (no permission, adb-readable),
`user://runs` elsewhere. `just pull-runs` copies them off the phone and runs
`playstats`: per track, the player's time, closest margin, spins and
time-outs next to 16 holdout human-bot drives of the same track and loadout,
then a per-index summary. A quit's last track is shown but not counted (its
replay runs on past the quit). Files from another epoch are skipped.

Gotcha: `JavaClassWrapper` rejects `null` for a `String` parameter (only
object parameters take `null`), so `getExternalFilesDir(null)` matched no
method and, in a release export, silently returned null.

## CI Android dev builds (2026-10-18)

`ci.yml`'s `android` job exports the release APK (arm64, Gradle, stripped
engine) on every pull request and push to main, checks that its manifest
registers `org.godotengine.plugin.v2.DevicePowerAndroid` and uploads it as
the `dist-android` artifact. On main, `dev release` replaces the rolling
`dev` prerelease ("Development build (<sha>)") with `Racer-android-arm64.apk`
and `SHA256SUMS`, attested (`gh attestation verify`). versionName is
`0.1.0-dev.<run number>`, versionCode the run number, so dev builds upgrade
each other with `adb install -r` as long as the key stays the same.

- **Engine**: main runs `prepare-android --require`, so a stale lock fails
  the build; pull requests warn and fall back to the official engine.
  `vars.STRIPPED_ENGINE=off` forces the official engine everywhere.
- **Signing**: main uses the release key from the `ANDROID_RELEASE_KEYSTORE`
  (or `_BASE64`), `ANDROID_RELEASE_KEYSTORE_PASSWORD` and
  `ANDROID_RELEASE_KEY_ALIAS` secrets (repository or `release` environment)
  when all are set; otherwise `ANDROID_DEBUG_KEYSTORE` (base64), otherwise a
  throwaway key per run. The release notes say which. Changing key, or
  going between a CI build and a local `just android` build (different debug
  keys), needs an uninstall, which deletes saved runs.

## README and project page (2026-10-18)

- `site/` is a one-page project site and a licenses page, modelled on Salt
  Line's. Its game images are the renderer's golden frames, painted by
  `tools/build_site.py` (standard library only: it runs
  `dump_golden_frames` and writes indexed PNG files in the 16 colours, failing
  on any other colour), so the page follows the renderer without screenshots.
  The README's three captures (`docs/screens/`) are Pixel 10 screenshots
  scaled to 2x the buffer.
- `pages.yml` builds the site on pull requests and `main` pushes that touch it
  and uploads it as the `site` artifact. Deploying is off while the game is
  unnamed: enable Pages with GitHub Actions as the source and set the
  repository variable `PAGES_DEPLOY=on` to publish.
- `LICENSE` (MIT, matching `license = "MIT"` in Cargo.toml) and
  `third_party/` (Godot, gdext, glam, libc notices) were added for the
  licenses page.

## Sound and music (2026-10-18)

- Every sound is generated by `tools/gen_audio.py` (after Salt Line's:
  standard library only, no samples) into `art/audio/*.wav`, 16-bit mono:
  effects at 22,050 Hz, the title music at 16,000 Hz. It uses only exact
  float arithmetic (its own sine and exp series, not libm), so `just
  check-audio` (CI's hygiene job) can demand byte-identical output;
  `just gen-audio` rewrites them. About 780 KB in all, 512 KB of it the music.
- Loops (engine 0.5 s, tyres 0.6 s, music 16 s: eight bars at 120 bpm, Am F C
  G) carry a RIFF `smpl` forward loop over the whole file, which Godot's
  default import ("Detect From WAV") turns into `LOOP_FORWARD`, so the
  `.import` files keep their defaults. Tones fit whole cycles in the loop;
  noise is folded with an equal-power crossfade.
- The core decides (`audio.rs`, engine-free, tested): one-shots from what
  changed between sim ticks (a gate, a boost by tier, a spin, each of the last
  five timer seconds, time up, the win; clicks for menu choices) as
  `Effect::Sound`; the engine (pitch and volume from speed, lifted by a boost)
  and the screech (volume from slip while drifting) as `Effect::Loops`, eased
  in and out at 8 and 6 per second and sent at most 30 times a second; the
  title music (`Effect::Music`) on the title and while the first track builds,
  never while racing. Pause, the fork, results and the title fade the loops
  out; focus loss cuts them. DEMO asks for nothing.
- Levels and the sounds themselves are first guesses made without a speaker:
  listen on the Pixel (phone speaker especially: the engine's 80 Hz
  fundamental leans on its harmonics there).
- A headless `godot --quit-after` run reports an `AudioStreamPlaybackWAV`
  leaked at exit while the music plays; a bare GDScript player does the same,
  so it is Godot's dummy audio driver at quit, not the game.

## Ghost of your best run, and RETRY SEED (2026-10-18)

- A track is its key `(run_seed, index, branch)`, so a ghost can only exist on
  a track the player has finished before: in practice a retry. RETRY SEED on
  the results screen (Up then confirm, or the button) starts the same seed
  again; the cached tracks start it at once. `Game::start_seed` is the hook a
  daily seed will use. A retry's run file is `run-<seed>-<n>.txt`, so it no
  longer overwrites the first attempt's.
- `racer_sim::ghost`: when the player (never DEMO) finishes a track, the race's
  own log, the loadout and the finishing tick go to a `GhostStore` if faster
  than the stored best (a tie keeps the old one); the adapter writes it to
  `user://ghosts/ghosts.txt` (app-private, not the runs directory) via a
  temporary file and a rename. Format: a `racer-ghosts 1` header, then one
  `g <epoch> <seed 0x…> <index> <branch> <stacks> <ticks> v1 <log…>` line per
  key (`<stacks>` one digit per upgrade, `Upgrade::ALL` order). Lines from
  another `SIM_EPOCH` and lines that do not parse are dropped.
- Racing a track with a stored best runs a second `Race` (`GhostRun`) one tick
  per player tick, fed exactly as `replay::feed` would. It is replayed in full
  once first and refused unless it finishes on its stored tick, which also
  gives its gate splits up front for the HUD's `GHOST +1.2S` (green when
  ahead, red behind, two seconds after each gate). It only reads the player's
  race; `a_ghost_never_changes_the_players_run` compares state hashes every
  frame with and without one.
- Drawn as a pale (ICE/WATER/WHITE) traffic-sized car with every other pixel
  cleared in a checkerboard, placed through the same projection as roadside
  sprites: depth-sorted, crest-clipped, gone behind the camera and once it
  has finished. It is true to scale, so a ghost more than ~15 m ahead is a
  few pixels; if that reads too faintly on the phone, an outline or a
  marker over it is the next step. Golden frame `ghost_ahead` added; the
  `time_up` screen digest moved for the new button; no other golden changed.

## Layouts, Phase 1 (2026-10-18)

DESIGN §9. Data model, fork from seed, prove/stamp/verify, `RL`/`RD`
codes, four bundled courses, `just layout …`, `--racer-layout`. No editor
UI.

- **Nothing generated moved.** `solver::validate`'s body became
  `solver::check` (one pass, returning `WeakDrift` for the ladder to
  tighten) so layouts prove with the same code; the ladder calls it exactly
  as before. Golden runs pass unchanged, no `SIM_EPOCH` bump, and the
  determinism digest is still `0x4a5c1bcc8fc9fdfb` (200 runs, 432,547
  inputs, 0 divergences). `RunState` gained an unhashed `tracks` count
  (`TRACKS`, or 1 for `RunState::single`); no golden frame moved.
- **Custom-race rules**: base loadout, 1.5 s margin, default effort,
  strict bands, no repairs. `just layout survey --seeds 50` (the batch's
  seed schedule, 450 forks): 50/50, 96/100, 88/100, 89/100, 71/100 prove by
  track index (87% overall). Failures: margins 30, skill gap 17, perfect
  band 7, crash-prone 2. A 2.0 s margin would fail more of the hard tracks;
  1.0 s would let a layout's timer bite less than a run's middle track.
- **Prove cost**, this machine (Ryzen AI 7 PRO 350), release, one prove at
  a time: 56–81 ms for eleven forks across all five indices (about 0.35M
  ticks each); the bundled courses verify in 59–77 ms. With 16 proves in
  parallel the median is 108 ms (p95 138). Cheap enough that the dev flag
  re-proves every code it loads; on a phone it wants the builder thread.
- **Code sizes**: draft codes median 553 characters (p95 678, max 788),
  share codes median 572 (max 814) over the 450 forks; the bundled courses
  548–646; the pinned hand-made 240-segment layout 95. Most of a code is
  the curve and pitch channels (smoothstep bends and bump hills: second
  differences win on hills, first on bends, chosen per channel).
- **Bundled courses** (forks of small seeds, chosen for variety and a pass
  under the custom rules, then tuned): FIRST LIGHT (3/0/0, puddle), RIDGE
  RUN (5/2/1), GRAVEL PASS (7/3/0, gravel on the drift corner), LONG DUSK
  (8/4/0, one car fewer); perfect bot leaves 10.0, 9.6, 8.8 and 8.4 s.
  Remake with the ignored `layout::bundled::tests::print_codes`.
- `--racer-layout` checked headless on desktop: a share code and a layout
  file start the race (`proven: timed (verified)` / `unproven: free drive`),
  garbage leaves the title with a warning. The import step left
  `project.godot` unchanged (its stray `.import` files for docs/site images
  were deleted, not committed).
- `_typos.toml` ignores quoted base32 runs of 30+ characters (the codes).

## Track editor, Phase 2 (2026-10-18)

DESIGN §9.11 (the editor), §9.4 (length-scaled proof rules), §9.6 (run
file format 2).

- **Nothing generated moved.** The perfect-bot band became a field of
  `solver::Rules` (generated rules set `PERFECT_LEFT`), and the race cap
  became `race::race_cap` (four minutes unless the finish is past 3,840 m,
  which no generated track reaches). Golden runs pass unchanged, no
  `SIM_EPOCH` bump, the determinism digest is still `0x4a5c1bcc8fc9fdfb`
  (200 runs, 432,547 inputs, 0 divergences) and `batch --seeds 200` prints
  the same numbers as before (1,009 first draw, 24 fallback, 3.9% human run
  failure).
- **Long layouts.** Three generated first tracks end to end (7.6 km, eight
  gates) failed `PerfectBand(2500)` under the fixed 6–18 s band; two end to
  end (5.1 km) left 13–16 s and passed. With the band's top edge scaled by
  length (`perfect_band`) the 7.6 km layout proves (tested), in about
  130 ms on this machine (release, one thread). `PROOF_RULES` is 2; the
  bundled courses were restamped (only their stamps and checksums changed:
  same timers), and a Phase 1 stamp reads `Stale` and re-proves to the same
  timers (tested with FIRST LIGHT's Phase 1 code).
- **Run file format 2** carries a custom race's layout code; generated runs
  are still format 1 byte for byte. `playstats` reads both and replays
  custom races on their own track (tested end to end through the binary).
- **Golden screens.** `title` (TRACKS beside DEMO), `paused` (FORK TRACK)
  and `time_up` (FORK TRACK and the SEED line) moved; `racing_start` and
  `fork` did not; eight editor screens were added (`tracks`,
  `editor_bend`, `editor_proving`, `editor_failed`, `editor_menu`,
  `editor_save`, `editor_name`, `editor_seed`). The render goldens
  (`golden_frames`) are unchanged. `dump_screens` writes them all to
  `target/frames/screen_*.ppm`.
- **Desktop check.** `cargo build -p racer-godot`, a headless import and a
  300-frame headless run start clean (the known exit-time audio leak
  aside); `git diff project.godot` is empty after the import (its stray
  `.import` files for docs/site images were deleted again). The editor's
  input and drawing are covered engine-free; it has not been tried on the
  Pixel yet, nor the Android clipboard.
- **Gotchas.** Crockford base32 reads `O`, `I` and `L` as digits, so the
  word "origin" is a valid run of code characters: the paste search stops
  a code at a line starting `origin` or `#`, and tries shorter cuts at each
  space or line break so words after a code are not swallowed. A prefix
  inside a word (`HARD`, `WORLD`) does not start a candidate.
