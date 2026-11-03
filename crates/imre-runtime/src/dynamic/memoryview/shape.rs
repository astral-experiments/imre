//! Geometry of views cast from a C-contiguous buffer. Slicing changes only the first axis.
use super::{Error, Result, slice};
use std::rc::Rc;

#[derive(Clone)]
pub(crate) struct ViewLayout {
    start: usize,
    byte_offset: usize,
    pub(crate) len: usize,
    stride: i64,
    pub(super) byte_format: Option<&'static str>,
    // The common 1-D case uses len directly. A scalar stores an empty shape.
    shape: Option<Rc<[usize]>>,
}

impl ViewLayout {
    pub(crate) fn full(len: usize) -> Self {
        Self {
            start: 0,
            byte_offset: 0,
            len,
            stride: 1,
            byte_format: None,
            shape: None,
        }
    }

    pub(super) fn cast(
        offset: usize,
        width: usize,
        len: usize,
        format: &'static str,
        shape: Option<Vec<usize>>,
    ) -> Self {
        let mut view = Self {
            start: offset / width,
            byte_offset: offset % width,
            byte_format: Some(format),
            shape: shape.filter(|dims| dims.len() != 1).map(Rc::from),
            ..Self::full(len)
        };
        view.stride = view.row_len() as i64;
        view
    }

    pub(crate) fn shape(&self) -> &[usize] {
        self.shape
            .as_deref()
            .unwrap_or_else(|| std::slice::from_ref(&self.len))
    }

    // Only the first dimension of a cast view can become empty, through slicing.
    pub(super) fn same_shape(&self, other: &Self) -> bool {
        if self.len == 0 && other.len == 0 {
            self.shape().len() == other.shape().len()
        } else {
            self.shape() == other.shape()
        }
    }

    pub(crate) fn first_len(&self) -> Result<usize> {
        self.shape()
            .first()
            .copied()
            .ok_or_else(|| Error::new("TypeError", "0-dim memory has no length"))
    }

    // Every axis after the first still has its original compact C layout.
    fn row_len(&self) -> usize {
        match self.shape() {
            [] | [_] => 1,
            [0, tail @ ..] => tail.iter().product(),
            [rows, ..] => self.len / rows,
        }
    }

    pub(crate) fn strides(&self, width: usize) -> Vec<i64> {
        let dims = self.shape();
        let mut strides = vec![0; dims.len()];
        let mut stride = width as i64;
        for axis in (1..dims.len()).rev() {
            strides[axis] = stride;
            stride *= dims[axis] as i64;
        }
        if let Some(first) = strides.first_mut() {
            *first = self.stride.wrapping_mul(width as i64);
        }
        strides
    }

    pub(crate) fn is_contiguous(&self) -> bool {
        match self.shape() {
            [] => true,
            [n] => *n == 1 || self.stride == 1,
            [rows, ..] => *rows <= 1 || self.stride == self.row_len() as i64,
        }
    }

    pub(crate) fn is_f_contiguous(&self) -> bool {
        self.is_contiguous()
            && (self.len == 0 || self.shape().iter().filter(|&&n| n > 1).count() <= 1)
    }

    fn position(&self, index: usize) -> i128 {
        let row = self.row_len();
        self.start as i128 + (index / row) as i128 * i128::from(self.stride) + (index % row) as i128
    }

    pub(super) fn checked(self, bytes: usize, width: usize) -> Result<Self> {
        let elements = bytes.saturating_sub(self.byte_offset) / width;
        let fits = if self.len == 0 {
            self.start <= elements
        } else {
            let row = self.row_len();
            let first = self.start as i128;
            let last = self.position(self.len - row);
            first.min(last) >= 0 && first.max(last) + row as i128 <= elements as i128
        };
        if self.byte_offset > bytes || self.len > elements || !fits {
            return Err(Error::new(
                "BufferError",
                "underlying buffer has changed size",
            ));
        }
        Ok(self)
    }

    pub(super) fn element(&self, index: usize) -> usize {
        self.position(index) as usize
    }

    pub(crate) fn byte_start(&self, width: usize) -> usize {
        self.byte_offset + self.start * width
    }

    pub(super) fn byte_at(&self, index: usize, width: usize) -> usize {
        self.byte_offset + self.element(index) * width
    }

    pub(super) fn scalar_at(&self, index: usize) -> Self {
        Self {
            start: self.element(index),
            byte_offset: self.byte_offset,
            ..Self::full(1)
        }
    }

    pub(super) fn sliced(mut self, bounds: [Option<i64>; 3]) -> Result<Self> {
        let [start, stop, step] = slice::normalize(bounds, self.first_len()? as i64);
        // PySlice_Unpack limits the absolute step to PY_SSIZE_T_MAX.
        let step = step.max(-i64::MAX);
        let distance = if step > 0 { stop - start } else { start - stop };
        let rows = (distance.max(0) as u64).div_ceil(step.unsigned_abs()) as usize;
        let row = self.row_len();
        self.start = if rows == 0 {
            0
        } else {
            self.element(start as usize * row)
        };
        self.len = rows * row;
        if let Some(shape) = &mut self.shape {
            Rc::make_mut(shape)[0] = rows;
        }
        // CPython uses signed Py_ssize_t arithmetic for empty and singleton strides.
        self.stride = self.stride.wrapping_mul(step);
        Ok(self)
    }

    // Convert a flat Fortran-order index into the C-order index used by element().
    pub(super) fn fortran_index(&self, mut index: usize) -> usize {
        let mut c_index = 0;
        for &size in self.shape() {
            c_index = c_index * size + index % size;
            index /= size;
        }
        c_index
    }
}
