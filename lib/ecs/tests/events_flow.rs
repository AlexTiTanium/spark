//! Integration coverage for the events double-buffer rotated by a *real*
//! `Schedule::run` (issue #80 Phase 0 regression net).
//!
//! The unit test in `events.rs` calls `Events::swap()` by hand and drives
//! systems directly. This pins the scheduler-integrated path: a
//! `swap_events::<T>` system registered at the *top* of the frame rotates
//! the buffer, so an event a writer sends in frame N is observed by a
//! reader in frame N+1 — the one-frame latency `EventReader`'s
//! read-previous contract promises, and the boundary the M4 parallel
//! scheduler must preserve.
//!
//! Frame layout (workloads ordered `Rotate → Emit → Consume`):
//!
//! ```text
//! frame 1:  swap(prev=[])  → send Ping(1) → read prev=[]   ⇒ Seen []
//! frame 2:  swap(prev=[1]) → send Ping(2) → read prev=[1]  ⇒ Seen [1]
//! frame 3:  swap(prev=[2]) → send Ping(3) → read prev=[2]  ⇒ Seen [1,2]
//! ```

use spark_ecs::{
    Event, EventReader, EventWriter, Events, ResMut, Resource, Schedule, WorkloadLabel, World,
    swap_events,
};

#[derive(Event)]
struct Ping(u32);

/// Collects every payload the reader observes, across all frames.
#[derive(Resource, Default)]
struct Seen(Vec<u32>);

/// A monotonically increasing send counter, so each frame's event carries a
/// distinct payload and the one-frame latency is visible in the trace.
#[derive(Resource, Default)]
struct FrameCounter(u32);

#[derive(WorkloadLabel)]
enum Phase {
    Rotate,
    Emit,
    Consume,
}

#[test]
fn writer_send_is_read_one_frame_later_through_schedule_run() {
    let mut world = World::new();
    world.add_resource(Events::<Ping>::default());
    world.add_resource(Seen::default());
    world.add_resource(FrameCounter::default());

    let mut schedule = Schedule::new();
    // `swap_events` runs first (top of frame), so a reader always observes
    // the *previous* frame's sends. The `.after` edges declare the
    // write/write (Rotate↔Emit) and write/read (Emit↔Consume) conflicts on
    // `Events<Ping>`, so the conflict policy is satisfied without a panic.
    schedule.add_workload(Phase::Rotate, |w| {
        w.add_system(swap_events::<Ping>);
    });
    schedule
        .add_workload(Phase::Emit, |w| {
            // Sends one `Ping` per frame, numbered `1, 2, 3, …`.
            w.add_system(
                |mut counter: ResMut<FrameCounter>, mut writer: EventWriter<Ping>| {
                    counter.0 += 1;
                    writer.send(Ping(counter.0));
                },
            );
        })
        .after(Phase::Rotate);
    schedule
        .add_workload(Phase::Consume, |w| {
            // Records whatever the reader sees this frame (the *previous* buffer).
            w.add_system(|reader: EventReader<Ping>, mut seen: ResMut<Seen>| {
                seen.0.extend(reader.read().map(|p| p.0));
            });
        })
        .after(Phase::Emit);

    // Frame 1: send 1, read previous (empty).
    schedule.run(&mut world);
    assert_eq!(world.resource::<Seen>().0, Vec::<u32>::new());

    // Frame 2: send 2, read previous (= frame 1's send).
    schedule.run(&mut world);
    assert_eq!(world.resource::<Seen>().0, vec![1]);

    // Frame 3: send 3, read previous (= frame 2's send) — and crucially
    // *not* an accumulation of frames 1+2, proving the buffer holds exactly
    // one frame.
    schedule.run(&mut world);
    assert_eq!(world.resource::<Seen>().0, vec![1, 2]);
}
