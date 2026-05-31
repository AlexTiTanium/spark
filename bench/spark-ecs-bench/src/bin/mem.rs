//! Memory-footprint probe: live heap bytes retained by a populated
//! [`ENTITY_COUNT`]-entity world, per ECS.
//!
//! Uses a counting global allocator rather than OS RSS, so the number is
//! **deterministic and noise-free**: it is the exact `allocated − freed`
//! delta around building each world, i.e. the heap the world structure
//! holds. It excludes one-time global/static registrations each ECS makes
//! (component-id tables, etc.), which are counted once before the first
//! build and amortised away by the before/after delta.
//!
//! Run: `cargo run --bin mem --release --features external`
//!
//! `#![allow(unsafe_code)]` is unavoidable here: a `#[global_allocator]`
//! must implement the `unsafe` [`GlobalAlloc`] trait. The unsafe surface is
//! confined to forwarding to the system allocator, with `SAFETY` notes at
//! each call.

#![allow(unsafe_code)]
#![allow(clippy::cast_precision_loss)] // counts → f64 for the per-entity ratio

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};

use spark_ecs_bench::{ENTITY_COUNT, spark_world};

static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static FREED: AtomicUsize = AtomicUsize::new(0);

/// A pass-through allocator that tallies bytes handed out and returned.
struct Counting;

// SAFETY: every method forwards directly to the global `System` allocator
// with the same `Layout`, so all of `GlobalAlloc`'s safety requirements are
// upheld by `System`. We only add relaxed atomic counters around the calls,
// which cannot affect allocation correctness.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarding the caller's `layout` to the system allocator.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` was returned by `alloc` with this same `layout`.
        unsafe { System.dealloc(ptr, layout) };
        FREED.fetch_add(layout.size(), Ordering::Relaxed);
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Current live heap bytes (allocated minus freed).
fn live() -> usize {
    ALLOCATED
        .load(Ordering::Relaxed)
        .saturating_sub(FREED.load(Ordering::Relaxed))
}

/// Builds one world, prints the heap bytes it retains, then drops it.
fn report<W>(label: &str, build: impl FnOnce() -> W) {
    let before = live();
    let world = build();
    let bytes = live() - before;
    println!(
        "{label:<10} {bytes:>10} bytes   {:>6.1} bytes/entity",
        bytes as f64 / ENTITY_COUNT as f64
    );
    black_box(&world);
    drop(world);
}

fn main() {
    println!("Live heap bytes for a {ENTITY_COUNT}-entity (Position + Velocity) world:\n");
    report("spark-ecs", || spark_world(ENTITY_COUNT));

    #[cfg(feature = "external")]
    {
        use spark_ecs_bench::rivals;
        report("hecs", || rivals::hecs::world(ENTITY_COUNT));
        report("bevy_ecs", || rivals::bevy::world(ENTITY_COUNT));
        report("shipyard", || rivals::shipyard::world(ENTITY_COUNT));
        report("flax", || rivals::flax::world(ENTITY_COUNT));
    }

    #[cfg(not(feature = "external"))]
    println!("\n(build with --features external for the rival-ECS comparison)");
}
