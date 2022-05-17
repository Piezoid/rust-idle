// Copyright (c) 2022 Maël Kerbiriou <m431.kerbiriou@gmail.com>. All rights
// reserved. Use of this source is governed by MIT License that can be found in
// the LICENSE file.

use std::ffi::OsStr;
use std::os::unix::prelude::OsStrExt;

use crate::errors::{Context, Result};
use crate::sys;
use crate::utils::BulkReader;

const MOUNTS_PATH: &str = "/proc/self/mounts";

pub struct Mounts(BulkReader);

impl Mounts {
    pub fn new() -> Result<Self> {
        Ok(Mounts(BulkReader::open(MOUNTS_PATH)?))
    }

    pub fn update(&mut self) {
        self.0.clear();
    }

    pub fn for_dev<F>(&mut self, dev_name: &OsStr, mut f: F) -> Result<()>
    where
        F: FnMut(&sys::CStr) -> Result<()>,
    {
        if self.0.empty() {
            self.0.read()?;
        }
        for line in self.0.parse_lines_mut() {
            if let Some(mount_point) = parse_line(line, dev_name)? {
                f(&mount_point)?;
            }
        }
        Ok(())
    }
}

fn parse_line<'a>(line: &'a mut [u8], dev_name: &OsStr) -> Result<Option<&'a sys::CStr>> {
    let mut it = line.split_inclusive_mut(|c| *c == b' ' || *c == b'\0');
    let mut next_tok = move || it.next().ok_or_else(|| "Expected token".into());

    let source = next_tok().context("Parsing mount source")?;
    if !source.starts_with(b"/dev/") {
        return Ok(None); // not a block device
    }
    if !source[5..].starts_with(dev_name.as_bytes()) {
        return Ok(None); // not the device we're looking for
    }

    next_tok()
        .and_then(sys::make_inplace_cstr)
        .map(Some)
        .context("Parsing mount point")
}
