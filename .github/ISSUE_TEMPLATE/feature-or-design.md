---
name: Feature or design
about: A new engine layer, plugin, milestone slice, or architectural change — anything that warrants design discussion before code.
title: ""
labels: ["design"]
---

<!--
Spark feature/design issue template. Keep top-level sections short and
crisp; expand the <details> blocks for anything that would otherwise
bloat the issue description. Required sections are marked "(required)".

A design issue captures *the plan* before any code lands. It should be
self-contained enough that a contributor can pick it up later, read it
once, and start implementing without further questions.
-->

## Context (required)

<!--
Why this change is being made. What problem does it solve? What
prompted it (incident, milestone, design discussion)? Link related
issues, PRs, design docs, or chat threads.
-->

## Goals & non-goals (required)

**Goals**
-

**Non-goals**
-

## Proposed shape (required)

<!--
The chosen approach. Code sketches are welcome. Reference existing
files with [file.rs](relative/path) links and `file.rs:line` form for
specific lines. Don't enumerate rejected alternatives unless one is
likely to be raised — keep the issue focused on the chosen design.
-->

```rust
// example sketch
```

<details>
<summary><strong>Architecture diagrams</strong> — required for structural / API / data-flow changes</summary>

<!--
At minimum one of:
  - Dependency graph (before → after)
  - Data flow / control flow
  - Module layout
  - Sequence diagram (for boot sequence, frame loop, etc.)

Use ```mermaid for mermaid; ```text for ASCII.
-->

</details>

<details>
<summary><strong>Learn — Rust and engine concepts</strong> — required for issues introducing new patterns / libs / features</summary>

<!--
Audience: a 14-year-old learning Rust + game engines. Explain every
non-obvious language feature and engine-architecture concept this design
touches. Each item gets a paragraph + (where it helps) a runnable
snippet.

Cover language features such as traits, trait objects, generics,
lifetimes, `?` + `From`, `pub use`, `#[non_exhaustive]`, `Send + Sync`,
closures, etc.

Cover architecture concepts such as plugin pattern, stages, dependency
direction, error roles (thiserror vs anyhow), workspace organisation.
-->

</details>

## File-tree diff (required for code-touching designs)

```text
<old/new tree — show ADDED, MODIFIED, DELETED files>
```

## Acceptance criteria

<!-- Mark-tickable items. What does "done" look like? -->

- [ ]
- [ ]

## Verification plan

<!-- How will we know it works? -->

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo build --workspace`
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] Manual run / smoke test:

## Forward evolution

<!--
How does this design grow when later milestones arrive? Additive-only
changes are preferred; rewrites are red flags. Use a table:

| Milestone | Adds / changes |
|---|---|
| M1 (this issue) | … |
| M2 | … |
-->

## Out of scope

<!-- Things explicitly NOT addressed here. Link the follow-up issues. -->

-
