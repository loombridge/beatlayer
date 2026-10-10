//! Stable object IDs.
//!
//! Every object in a project (a track, a clip, a device) has an [`Id`]: a
//! 64-bit number that never changes and is never reused within the project,
//! even after the object is deleted. The engine keys its nodes by these IDs,
//! so a reused ID could hand one device's state to another.
//!
//! IDs come from one [`IdAllocator`] per project, which only counts upwards.
//! The allocator is saved with the project, so IDs stay unique across saves
//! and loads.
//!
//! ```
//! use bl_core::id::IdAllocator;
//!
//! let mut ids = IdAllocator::new();
//! let a = ids.allocate();
//! let b = ids.allocate();
//! assert_ne!(a, b);
//! assert!(b > a);
//! ```

use core::fmt;
use core::num::NonZeroU64;

/// A stable 64-bit object ID, never zero.
///
/// IDs are unique within a project across every kind of object, so a track
/// and a clip never share one. They're ordered by when they were allocated.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(transparent)
)]
#[repr(transparent)]
pub struct Id(NonZeroU64);

impl Id {
    /// The ID with this raw value, or `None` for zero.
    #[must_use]
    pub const fn new(raw: u64) -> Option<Self> {
        match NonZeroU64::new(raw) {
            Some(raw) => Some(Self(raw)),
            None => None,
        }
    }

    /// The raw value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({})", self.0)
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

impl From<Id> for u64 {
    fn from(id: Id) -> u64 {
        id.get()
    }
}

/// Hands out a project's [`Id`]s, counting up from 1.
///
/// The allocator never goes backwards: undoing the creation of an object
/// doesn't return its ID. Save it with the project (it serialises as the next
/// ID it will hand out) so IDs stay unique after a reload.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(transparent)
)]
pub struct IdAllocator {
    next: NonZeroU64,
}

impl IdAllocator {
    /// An allocator whose first ID is 1.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next: NonZeroU64::MIN,
        }
    }

    /// An allocator whose first ID is `next`.
    #[must_use]
    pub const fn starting_at(next: Id) -> Self {
        Self { next: next.0 }
    }

    /// The ID that [`Self::allocate`] will return next.
    #[must_use]
    pub const fn peek(&self) -> Id {
        Id(self.next)
    }

    /// Hands out the next ID.
    ///
    /// # Panics
    ///
    /// Instead of handing out `u64::MAX`, after 2⁶⁴ − 2 IDs. At a million IDs
    /// a second that takes over half a million years.
    pub fn allocate(&mut self) -> Id {
        let id = Id(self.next);
        self.next = self.next.checked_add(1).expect("object IDs exhausted");
        id
    }

    /// Whether this allocator has handed out `id` (or skipped past it).
    #[must_use]
    pub const fn has_issued(&self, id: Id) -> bool {
        id.get() < self.next.get()
    }

    /// Moves the allocator past `id`, so it never hands `id` out. Does
    /// nothing if it's already past.
    ///
    /// A loader calls this with the largest ID in a document, in case the
    /// saved allocator is behind it.
    ///
    /// # Panics
    ///
    /// If `id` is `u64::MAX`.
    pub fn skip_past(&mut self, id: Id) {
        if !self.has_issued(id) {
            self.next = id.0.checked_add(1).expect("object IDs exhausted");
        }
    }
}

impl Default for IdAllocator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_count_up_from_one() {
        let mut ids = IdAllocator::new();
        assert_eq!(ids.peek().get(), 1);
        let first = ids.allocate();
        let second = ids.allocate();
        assert_eq!(first.get(), 1);
        assert_eq!(second.get(), 2);
        assert!(ids.has_issued(first));
        assert!(ids.has_issued(second));
        assert!(!ids.has_issued(ids.peek()));
    }

    #[test]
    fn zero_is_not_an_id() {
        assert_eq!(Id::new(0), None);
        assert_eq!(Id::new(7).map(Id::get), Some(7));
        assert_eq!(size_of::<Option<Id>>(), 8);
    }

    #[test]
    fn skip_past_only_moves_forwards() {
        let mut ids = IdAllocator::new();
        ids.skip_past(Id::new(41).unwrap());
        assert_eq!(ids.allocate().get(), 42);
        ids.skip_past(Id::new(3).unwrap());
        assert_eq!(ids.allocate().get(), 43);
    }

    #[test]
    #[should_panic(expected = "object IDs exhausted")]
    fn exhaustion_panics() {
        let mut ids = IdAllocator::starting_at(Id::new(u64::MAX - 1).unwrap());
        assert_eq!(ids.allocate().get(), u64::MAX - 1);
        ids.allocate();
    }

    #[test]
    fn display() {
        let id = Id::new(12).unwrap();
        assert_eq!(id.to_string(), "#12");
        assert_eq!(format!("{id:?}"), "Id(12)");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serialises_as_integers() {
        let mut ids = IdAllocator::new();
        let id = ids.allocate();
        assert_eq!(serde_json::to_string(&id).unwrap(), "1");
        assert_eq!(serde_json::to_string(&ids).unwrap(), "2");
        assert_eq!(serde_json::from_str::<Id>("9").unwrap().get(), 9);
        assert!(serde_json::from_str::<Id>("0").is_err());
        assert!(serde_json::from_str::<IdAllocator>("0").is_err());
    }
}
