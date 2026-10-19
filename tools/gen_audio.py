#!/usr/bin/env python3
"""Generate racer's sound effects and title music (art/audio/*.wav).

Modelled on Salt Line's tools/gen_audio.py: Python standard library only, no
samples, downloads or third-party material; every sound is synthesised here.

The output is byte-for-byte reproducible on any machine. Only IEEE-exact
arithmetic is used (+, -, *, /, sqrt, floor, ldexp); sine and exp are the
series below, not libm's, whose last bit may differ between C libraries.
`--check` regenerates into memory and fails if a committed file differs (CI
and `just check-audio`).

Loops (the engine, the tyres, the title music) are seamless and carry a RIFF
`smpl` chunk with a forward loop over the whole file, which Godot's WAV
importer reads with its default `edit/loop_mode` ("Detect From WAV"), so the
loop survives a re-import with default settings. Their noise is made
seamless by an equal-power crossfade of the tail into the head; their tones
fit a whole number of cycles in the loop.

    python3 tools/gen_audio.py           # write art/audio/*.wav
    python3 tools/gen_audio.py --check   # exit 1 if any committed file differs
"""

import math
import pathlib
import struct
import sys

RATE = 22050
MUSIC_RATE = 16000
OUT = pathlib.Path(__file__).resolve().parents[1] / "art" / "audio"

PI = 3.141592653589793
TAU = 6.283185307179586
LN2 = 0.6931471805599453


# ---------------------------------------------------------------- exact math


def sin_cycles(c):
    """sin(2*pi*c), from the fractional part of `c` cycles. A Taylor series
    on [-pi/2, pi/2]: only +, * and /, so identical everywhere."""
    f = c - math.floor(c)
    x = f * TAU
    if x > PI:
        x -= TAU
    if x > PI / 2:
        x = PI - x
    elif x < -PI / 2:
        x = -PI - x
    x2 = x * x
    return x * (1 - x2 / 6 * (1 - x2 / 20 * (1 - x2 / 42 * (1 - x2 / 72 * (
        1 - x2 / 110 * (1 - x2 / 156 * (1 - x2 / 210)))))))


def dexp(x):
    """exp(x) by range reduction to [-ln2/2, ln2/2] and a series."""
    if x < -60:
        return 0.0
    n = math.floor(x / LN2 + 0.5)
    r = x - n * LN2
    term, total = 1.0, 1.0
    for k in range(1, 14):
        term = term * r / k
        total += term
    return math.ldexp(total, int(n))


def midi(n):
    """Equal-tempered frequency of MIDI note `n` (A4 = 69 = 440 Hz)."""
    return 440.0 * dexp((n - 69) * LN2 / 12)


class Noise:
    """A 32-bit LCG: white noise in [-1, 1]."""

    def __init__(self, seed):
        self.seed = seed & 0xFFFFFFFF

    def __call__(self):
        self.seed = (1664525 * self.seed + 1013904223) & 0xFFFFFFFF
        return (self.seed >> 8) / 0x7FFFFF - 1.0


class Lowpass:
    """A one-pole low-pass: `a` is the share of the gap closed per sample."""

    def __init__(self, a):
        self.a = a
        self.y = 0.0

    def __call__(self, x):
        self.y += self.a * (x - self.y)
        return self.y


class Osc:
    """A phase accumulator in cycles, for tones whose pitch moves."""

    def __init__(self, rate):
        self.rate = rate
        self.phase = 0.0

    def step(self, freq):
        p = self.phase
        self.phase += freq / self.rate
        return p


def decay(t, d, k=4.0):
    """An exponential fall to about e^-k over `d` seconds."""
    return dexp(-k * t / d)


def attack(t, a):
    return 1.0 if t >= a else t / a


def tri(c):
    f = c - math.floor(c)
    return 4.0 * abs(f - 0.5) - 1.0


def blep(t, dt):
    """PolyBLEP correction for a step at phase 0 (t in [0, 1))."""
    if t < dt:
        t /= dt
        return t + t - t * t - 1.0
    if t > 1.0 - dt:
        t = (t - 1.0) / dt
        return t * t + t + t + 1.0
    return 0.0


def pulse(c, duty, dt):
    """A band-limited pulse wave, `duty` high, at `c` cycles."""
    f = c - math.floor(c)
    v = 1.0 if f < duty else -1.0
    v += blep(f, dt)
    g = f - duty
    if g < 0:
        g += 1.0
    v -= blep(g, dt)
    return v


def loop_crossfade(x, frames):
    """`x` has `frames` + F samples; fold the last F into the first F with an
    equal-power fade, so sample frames-1 runs on into sample 0."""
    fade = len(x) - frames
    y = x[:frames]
    for i in range(fade):
        w = i / fade
        y[i] = x[i] * math.sqrt(w) + x[frames + i] * math.sqrt(1.0 - w)
    return y


# ---------------------------------------------------------------- the file


def wav_bytes(samples, rate, peak, loop):
    """16-bit mono PCM scaled so its loudest sample is `peak`. A one-shot
    fades out over its last 5 ms, so it never ends on a click."""
    samples = list(samples)
    if not loop:
        tail = min(len(samples), rate // 200)
        for k in range(tail):
            samples[len(samples) - 1 - k] *= k / tail
    gain = peak / max(abs(v) for v in samples)
    pcm = bytearray()
    for v in samples:
        v = max(-1.0, min(1.0, v * gain))
        pcm += struct.pack("<h", int(v * 32767))
    frames = len(samples)
    fmt = struct.pack("<HHIIHH", 1, 1, rate, rate * 2, 2, 16)
    chunks = b"fmt " + struct.pack("<I", len(fmt)) + fmt
    chunks += b"data" + struct.pack("<I", len(pcm)) + bytes(pcm)
    if loop:
        # smpl: manufacturer, product, sample period (ns), MIDI unity note,
        # pitch fraction, SMPTE format, SMPTE offset, loop count, sampler
        # data; then one loop: cue id, type 0 (forward), start, end, fraction,
        # play count 0 (forever). Godot treats `end` as the frame the loop
        # jumps back from, so it is the frame count: every sample plays.
        smpl = struct.pack("<9I", 0, 0, 1_000_000_000 // rate, 60, 0, 0, 0, 1, 0)
        smpl += struct.pack("<6I", 0, 0, 0, frames, 0, 0)
        chunks += b"smpl" + struct.pack("<I", len(smpl)) + smpl
    return b"RIFF" + struct.pack("<I", 4 + len(chunks)) + b"WAVE" + chunks


def render(seconds, voice, rate=RATE, seed=0x52414345):
    """`voice(t, n)` per sample, `n` a fresh noise value."""
    noise = Noise(seed)
    count = int(rate * seconds)
    return [voice(i / rate, noise()) for i in range(count)]


# ---------------------------------------------------------------- the sounds


def engine():
    """Half a second of engine at idle pitch: an 80 Hz buzz (twelve
    harmonics, falling 1/h, so a phone speaker hears the upper ones), a
    20 Hz firing pulse, and a little low-passed noise. The node plays it
    from pitch 0.55 to about 2.2 with the car's speed."""
    frames = RATE // 2
    fade = RATE // 20
    noise = Noise(0xE9)
    lp = Lowpass(0.08)
    # Only the noise needs the fold: the buzz repeats every 1/80 s, and
    # folding it too would swell it where the fade is.
    hiss = loop_crossfade([lp(noise()) for _ in range(frames + fade)], frames)
    out = []
    for i in range(frames):
        t = i / RATE
        buzz = 0.0
        for h in range(1, 13):
            buzz += sin_cycles(80 * h * t + 0.13 * h * h) / h
        fire = 0.75 + 0.25 * sin_cycles(20 * t)
        out.append(0.42 * buzz * fire + 0.9 * hiss[i])
    return out


def drift():
    """0.6 s of tyre screech: a squeal near 1.3 kHz wavering at 5 Hz, a
    fifth above it, and hissing band-limited noise, fluttering at 10 Hz."""
    frames = int(RATE * 0.6)
    fade = RATE // 20
    noise = Noise(0xD1F7)
    lp1, lp2 = Lowpass(0.5), Lowpass(0.05)
    band = []
    for _ in range(frames + fade):
        n = noise()
        band.append(lp1(n) - lp2(n))
    hiss = loop_crossfade(band, frames)
    out = []
    for i in range(frames):
        t = i / RATE
        # Phase of 1300 + 60*sin(2*pi*5t) Hz, integrated: whole cycles in
        # 0.6 s for both the carrier (780) and the vibrato (3).
        wobble = 60 / (TAU * 5) * (1 - sin_cycles(5 * t + 0.25))
        squeal = sin_cycles(1300 * t + wobble) + 0.35 * sin_cycles(1950 * t + 1.5 * wobble)
        flutter = 0.8 + 0.2 * sin_cycles(10 * t)
        out.append(flutter * (0.32 * squeal + 0.7 * hiss[i]))
    return out


def whoosh(seconds, f0, f1, shimmer, seed):
    """A boost: noise opening up through a rising low-pass under a rising
    tone. `shimmer` adds a bright octave that sparkles."""
    noise = Noise(seed)
    lp = Lowpass(0.02)
    osc = Osc(RATE)
    out = []
    for i in range(int(RATE * seconds)):
        t = i / RATE
        u = t / seconds
        lp.a = 0.02 + 0.3 * u
        f = f0 + (f1 - f0) * u * (2 - u)
        c = osc.step(f)
        env = attack(t, 0.04) * decay(t, seconds, 3.2)
        tone = sin_cycles(c) + 0.3 * sin_cycles(2 * c)
        if shimmer:
            tone += 0.25 * sin_cycles(4 * c) * (0.5 + 0.5 * sin_cycles(18 * t))
        out.append(env * (0.9 * lp(noise()) + 0.35 * tone))
    return out


def notes(t, pitches, step, tail, voice):
    """`pitches` (Hz) one after another, `step` seconds apart, each ringing
    over `step` (the last over `tail`), played by `voice(f, t)`."""
    i = min(int(t / step), len(pitches) - 1)
    local = t - i * step
    ring = tail if i == len(pitches) - 1 else step * 1.6
    return attack(local, 0.004) * decay(local, ring, 3.5) * voice(pitches[i], t)


def bell(f, t):
    return 0.6 * sin_cycles(f * t) + 0.2 * sin_cycles(2 * f * t) + 0.08 * sin_cycles(3.01 * f * t)


def square(f, t):
    return 0.5 * pulse(f * t, 0.5, f / RATE)


def chime():
    """A gate: two bright bell notes, E6 then B6."""
    return render(0.42, lambda t, n: notes(t, (midi(88), midi(95)), 0.07, 0.34, bell))


def tick():
    """The last five seconds: a short woodblock tick."""
    return render(0.06, lambda t, n: decay(t, 0.06, 5) * (0.7 * sin_cycles(1760 * t) + 0.15 * n))


def click():
    """A menu choice: a soft, low click."""
    return render(0.045, lambda t, n: attack(t, 0.002) * decay(t, 0.045, 5) * 0.6 * sin_cycles(700 * t))


def time_up():
    """Out of time: a falling square-wave sting, G5 E5 C5 then a sagging G4."""
    def voice(t, n):
        i = min(int(t / 0.13), 3)
        local = t - 0.13 * i
        f = midi((79, 76, 72, 67)[i])
        if i == 3:
            f *= 1.0 - 0.06 * min(1.0, local / 0.6)
        ring = 0.62 if i == 3 else 0.2
        return attack(local, 0.005) * decay(local, ring, 3) * (square(f, t) + 0.3 * bell(f / 2, t))
    return render(1.0, voice)


def win():
    """The run won: a rising C-major arpeggio into a held, shimmering chord."""
    arp = (72, 76, 79, 84)

    def voice(t, n):
        if t < 0.4:
            return notes(t, [midi(p) for p in arp], 0.1, 0.1, lambda f, tt: square(f, tt) + 0.4 * bell(f, tt))
        local = t - 0.4
        env = attack(local, 0.01) * decay(local, 1.0, 3)
        vib = 1 + 0.004 * sin_cycles(5.5 * t)
        chord = sum(0.3 * square(midi(p) * vib, t) + 0.25 * bell(midi(p), t) for p in (72, 76, 79))
        return env * 0.7 * chord
    return render(1.4, voice)


def spin():
    """A spin: a low thud falling from 95 to 45 Hz, a crunch of noise, then
    a skid of band-limited noise that dies away."""
    noise = Noise(0x5917)
    lp1, lp2 = Lowpass(0.35), Lowpass(0.04)
    osc = Osc(RATE)
    out = []
    seconds = 0.75
    for i in range(int(RATE * seconds)):
        t = i / RATE
        f = 45 + 50 * decay(t, 0.2, 3)
        thud = decay(t, 0.25, 4) * sin_cycles(osc.step(f))
        n = noise()
        crunch = decay(t, 0.05, 4) * n
        skid_env = attack(max(0.0, t - 0.04), 0.06) * decay(t, seconds, 2.5)
        skid = skid_env * (lp1(n) - lp2(n)) * (0.8 + 0.2 * sin_cycles(13 * t))
        out.append(0.9 * thud + 0.45 * crunch + 0.8 * skid)
    return out


# The title loop: 120 bpm, eight bars of A minor, F, C, G, twice through
# with a varied second half. Melody bars as (MIDI note or None, eighths).
BPM = 120
EIGHTH = 60 / BPM / 2
MELODY = [
    [(76, 2), (72, 1), (76, 1), (81, 2), (79, 1), (76, 1)],
    [(77, 2), (81, 2), (84, 2), (81, 1), (77, 1)],
    [(79, 3), (76, 1), (72, 2), (76, 2)],
    [(74, 2), (79, 2), (83, 3), (None, 1)],
    [(81, 1), (79, 1), (76, 2), (81, 1), (83, 1), (84, 2)],
    [(81, 2), (77, 2), (72, 2), (77, 1), (81, 1)],
    [(79, 2), (76, 1), (79, 1), (84, 2), (83, 1), (81, 1)],
    [(79, 4), (74, 2), (None, 2)],
]
ROOTS = [45, 41, 48, 43]  # A2, F2, C3, G2


def music():
    """About sixteen seconds of chiptune for the title: a quarter-duty pulse
    lead, a triangle bass in octaves, and a soft off-beat hat. Every note
    ends inside the bar, so the loop seam is clean."""
    events = []  # (start s, length s, note, voice)
    at = 0.0
    for bar in MELODY:
        for note, eighths in bar:
            if note is not None:
                events.append((at, eighths * EIGHTH * 0.88, note, "lead"))
            at += eighths * EIGHTH
    total_eighths = 8 * len(MELODY)
    for e in range(total_eighths):
        root = ROOTS[(e // 8) % 4]
        events.append((e * EIGHTH, EIGHTH * 0.8, root + (12 if e % 2 else 0), "bass"))
    frames = int(round(total_eighths * EIGHTH * MUSIC_RATE))
    out = [0.0] * frames
    for start, length, note, kind in events:
        f = midi(note)
        a = int(round(start * MUSIC_RATE))
        b = min(frames, a + int(length * MUSIC_RATE))
        dt = f / MUSIC_RATE
        for i in range(a, b):
            local = (i - a) / MUSIC_RATE
            gate = min(1.0, local / 0.005, (length - local) / 0.02)
            c = f * local
            if kind == "lead":
                v = 0.17 * gate * (0.85 + 0.15 * decay(local, length, 2)) * pulse(c, 0.25, dt)
            else:
                v = 0.26 * gate * tri(c)
            out[i] += v
    noise = Noise(0x4A7)
    lp = Lowpass(0.4)
    hat_len = int(0.03 * MUSIC_RATE)
    for e in range(1, total_eighths, 2):
        a = int(round(e * EIGHTH * MUSIC_RATE))
        for k in range(hat_len):
            n = noise()
            out[a + k] += 0.06 * decay(k / MUSIC_RATE, 0.03, 4) * (n - lp(n))
    return out


SOUNDS = [
    # name, builder, rate, peak, loop. The peaks are the mix: the node plays
    # everything at 0 dB but the loops, which it sets from the car.
    ("engine", engine, RATE, 0.55, True),
    ("drift", drift, RATE, 0.45, True),
    ("boost_mini", lambda: whoosh(0.45, 260, 820, False, 0xB0), RATE, 0.6, False),
    ("boost_super", lambda: whoosh(0.85, 180, 1250, True, 0xB1), RATE, 0.72, False),
    ("chime", chime, RATE, 0.6, False),
    ("tick", tick, RATE, 0.5, False),
    ("click", click, RATE, 0.4, False),
    ("time_up", time_up, RATE, 0.7, False),
    ("win", win, RATE, 0.7, False),
    ("spin", spin, RATE, 0.8, False),
    ("music", music, MUSIC_RATE, 0.5, True),
]


def build():
    return {name: wav_bytes(make(), rate, peak, loop) for name, make, rate, peak, loop in SOUNDS}


def main():
    check = "--check" in sys.argv[1:]
    stale = []
    for name, data in build().items():
        path = OUT / f"{name}.wav"
        if check:
            if not path.is_file() or path.read_bytes() != data:
                stale.append(path.name)
            continue
        OUT.mkdir(parents=True, exist_ok=True)
        if not path.is_file() or path.read_bytes() != data:
            path.write_bytes(data)
        print(f"{path.relative_to(OUT.parents[1])}  {len(data)} bytes")
    if stale:
        print("art/audio differs from tools/gen_audio.py: " + ", ".join(stale)
              + " (run `just gen-audio` and commit)", file=sys.stderr)
        sys.exit(1)
    if check:
        print(f"art/audio matches tools/gen_audio.py ({len(SOUNDS)} files)")


if __name__ == "__main__":
    main()
