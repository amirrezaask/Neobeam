//! Ring buffer for scrollback line storage.

use std::ops::{Bound, Index, IndexMut, Range, RangeBounds};

pub struct RingBuffer<T> {
    elements: Vec<T>,
    current_index: isize,
}

pub struct RingBufferIter<'a, T> {
    ring_buffer: &'a RingBuffer<T>,
    range: Range<isize>,
}

pub struct RingBufferIterMut<'a, T> {
    ring_buffer: &'a mut RingBuffer<T>,
    range: Range<isize>,
}

impl<T: Clone> RingBuffer<T> {
    pub fn new(size: usize, default_value: T) -> Self {
        let mut elements = Vec::new();
        elements.resize(size, default_value);
        Self { current_index: 0, elements }
    }

    pub fn clone_from_iter<'a, I>(&'a mut self, iter: I)
    where
        I: IntoIterator<Item = &'a T>,
    {
        self.iter_mut().zip(iter).for_each(|(a, b)| *a = b.clone());
    }

    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    pub fn iter(&self) -> RingBufferIter<'_, T> {
        self.iter_range(..)
    }

    pub fn iter_mut(&mut self) -> RingBufferIterMut<'_, T> {
        self.iter_range_mut(..)
    }

    pub fn iter_range<R: RangeBounds<isize>>(&self, range: R) -> RingBufferIter<'_, T> {
        let range = self.get_bounds(range);
        RingBufferIter { ring_buffer: self, range }
    }

    pub fn iter_range_mut<R: RangeBounds<isize>>(&mut self, range: R) -> RingBufferIterMut<'_, T> {
        let range = self.get_bounds(range);
        RingBufferIterMut { ring_buffer: self, range }
    }

    pub fn len(&self) -> usize {
        self.elements.len()
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        let array_index = self.checked_array_index(index)?;
        Some(&self.elements[array_index])
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        let array_index = self.checked_array_index(index)?;
        Some(&mut self.elements[array_index])
    }

    pub fn resize(&mut self, new_size: usize, default_value: T) {
        if new_size > 0 && !self.elements.is_empty() {
            let index = self.get_array_index(0);
            self.elements.rotate_left(index);
        }
        self.elements.resize(new_size, default_value);
        self.current_index = 0;
    }

    pub fn rotate(&mut self, num: isize) {
        self.current_index += num;
    }

    fn get_array_index(&self, index: isize) -> usize {
        let num = self.elements.len() as isize;
        (self.current_index + index).rem_euclid(num) as usize
    }

    fn checked_array_index(&self, index: usize) -> Option<usize> {
        (index < self.len()).then(|| self.get_array_index(index as isize))
    }

    fn get_bounds<R: RangeBounds<isize>>(&self, range: R) -> Range<isize> {
        let start = match range.start_bound() {
            Bound::Included(start) => *start,
            Bound::Excluded(start) => *start + 1,
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(end) => *end + 1,
            Bound::Excluded(end) => *end,
            Bound::Unbounded => self.len() as isize,
        };
        start..end
    }
}

impl<T: Clone> Index<isize> for RingBuffer<T> {
    type Output = T;

    fn index(&self, index: isize) -> &Self::Output {
        let array_index = self.get_array_index(index);
        &self.elements[array_index]
    }
}

impl<T: Clone> IndexMut<isize> for RingBuffer<T> {
    fn index_mut(&mut self, index: isize) -> &mut Self::Output {
        let array_index = self.get_array_index(index);
        &mut self.elements[array_index]
    }
}

impl<'a, T: Clone> Iterator for RingBufferIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.range.is_empty() {
            return None;
        }
        let ret = &self.ring_buffer[self.range.start];
        self.range.start += 1;
        Some(ret)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.range.size_hint()
    }
}

impl<'a, T: Clone> Iterator for RingBufferIterMut<'a, T> {
    type Item = &'a mut T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.range.is_empty() {
            return None;
        }
        let elements = self.ring_buffer.elements.as_mut_ptr();
        let array_index = self.ring_buffer.get_array_index(self.range.start);
        let ret = unsafe { &mut *elements.add(array_index) };
        self.range.start += 1;
        Some(ret)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.range.size_hint()
    }
}
