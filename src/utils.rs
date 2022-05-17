// Copyright (c) 2022 Maël Kerbiriou <m431.kerbiriou@gmail.com>. All rights
// reserved. Use of this source is governed by MIT License that can be found in
// the LICENSE file.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::errors::{Context, Result};

/// An utility to repeatedly read a file into a buffer, minimizing allocations.
pub struct BulkReader {
    file: File,
    buf: Vec<u8>,
}

impl BulkReader {
    pub fn open_with_capacity<P: AsRef<Path>>(path: P, capacity: usize) -> Result<Self> {
        Ok(BulkReader {
            file: File::open(path.as_ref())
                .with_context(|| format!("Openning '{}' for reading", path.as_ref().display()))?,
            buf: Vec::with_capacity(capacity),
        })
    }

    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::open_with_capacity(path, 4096)
    }

    pub fn empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn get(&self) -> &[u8] {
        &self.buf
    }

    pub fn clear(&mut self) {
        self.buf.clear();
    }

    pub fn read(&mut self) -> Result<&mut [u8]> {
        self.file.seek(SeekFrom::Start(0))?;
        self.clear();
        self.file.read_to_end(&mut self.buf)?;
        Ok(&mut self.buf)
    }

    pub fn read_lines(&mut self) -> Result<impl Iterator<Item = &mut [u8]>> {
        Ok(self
            .read()?
            .split_mut(|c| *c == b'\n')
            .filter(|l| l.len() > 0))
    }

    #[allow(unused)]
    pub fn parse_lines(&self) -> impl Iterator<Item = &[u8]> {
        self.get().split(|c| *c == b'\n').filter(|l| l.len() > 0)
    }

    pub fn parse_lines_mut(&mut self) -> impl Iterator<Item = &mut [u8]> {
        self.buf.split_mut(|c| *c == b'\n').filter(|l| l.len() > 0)
    }
}

pub fn parse_integer(txt: &[u8]) -> Result<usize> {
    let mut res: usize = 0;
    for &c in txt {
        let v = c.wrapping_sub(b'0');
        if v <= 9 {
            res = res.wrapping_mul(10).wrapping_add(v as usize);
        } else {
            return Err(format!("Invalid integer: '{}'", String::from_utf8_lossy(txt)).into());
        }
    }
    Ok(res)
}
