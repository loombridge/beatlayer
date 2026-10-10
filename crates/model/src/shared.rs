//! A shared node of the document tree.

use std::ops::Deref;
use std::sync::Arc;

/// An `Arc` whose equality checks the pointer before the value.
///
/// The document keeps each track, clip, and device behind one of these, so
/// an edit replaces only the nodes it touches and every other node stays
/// shared with older versions. Two versions that share a node compare equal
/// without looking inside it, which keeps equality and change detection
/// proportional to the size of the change rather than the size of the song.
#[derive(Debug)]
pub(crate) struct Shared<T>(Arc<T>);

impl<T> Shared<T> {
    pub(crate) fn new(value: T) -> Self {
        Self(Arc::new(value))
    }

    /// Whether both point to the same node.
    pub(crate) fn ptr_eq(a: &Self, b: &Self) -> bool {
        Arc::ptr_eq(&a.0, &b.0)
    }
}

impl<T: Clone> Shared<T> {
    /// The value for editing, copied first if another version shares it.
    pub(crate) fn make_mut(&mut self) -> &mut T {
        Arc::make_mut(&mut self.0)
    }
}

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> Deref for Shared<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: PartialEq> PartialEq for Shared<T> {
    fn eq(&self, other: &Self) -> bool {
        Self::ptr_eq(self, other) || *self.0 == *other.0
    }
}
