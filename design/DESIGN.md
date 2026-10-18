# Racer — Design Spec

Working codename `racer`; the real name is picked after the feel slice (§7).
A portrait, retro pseudo-3D "behind the car" racer in which every generated
track is **provably drivable** by a human-like driver before it is shown.
Deterministic Rust simulation core, Godot 4 presentation layer via gdext.

Milestone 1 (this document's "built" parts) is the simulation and solver
spine: no renderer, no art. Sections marked *(M2)* are decided but not built.

---

## 1. Run structure

*(Filled in as the run lands, build step 8.)*

## 2. Tracks

*(Filled in as the generator lands, build steps 2 and 7.)*

## 3. The car

*(Filled in as the car lands, build step 3.)*

## 4. Upgrades

*(Filled in with the run, build step 8.)*

## 5. Art and the portrait layout *(M2)*

*(Filled in at the end of M1.)*

## 6. Architecture

```
racer-sim/              # Rust, zero Godot dependency, zero dependencies at all
  ├─ dmath.rs           # platform-stable trig (no libm)
  ├─ rng.rs             # SplitMix64 + PCG32, named substreams
  └─ hash.rs            # FNV-1a for digests
```

*(Grows with each build step.)*

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
- [ ] Employer personal-project review submitted and cleared before publishing

## 8. Determinism, replay and versioning

*(Filled in as the replay lands, build steps 4 and 8.)*
