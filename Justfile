default:
    @just --list

# Unit, property, snapshot and golden-run tests.
test:
    cargo test --workspace

fmt:
    cargo fmt --all -- --check

lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Generate tracks across many run seeds, validate each with the bots and print
# the solvability gate report (DESIGN §2.4). Exit status 1 if any gate fails.
balance seeds="200":
    cargo run --release -p racer-sim --bin batch -- --seeds {{ seeds }}

# Cross-platform determinism digest (DESIGN §6): 200 noisy bot runs, each
# replayed from its log. CI compares the digest across platforms.
digest:
    cargo run --release -p racer-sim --bin determinism

# Look at one generated track: `just trackviz 7 2 1` prints a strip chart of
# run seed 7, track index 2, branch 1; add `--png out.png` for a picture.
trackviz seed="1" index="0" branch="0" *flags:
    cargo run --release -p racer-sim --bin trackviz -- --seed {{ seed }} --index {{ index }} --branch {{ branch }} {{ flags }}

# Check advisories, licences, bans and sources (deny.toml). Needs cargo-deny.
deny:
    cargo deny check
