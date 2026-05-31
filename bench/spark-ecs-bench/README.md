# spark-ecs-bench

Benchmark harness that measures [`spark-ecs`](../../lib/ecs) against rival ECS
crates on representative workloads.

> **What is this for?** A benchmark number means nothing on its own — the value
> is the *delta over time*. This crate records where `spark-ecs` sits today
> (single-threaded, post-PR #62) so that after M4 adds Rayon parallelism, and
> again after the Stage 24 archetype refactor, we can re-run the *same*
> instrument on the *same* machine and attribute each change to its cause. It is
> diagnostic, not promotional.

It is the first of a `spark-<crate>-bench` family; future per-subsystem benches
(`spark-render-bench`, …) follow the same shape.

## Why it lives outside the workspace

This crate is **excluded from the root Cargo workspace** (see `exclude` in the
root `Cargo.toml`). A benchmark harness is tooling, not an engine layer, and its
comparison dependencies (`bevy_ecs`, `hecs`) have nothing to do with the engine's
runtime dependency graph. Excluding it guarantees those heavy crates never:

- compile as part of `cargo build --workspace`,
- appear in the engine's `Cargo.lock`,
- get pulled in by a `--all-features` CI job.

The trade-off: as a standalone crate it can't inherit `[workspace.package]` /
`[workspace.lints]`, so `edition`, `rust-version`, `license`, and the lints are
spelled out in its own `Cargo.toml` and kept in sync with the root by hand. It
also has its own committed `Cargo.lock`, which pins the exact rival-ECS versions
behind each baseline.

The only edge between this crate and the engine is a one-way path dependency:
`spark-ecs-bench → spark-ecs`. Nothing in the engine depends on the bench.

## How to run

Run from anywhere in the repo. Because the crate is outside the workspace, select
it by manifest path rather than `-p`:

```bash
# Spark-only timing suite — fast inner loop, no rival ECS fetched.
cargo bench --manifest-path bench/spark-ecs-bench/Cargo.toml

# Full cross-ECS timing sweep — pulls the rival crates (first build is slow).
cargo bench --manifest-path bench/spark-ecs-bench/Cargo.toml --features external

# Memory footprint (live heap bytes per 10k-entity world):
cargo run --manifest-path bench/spark-ecs-bench/Cargo.toml --bin mem --release --features external
```

Or `cd bench/spark-ecs-bench` first and drop the `--manifest-path` flag.

Criterion writes an HTML report to `bench/spark-ecs-bench/target/criterion/`
(open `report/index.html`); the terminal prints the median ± confidence interval
and throughput per bench.

## What is measured

Four metric axes over a 10 000-entity "movement" workload — each entity carries
a `Position(f32, f32, f32)` and a `Velocity(f32, f32, f32)`:

| Metric            | What                                                        | Tool                          |
| ----------------- | ----------------------------------------------------------- | ----------------------------- |
| **time**          | `spawn` / `iter` / `iter_mut` (storage, read, write paths)  | Criterion (`benches/micro.rs`) |
| **throughput**    | entities/s, reported alongside time                         | Criterion                     |
| **memory**        | live heap bytes a populated world retains                   | counting allocator (`src/bin/mem.rs`) |
| **dependency weight** | transitive crates each ECS pulls in                     | `cargo tree -e normal`        |

The three timing benches:

| Bench      | Work                                            | Hot path exercised                          |
| ---------- | ----------------------------------------------- | ------------------------------------------- |
| `spawn`    | build 10 000 two-component entities, fresh world | storage allocation                          |
| `iter`     | read-only sum over `&Position`                  | query traversal (read)                      |
| `iter_mut` | `pos += vel` over `(&mut Position, &Velocity)`  | mutable write path (`spark-ecs`: `Mut<T>` change-marker from PR #62) |

**Compared crates** (all under `--features external`): `spark-ecs` (always),
`bevy_ecs`, `hecs`, `shipyard`, `flax`, and `legion`. Architecture spread —
sparse-set: `spark-ecs`, `shipyard`; archetype: `bevy_ecs`, `hecs`, `flax`,
`legion`. `legion` 0.4 is unmaintained since 2021 (reference point only). The
remaining micros (`despawn`, filters, change detection, commands, resources)
and the macro / Spark-realistic scenarios stay deferred to follow-up PRs
against issue #63.

> **Note on `iter_mut` fairness.** The benched ECS differ in default
> modification tracking: `spark-ecs` and `bevy_ecs`/`flax` stamp a change tick
> on write; `hecs`, `legion`, and `shipyard`'s default `ViewMut` do not. So
> `iter_mut` compares each crate's *default* mutable iteration, which is the
> honest out-of-the-box path — not an identical one.

## Methodology

- **Criterion** for sampling, warm-up, and confidence intervals.
- `iter` / `iter_mut` build the populated world **and** the query/state *outside*
  the timed closure wherever the borrow model allows, so only traversal is timed.
  This is deliberately fair to `bevy_ecs`, whose `QueryState` is meant to be cached
  and reused across frames. Two crates can't hoist and are re-issued per sample
  (commented at their call sites): `hecs::query_mut` (`&mut World`) and
  `flax::Query::borrow`.
- `bevy_ecs` is depended on **without** the surrounding app/render layer, so the
  comparison is storage + query against storage + query — apples to apples.
- **memory** is a counting-allocator delta (`allocated − freed` around each build),
  not OS RSS — deterministic and noise-free.
- One machine, one toolchain per result file; CPU / RAM / OS / kernel and
  `rustc --version` are recorded in the result's YAML front-matter.
- Manual, local, quiet-system runs only — **never in CI**. Shared CI runners have
  too much hardware variance for trend numbers to mean anything.

## Results

Result files live in [`results/`](results/), named `YYYY-MM-DD-<sha>.md`. Each
carries machine-readable provenance (YAML front-matter), a human narrative, and
Mermaid bar charts. The latest:

- [`results/2026-05-31-bbb0e5d.md`](results/2026-05-31-bbb0e5d.md) — baseline,
  post-PR #62, single-threaded: six ECS across time, throughput, memory, and
  dependency weight.

## Pitfalls

- **First `--features external` build is slow.** `bevy_ecs` is a large
  dependency tree. This is expected and is exactly why the crate is excluded from
  the default workspace build.
- **Numbers are not portable across machines.** Only compare result files that
  share the same `machine` and `toolchain` front-matter. The point is the *trend*
  on one machine, not absolute nanoseconds.
- **`iter_mut` lets `Position` drift.** Each timed iteration adds velocity to
  position without resetting, so values grow unbounded over a Criterion run. The
  per-iteration work is constant regardless, so timing is unaffected; the final
  position values are meaningless by design.
