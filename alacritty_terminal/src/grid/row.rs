//! Defines the Row type which makes up lines in the grid.

use std::cmp::{max, min};
use std::ops::{Index, IndexMut, Range, RangeFrom, RangeFull, RangeTo, RangeToInclusive};
use std::sync::atomic::{AtomicBool, Ordering};
use std::{mem, ptr, slice};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::grid::GridCell;
use crate::index::Column;
use crate::term::cell::ResetDiscriminant;

/// Measurement scaffolding: picks between the bulk row clear and the reset
/// per cell it replaced.  Remove once the two have been compared.
pub static BULK_RESET: AtomicBool = AtomicBool::new(true);

/// A row in the grid.
#[derive(Default, Clone, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Row<T> {
    inner: Vec<T>,

    /// Maximum number of occupied entries.
    ///
    /// This is the upper bound on the number of elements in the row, which have been modified
    /// since the last reset. All cells after this point are guaranteed to be equal.
    pub(crate) occ: usize,

    /// Whether any cell in the row may own heap storage.
    ///
    /// Conservative: false means no cell owns anything, which is what lets
    /// [`Row::reset`] clear the row as one bulk copy instead of running a
    /// destructor per cell. Anything giving a cell storage has to announce it
    /// through [`Row::mark_owns_storage`]; a debug build checks the claim on
    /// every reset.
    #[cfg_attr(feature = "serde", serde(skip, default = "owns_storage_on_load"))]
    owns_storage: bool,
}

/// Nothing in a serialized row says whether its cells own storage, so a row
/// read back from one is assumed to.
#[cfg(feature = "serde")]
fn owns_storage_on_load() -> bool {
    true
}

impl<T: PartialEq> PartialEq for Row<T> {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl<T: Default> Row<T> {
    /// Create a new terminal row.
    ///
    /// Ideally the `template` should be `Copy` in all performance sensitive scenarios.
    pub fn new(columns: usize) -> Row<T> {
        debug_assert!(columns >= 1);

        let mut inner: Vec<T> = Vec::with_capacity(columns);

        // This is a slightly optimized version of `std::vec::Vec::resize`.
        unsafe {
            let mut ptr = inner.as_mut_ptr();

            for _ in 1..columns {
                ptr::write(ptr, T::default());
                ptr = ptr.offset(1);
            }
            ptr::write(ptr, T::default());

            inner.set_len(columns);
        }

        Row { inner, occ: 0, owns_storage: false }
    }

    /// Increase the number of columns in the row.
    #[inline]
    pub fn grow(&mut self, columns: usize) {
        if self.inner.len() >= columns {
            return;
        }

        self.inner.resize_with(columns, T::default);
    }

    /// Reduce the number of columns in the row.
    ///
    /// This will return all non-empty cells that were removed.
    pub fn shrink(&mut self, columns: usize) -> Option<Vec<T>>
    where
        T: GridCell,
    {
        if self.inner.len() <= columns {
            return None;
        }

        // Split off cells for a new row.
        let mut new_row = self.inner.split_off(columns);
        let index = new_row.iter().rposition(|c| !c.is_empty()).map_or(0, |i| i + 1);
        new_row.truncate(index);

        self.occ = min(self.occ, columns);

        if new_row.is_empty() { None } else { Some(new_row) }
    }

    /// Reset all cells in the row to the `template` cell.
    #[inline]
    pub fn reset<D>(&mut self, template: &T)
    where
        T: ResetDiscriminant<D> + GridCell,
        D: PartialEq,
    {
        debug_assert!(!self.inner.is_empty());

        // Mark all cells as dirty if template cell changed.
        let len = self.inner.len();
        if self.inner[len - 1].discriminant() != template.discriminant() {
            self.occ = len;
        }

        let occ = mem::replace(&mut self.occ, 0);
        if occ == 0 {
            self.owns_storage = false;
            return;
        }
        let cells = &mut self.inner[..occ];

        debug_assert!(
            self.owns_storage || !cells.iter().any(GridCell::owns_storage),
            "row holds owned storage without having announced it",
        );

        // Clearing rows is most of what scrolling costs, and a reset per cell
        // compiles to a load, a branch on whatever the cell owns and a store.
        // Ordinary text owns nothing, so stamping a single blank across the
        // row turns the whole clear into a memcpy.
        let owned = mem::replace(&mut self.owns_storage, false);
        if !BULK_RESET.load(Ordering::Relaxed) || owned {
            for item in cells {
                item.reset(template);
            }
            return;
        }

        let mut blank = T::default();
        blank.reset(template);

        // SAFETY: no cell in `cells` owns storage, so overwriting one without
        // dropping it leaks nothing, and `blank` owns none either, which makes
        // duplicating its bits sound.  Doubling the written prefix keeps every
        // copy inside the slice with its source and destination disjoint.
        unsafe {
            let head = cells.as_mut_ptr();
            ptr::write(head, blank);
            let mut written = 1;
            while written < occ {
                let count = min(written, occ - written);
                ptr::copy_nonoverlapping(head, head.add(written), count);
                written += count;
            }
        }
    }
}

#[allow(clippy::len_without_is_empty)]
impl<T> Row<T> {
    #[inline]
    pub fn from_vec(vec: Vec<T>, occ: usize) -> Row<T> {
        Row { inner: vec, occ, owns_storage: true }
    }

    /// Record that a cell in this row may own heap storage.
    ///
    /// Anything that writes a cell's owned storage has to call this, or the
    /// next [`Row::reset`] will clear the row without running the destructor
    /// and leak it.
    #[inline]
    pub fn mark_owns_storage(&mut self) {
        self.owns_storage = true;
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    #[inline]
    pub fn last(&self) -> Option<&T> {
        self.inner.last()
    }

    #[inline]
    pub fn last_mut(&mut self) -> Option<&mut T> {
        self.occ = self.inner.len();
        self.inner.last_mut()
    }

    // Cells arriving from another row bring whatever it owned with them, and
    // reflow is rare enough that assuming the worst costs nothing.
    #[inline]
    pub fn append(&mut self, vec: &mut Vec<T>)
    where
        T: GridCell,
    {
        self.occ += vec.len();
        self.owns_storage = true;
        self.inner.append(vec);
    }

    #[inline]
    pub fn append_front(&mut self, mut vec: Vec<T>) {
        self.occ += vec.len();
        self.owns_storage = true;

        vec.append(&mut self.inner);
        self.inner = vec;
    }

    /// Check if all cells in the row are empty.
    #[inline]
    pub fn is_clear(&self) -> bool
    where
        T: GridCell,
    {
        self.inner.iter().all(GridCell::is_empty)
    }

    #[inline]
    pub fn front_split_off(&mut self, at: usize) -> Vec<T> {
        self.occ = self.occ.saturating_sub(at);

        let mut split = self.inner.split_off(at);
        std::mem::swap(&mut split, &mut self.inner);
        split
    }
}

impl<'a, T> IntoIterator for &'a Row<T> {
    type IntoIter = slice::Iter<'a, T>;
    type Item = &'a T;

    #[inline]
    fn into_iter(self) -> slice::Iter<'a, T> {
        self.inner.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut Row<T> {
    type IntoIter = slice::IterMut<'a, T>;
    type Item = &'a mut T;

    #[inline]
    fn into_iter(self) -> slice::IterMut<'a, T> {
        self.occ = self.len();
        self.inner.iter_mut()
    }
}

impl<T> Index<Column> for Row<T> {
    type Output = T;

    #[inline]
    fn index(&self, index: Column) -> &T {
        &self.inner[index.0]
    }
}

impl<T> IndexMut<Column> for Row<T> {
    #[inline]
    fn index_mut(&mut self, index: Column) -> &mut T {
        self.occ = max(self.occ, *index + 1);
        &mut self.inner[index.0]
    }
}

impl<T> Index<Range<Column>> for Row<T> {
    type Output = [T];

    #[inline]
    fn index(&self, index: Range<Column>) -> &[T] {
        &self.inner[(index.start.0)..(index.end.0)]
    }
}

impl<T> IndexMut<Range<Column>> for Row<T> {
    #[inline]
    fn index_mut(&mut self, index: Range<Column>) -> &mut [T] {
        self.occ = max(self.occ, *index.end);
        &mut self.inner[(index.start.0)..(index.end.0)]
    }
}

impl<T> Index<RangeTo<Column>> for Row<T> {
    type Output = [T];

    #[inline]
    fn index(&self, index: RangeTo<Column>) -> &[T] {
        &self.inner[..(index.end.0)]
    }
}

impl<T> IndexMut<RangeTo<Column>> for Row<T> {
    #[inline]
    fn index_mut(&mut self, index: RangeTo<Column>) -> &mut [T] {
        self.occ = max(self.occ, *index.end);
        &mut self.inner[..(index.end.0)]
    }
}

impl<T> Index<RangeFrom<Column>> for Row<T> {
    type Output = [T];

    #[inline]
    fn index(&self, index: RangeFrom<Column>) -> &[T] {
        &self.inner[(index.start.0)..]
    }
}

impl<T> IndexMut<RangeFrom<Column>> for Row<T> {
    #[inline]
    fn index_mut(&mut self, index: RangeFrom<Column>) -> &mut [T] {
        self.occ = self.len();
        &mut self.inner[(index.start.0)..]
    }
}

impl<T> Index<RangeFull> for Row<T> {
    type Output = [T];

    #[inline]
    fn index(&self, _: RangeFull) -> &[T] {
        &self.inner[..]
    }
}

impl<T> IndexMut<RangeFull> for Row<T> {
    #[inline]
    fn index_mut(&mut self, _: RangeFull) -> &mut [T] {
        self.occ = self.len();
        &mut self.inner[..]
    }
}

impl<T> Index<RangeToInclusive<Column>> for Row<T> {
    type Output = [T];

    #[inline]
    fn index(&self, index: RangeToInclusive<Column>) -> &[T] {
        &self.inner[..=(index.end.0)]
    }
}

impl<T> IndexMut<RangeToInclusive<Column>> for Row<T> {
    #[inline]
    fn index_mut(&mut self, index: RangeToInclusive<Column>) -> &mut [T] {
        self.occ = max(self.occ, *index.end + 1);
        &mut self.inner[..=(index.end.0)]
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use vte::ansi::{Color, NamedColor};

    use super::*;
    use crate::term::cell::Cell;

    /// Clearing a row that owns heap storage has to release it.
    ///
    /// The bulk clear overwrites cells without running their destructors, so
    /// taking it for a row holding zerowidth characters would leak every one.
    #[test]
    fn reset_releases_owned_storage() {
        let mut row = Row::<Cell>::new(8);
        row.mark_owns_storage();
        row[Column(3)].push_zerowidth('\u{0301}');

        let owned = Arc::clone(row[Column(3)].extra.as_ref().expect("zerowidth allocates extra"));
        assert_eq!(Arc::strong_count(&owned), 2);

        row.reset(&Cell::default());

        assert_eq!(Arc::strong_count(&owned), 1);
        assert!(row.is_clear());
        assert!(row[Column(3)].extra.is_none());
    }

    /// A row of plain text takes the bulk clear, which has to leave it in the
    /// same state the reset per cell did.
    #[test]
    fn bulk_reset_blanks_every_written_cell() {
        let mut row = Row::<Cell>::new(8);
        for column in 0..5 {
            row[Column(column)].c = 'x';
        }

        let mut template = Cell::default();
        template.bg = Color::Named(NamedColor::Red);
        row.reset(&template);

        assert_eq!(row.occ, 0);
        for column in 0..5 {
            assert_eq!(row[Column(column)].c, ' ');
            assert_eq!(row[Column(column)].bg, Color::Named(NamedColor::Red));
        }
    }
}
