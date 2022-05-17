// Copyright (c) 2022 Maël Kerbiriou <m431.kerbiriou@gmail.com>. All rights
// reserved. Use of this source is governed by MIT License that can be found in
// the LICENSE file.

use std::ffi::{OsStr, OsString};
use std::ops::{Index, IndexMut};
use std::os::unix::prelude::OsStrExt;

use anyhow::{Context, Error, Result};

use crate::utils::{parse_integer, BulkReader};

const DISKSTATS_PATH: &str = "/proc/diskstats";

pub struct IOMonitor<T> {
    file: BulkReader,
    state: Vec<(OsString, usize, T)>,
}

impl<T> Index<usize> for IOMonitor<T> {
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        &self.state[index].2
    }
}

impl<T> IndexMut<usize> for IOMonitor<T> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.state[index].2
    }
}

fn get_entry_idx<T>(vec: &Vec<(OsString, usize, T)>, name: &OsStr, hint: usize) -> Option<usize> {
    for i in hint..vec.len() {
        if vec[i].0 == name {
            return Some(i);
        }
    }
    for i in 0..hint {
        if vec[i].0 == name {
            return Some(i);
        }
    }
    None
}

impl<T> IOMonitor<T> {
    pub fn new() -> Result<Self> {
        Ok(IOMonitor {
            file: BulkReader::open(DISKSTATS_PATH)?,
            state: Vec::with_capacity(16),
        })
    }

    pub fn get_entry_idx(&self, dev: &OsStr, hint: usize) -> Option<usize> {
        for i in hint..self.state.len() {
            if self.state[i].0 == dev {
                return Some(i);
            }
        }
        for i in 0..hint {
            if self.state[i].0 == dev {
                return Some(i);
            }
        }
        None
    }

    pub fn push(&mut self, dev: OsString, val: T) -> (&OsString, &mut T) {
        let idx = self.get_entry_idx(&dev, 0);
        let slot = if let Some(idx) = idx {
            let slot = &mut self.state[idx];
            slot.2 = val;
            slot
        } else {
            self.state.push((dev, 0, val));
            self.state.last_mut().unwrap()
        };
        (&slot.0, &mut slot.2)
    }

    pub fn check_activity<'s, U, D>(&'s mut self, mut update_cb: U, create: D) -> Result<()>
    where
        U: FnMut(&'s OsStr, usize, &'s mut T) -> (),
        D: Fn(&'s OsStr) -> T,
    {
        for (_, value, _) in self.state.iter_mut() {
            *value = 0;
        }

        let mut entry_idx = 0;

        for line in self.file.read_lines()? {
            if let Some((name, sectors)) = parse_line(line)
                .with_context(|| format!("Parsing line '{}'", String::from_utf8_lossy(line)))?
            {
                if let Some(new_entry_idx) = get_entry_idx(&self.state, &name, entry_idx) {
                    entry_idx = new_entry_idx;
                    let entry_sectors = &mut self.state[entry_idx].1;
                    *entry_sectors = entry_sectors.wrapping_add(sectors);
                } else {
                    entry_idx = self.state.len().min(entry_idx + 1);
                    self.state
                        .insert(entry_idx, (name.into(), sectors, create(name)));
                }
            }
        }

        for (name, sectors, value) in self.state.iter_mut() {
            update_cb(name, *sectors, value);
        }

        Ok(())
    }
}

fn parse_line(line: &[u8]) -> Result<Option<(&OsStr, usize)>> {
    let mut it = line.split(|c| *c == b' ').filter(|s| s.len() > 0);
    let mut next_tok = move || it.next().ok_or_else(|| Error::msg("Expected token"));

    // major
    if !crate::sys::is_scsi(parse_integer(next_tok()?)?) {
        return Ok(None);
    }
    next_tok()?; // minor

    let name = next_tok()?; // block identifier
    let name_digits = name
        .iter()
        .rev()
        .take_while(|c| c.wrapping_sub(b'0') <= 9)
        .count();
    if name_digits == 0 {
        return Ok(None); // not a partition
    }

    next_tok()?; // of reads completed (unsigned long)
    next_tok()?; // of reads merged, field 6 – # of writes merged (unsigned long)

    // of sectors read (unsigned long)
    let mut sectors = parse_integer(next_tok()?)?;

    next_tok()?; // of milliseconds spent reading (unsigned int)
    next_tok()?; // of writes completed (unsigned long)
    next_tok()?; // of writes merged (unsigned long)

    // of sectors written (unsigned long)
    sectors = sectors.wrapping_add(parse_integer(next_tok()?)?);

    next_tok()?; // of milliseconds spent writing (unsigned int)
    next_tok()?; // of I/Os currently in progress (unsigned int)
    next_tok()?; // of milliseconds spent doing I/Os (unsigned int)
    next_tok()?; // weighted # of milliseconds spent doing I/Os (unsigned int)

    next_tok()?; // of discards completed (unsigned long)
    next_tok()?; // of discards merged (unsigned long)

    // of sectors discarded (unsigned long)
    sectors = sectors.wrapping_add(parse_integer(next_tok()?)?);

    Ok(Some((
        OsStr::from_bytes(&name[..name.len() - name_digits]),
        sectors,
    )))
}
