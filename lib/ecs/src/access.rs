//! Component-access bookkeeping for queries (and, soon, systems).
//!
//! A [`QueryAccess`] is what a query *claims* it touches: which component
//! [`TypeId`]s it reads and which it writes. Today it does one job —
//! per-query self-conflict detection at [`Query::from_world`] time, so
//! `(&mut A, &A)` panics with a message naming the offending component
//! instead of the [`RefCell`]'s cryptic "already borrowed". Tomorrow the
//! scheduler (roadmap item 3) aggregates these per-query sets up to
//! `SystemParam` level, so the type lives outside [`crate::query`] from
//! day one.
//!
//! [`Query::from_world`]: crate::Query::from_world
//! [`RefCell`]: std::cell::RefCell

use std::any::{TypeId, type_name};

/// What a query reads and writes — one entry per component type
/// referenced in the data shape, identified by [`TypeId`] and carrying
/// its type name for diagnostics.
///
/// `&T` in a [`QueryData`](crate::QueryData) shape adds a read,
/// `&mut T` adds a write; tuples concatenate their elements' access in
/// shape order. Two entries with the same `TypeId` is a *self-conflict*
/// iff at least one is a write — [`assert_no_self_conflict`] catches it.
///
/// # Why both `TypeId` and a name
///
/// [`TypeId`] is what the scheduler will need (set operations); the name
/// is what makes the panic message readable. Capturing
/// [`type_name::<T>()`] at the impl site, where `T` is a real generic
/// parameter, is the only way to surface a component name in the
/// diagnostic — by the time [`assert_no_self_conflict`] runs the types
/// are erased.
///
/// # Examples
///
/// ```
/// use spark_ecs::QueryAccess;
///
/// struct Position(f32, f32);
/// struct Velocity(f32, f32);
///
/// let mut access = QueryAccess::default();
/// access.add_write::<Position>();
/// access.add_read::<Velocity>();
/// access.assert_no_self_conflict();   // distinct types, no conflict
/// ```
///
/// A self-conflict panics:
///
/// ```should_panic
/// use spark_ecs::QueryAccess;
///
/// struct Position(f32, f32);
///
/// let mut access = QueryAccess::default();
/// access.add_write::<Position>();
/// access.add_read::<Position>();
/// access.assert_no_self_conflict();   // panics naming `Position`
/// ```
///
/// [`assert_no_self_conflict`]: Self::assert_no_self_conflict
#[derive(Default)]
pub struct QueryAccess {
    reads: Vec<Entry>,
    writes: Vec<Entry>,
}

/// One `(TypeId, name)` pair — name captured at the impl site so the
/// panic can identify the component.
struct Entry {
    id: TypeId,
    name: &'static str,
}

impl QueryAccess {
    /// Records that the query reads `T`.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::QueryAccess;
    ///
    /// struct Health(u32);
    ///
    /// let mut access = QueryAccess::default();
    /// access.add_read::<Health>();
    /// access.assert_no_self_conflict();
    /// ```
    pub fn add_read<T: 'static>(&mut self) {
        self.reads.push(Entry {
            id: TypeId::of::<T>(),
            name: type_name::<T>(),
        });
    }

    /// Records that the query writes `T`.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::QueryAccess;
    ///
    /// struct Health(u32);
    ///
    /// let mut access = QueryAccess::default();
    /// access.add_write::<Health>();
    /// access.assert_no_self_conflict();
    /// ```
    pub fn add_write<T: 'static>(&mut self) {
        self.writes.push(Entry {
            id: TypeId::of::<T>(),
            name: type_name::<T>(),
        });
    }

    /// Panics if any single component is written and either written or
    /// read elsewhere in the same query — `(&mut A, &mut A)` or
    /// `(&mut A, &A)` in either element order. Two reads of the same
    /// `T` never conflict.
    ///
    /// Runs at [`crate::Query::from_world`] time, *before* any storage
    /// borrow. Today the `RefCell` storage would also catch the same
    /// case (as "already borrowed") on the second `init_state` call;
    /// this explicit check fires first so the diagnostic names the
    /// offending component, and stays the sole guard once M4 swaps
    /// storages to `UnsafeCell`.
    ///
    /// # Panics
    ///
    /// Panics with `"query has conflicting access to component
    /// `{name}`"` where `{name}` is the offending type's
    /// [`std::any::type_name`].
    pub fn assert_no_self_conflict(&self) {
        for (i, w) in self.writes.iter().enumerate() {
            assert!(
                !self.writes[i + 1..].iter().any(|other| other.id == w.id),
                "query has conflicting access to component `{}` (written twice)",
                w.name
            );
            assert!(
                !self.reads.iter().any(|other| other.id == w.id),
                "query has conflicting access to component `{}` (written and read)",
                w.name
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct A;
    struct B;

    #[test]
    fn empty_access_has_no_conflict() {
        QueryAccess::default().assert_no_self_conflict();
    }

    #[test]
    fn two_distinct_writes_are_fine() {
        let mut access = QueryAccess::default();
        access.add_write::<A>();
        access.add_write::<B>();
        access.assert_no_self_conflict();
    }

    #[test]
    fn write_plus_disjoint_read_is_fine() {
        let mut access = QueryAccess::default();
        access.add_write::<A>();
        access.add_read::<B>();
        access.assert_no_self_conflict();
    }

    #[test]
    fn two_reads_of_same_type_are_fine() {
        let mut access = QueryAccess::default();
        access.add_read::<A>();
        access.add_read::<A>();
        access.assert_no_self_conflict();
    }

    #[test]
    #[should_panic(expected = "written twice")]
    fn two_writes_of_same_type_panic() {
        let mut access = QueryAccess::default();
        access.add_write::<A>();
        access.add_write::<A>();
        access.assert_no_self_conflict();
    }

    #[test]
    #[should_panic(expected = "written and read")]
    fn write_then_read_of_same_type_panics() {
        let mut access = QueryAccess::default();
        access.add_write::<A>();
        access.add_read::<A>();
        access.assert_no_self_conflict();
    }

    #[test]
    #[should_panic(expected = "written and read")]
    fn read_then_write_of_same_type_panics() {
        let mut access = QueryAccess::default();
        access.add_read::<A>();
        access.add_write::<A>();
        access.assert_no_self_conflict();
    }

    #[test]
    fn panic_message_names_offending_type() {
        let mut access = QueryAccess::default();
        access.add_write::<A>();
        access.add_write::<A>();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            access.assert_no_self_conflict();
        }));
        let payload = result.expect_err("expected panic");
        let msg = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&'static str>().copied())
            .unwrap_or("");
        assert!(
            msg.contains("::A") || msg.contains("`A`"),
            "panic message did not name component `A`: {msg}"
        );
    }
}
