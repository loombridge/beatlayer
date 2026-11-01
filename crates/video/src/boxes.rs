//! Reading the boxes (atoms) of a QuickTime or ISO base media file from
//! memory. Every read is bounds-checked: a damaged file gives an error.

use crate::error::VideoError;

/// The largest `moov` box read into memory: far above any real movie's, whose
/// sample tables run to a few megabytes an hour.
pub(crate) const MOOV_LIMIT: u64 = 256 << 20;

pub(crate) fn malformed(what: &str) -> VideoError {
    VideoError::Malformed(what.into())
}

/// The boxes in a body: `(kind, body)`. Stops at the first box that doesn't
/// fit.
pub(crate) fn boxes(mut data: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    std::iter::from_fn(move || {
        let (kind, body, rest) = split_box(data)?;
        data = rest;
        Some((kind, body))
    })
}

/// One box off the front of `data`: its kind, its body, and what follows.
pub(crate) fn split_box(data: &[u8]) -> Option<([u8; 4], &[u8], &[u8])> {
    if data.len() < 8 {
        return None;
    }
    let size32 = u32::from_be_bytes(data[0..4].try_into().ok()?);
    let kind: [u8; 4] = data[4..8].try_into().ok()?;
    let (size, header) = match size32 {
        0 => (data.len(), 8),
        1 if data.len() >= 16 => (
            usize::try_from(u64::from_be_bytes(data[8..16].try_into().ok()?)).ok()?,
            16,
        ),
        1 => return None,
        n => (n as usize, 8),
    };
    if size < header || size > data.len() {
        return None;
    }
    Some((kind, &data[header..size], &data[size..]))
}

pub(crate) fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(data).find(|(k, _)| k == kind).map(|(_, body)| body)
}

/// Big-endian reads from a body, each failing on a truncated box.
pub(crate) struct Be<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Be<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    pub fn skip(&mut self, n: usize) -> Result<(), VideoError> {
        self.take(n).map(|_| ())
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8], VideoError> {
        let end = self
            .at
            .checked_add(n)
            .filter(|&end| end <= self.data.len())
            .ok_or_else(|| malformed("a truncated box"))?;
        let bytes = &self.data[self.at..end];
        self.at = end;
        Ok(bytes)
    }

    pub fn u8(&mut self) -> Result<u8, VideoError> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, VideoError> {
        Ok(u16::from_be_bytes(
            self.take(2)?.try_into().unwrap_or_default(),
        ))
    }

    pub fn u32(&mut self) -> Result<u32, VideoError> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().unwrap_or_default(),
        ))
    }

    pub fn u64(&mut self) -> Result<u64, VideoError> {
        Ok(u64::from_be_bytes(
            self.take(8)?.try_into().unwrap_or_default(),
        ))
    }

    /// Bytes left.
    pub fn remaining(&self) -> usize {
        self.data.len() - self.at
    }

    /// The rest of the body.
    pub fn rest(&self) -> &'a [u8] {
        &self.data[self.at..]
    }
}
