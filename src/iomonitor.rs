// Copyright (c) 2022 Maël Kerbiriou <m431.kerbiriou@gmail.com>. All rights
// reserved. Use of this source is governed by MIT License that can be found in
// the LICENSE file.

use std::ffi::{OsStr, OsString};
use std::os::unix::prelude::OsStrExt;

use crate::errors::{Context, Result};

use crate::utils::{parse_integer, BulkReader};

const DISKSTATS_PATH: &str = "/proc/diskstats";

pub struct Device<T> {
    name: OsString,
    current_sectors: usize,
    pub data: T,
}

impl<T> Device<T> {
    pub fn name(&self) -> &OsStr {
        &self.name
    }
}

impl<'a, T> From<&'a mut Device<T>> for (&'a OsStr, usize, &'a mut T) {
    fn from(device: &'a mut Device<T>) -> Self {
        (&device.name, device.current_sectors, &mut device.data)
    }
}

/// Ressources for polling the # of touched sectors from /proc/diskstats
///
/// `data : T ` is attached user data.
pub struct IOMonitor<T> {
    file: BulkReader,
    state: Vec<Device<T>>,
}

/// Linear search by device name
fn get_entry_idx<T>(slice: &[Device<T>], name: &OsStr, hint: usize) -> Option<usize> {
    let pred = |&i: &usize| slice[i].name == name;
    (hint..slice.len())
        .find(pred)
        .or_else(|| (0..hint).find(pred))
}

impl<T> IOMonitor<T> {
    pub fn new() -> Result<Self> {
        Ok(Self {
            file: BulkReader::open(DISKSTATS_PATH)?,
            state: Vec::with_capacity(16),
        })
    }

    pub fn push(&mut self, name: OsString, data: T) -> &mut Device<T> {
        let idx = get_entry_idx(&self.state, &name, 0);
        if let Some(idx) = idx {
            let slot = &mut self.state[idx];
            slot.data = data;
            slot
        } else {
            self.state.push(Device {
                name,
                current_sectors: 0,
                data,
            });
            self.state.last_mut().unwrap()
        }
    }

    pub fn check_activity<'s, U, D>(&'s mut self, mut update_cb: U, create: D) -> Result<()>
    where
        U: FnMut(&mut Device<T>),
        D: Fn(&'s OsStr) -> T,
    {
        for device in &mut self.state {
            device.current_sectors = 0;
        }

        let mut entry_idx = 0;

        for line in self.file.read_lines()? {
            if let Some((name, sectors)) = parse_line(line)
                .with_context(|| format!("Parsing line '{}'", String::from_utf8_lossy(line)))?
            {
                if let Some(new_entry_idx) = get_entry_idx(&self.state, name, entry_idx) {
                    entry_idx = new_entry_idx;
                    let entry_sectors = &mut self.state[entry_idx].current_sectors;
                    *entry_sectors = entry_sectors.wrapping_add(sectors);
                } else {
                    entry_idx = self.state.len().min(entry_idx + 1);
                    let data = create(name);
                    let device = Device {
                        name: name.into(),
                        current_sectors: sectors,
                        data,
                    };
                    self.state.insert(entry_idx, device);
                }
            }
        }

        for device in &mut self.state {
            update_cb(device);
        }

        Ok(())
    }
}

fn parse_line(line: &[u8]) -> Result<Option<(&OsStr, usize)>> {
    let mut it = line.split(|c| *c == b' ').filter(|s| !s.is_empty());
    let mut next_tok = move || it.next().ok_or("Expected token");

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
