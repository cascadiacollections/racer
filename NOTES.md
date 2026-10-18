# Build notes

Where this build had to decide something the plan did not, or where a plan
number produced a bad result. Numbers that were changed are reported here
rather than quietly retuned.

DESIGN.md is authoritative for mechanics; this file records the why behind
the numbers.

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
