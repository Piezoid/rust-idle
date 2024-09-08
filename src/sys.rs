// Copyright (c) 2022 Maël Kerbiriou <m431.kerbiriou@gmail.com>. All rights
// reserved. Use of this source is governed by MIT License that can be found in
// the LICENSE file.

use std::ffi::{c_void, OsStr, OsString};
use std::os::unix::ffi::OsStrExt;

pub use nc::c_str::CStr;

use crate::errors::{Context, Result};

/// Create a `CStr` by writing a '\0' in place at the end of a mutable byte slice.
///
/// The last byte must be a whitespace character (' ', '\t', or '\0').
pub fn make_inplace_cstr(str: &mut [u8]) -> Result<&CStr> {
    let last = str.last_mut().ok_or("Empty string")?;
    match last {
        b' ' | b'\t' | b'\0' => {
            *last = b'\0';
            // Borrow-wise, it should be as safe as `return Ok(str)`:
            // No further mutation is possible while the returned &CStr is held.
            Ok(unsafe { &*(str as *const [u8] as *const CStr) })
        }
        _ => {
            Err(format!(
                "Expected null or whitespace at the end of '{}'",
                OsStr::from_bytes(str).to_string_lossy()
            )
            .into())
        }
    }
}

pub const fn is_scsi(major: usize) -> bool {
    matches!(major, 8 | 65..=71)
}

/// Returns the device name (as found under `/dev/`) from a symlink, while
/// ensuring that the device is indeed a SCSI device.
pub fn link_to_scsi_name(path: &OsStr) -> Result<OsString> {
    let mut stat_buf = nc::stat_t::default();
    unsafe { nc::stat(path, &mut stat_buf) }
        .with_context(|| format!("stat {}", path.to_string_lossy()))?;
    if stat_buf.st_mode & nc::S_IFMT != nc::S_IFBLK {
        return Err(format!("Not a block device: '{}'", path.to_string_lossy()).into());
    }
    let major = stat_buf.st_rdev >> 8;
    let minor = stat_buf.st_rdev & 0xff;
    if !is_scsi(major) {
        return Err(format!("Not a SCSI device: '{}'", path.to_string_lossy()).into());
    }
    if minor % 16 != 0 {
        return Err(format!(
            "'{}' is a partition, not a root device",
            path.to_string_lossy()
        )
        .into());
    }
    let dev_path = std::fs::canonicalize(path)
        .with_context(|| format!("getting cannonical path to '{}'", path.to_string_lossy()))?;
    dev_path
        .strip_prefix("/dev/")
        .map_err(|_| {
            format!(
                "path '{}' doesn't resolves to device under '/dev/' ('{}')",
                path.to_string_lossy(),
                dev_path.to_string_lossy()
            )
            .into()
        })
        .map(|name| name.to_owned().into())
}

/// Bracket style wrapper to safely open a device as a raw fd.
fn with_dev_fd<F, R>(dev_name: &OsStr, f: F) -> Result<R>
where
    F: Fn(i32) -> Result<R>,
{
    const MAX_PATH_LEN: usize = 16;
    const PATH_PREFIX: &[u8] = b"/dev/";
    let path_len = PATH_PREFIX.len() + dev_name.len();
    // == => no room for '\0'
    if path_len >= MAX_PATH_LEN {
        return Err(format!("Device name too long: '{}'", dev_name.to_string_lossy()).into());
    }
    let mut path = [0u8; MAX_PATH_LEN];
    path[..PATH_PREFIX.len()].copy_from_slice(PATH_PREFIX);
    path[PATH_PREFIX.len()..path_len].copy_from_slice(dev_name.as_bytes());
    path[path_len] = b'\0';

    let filename_ptr = path.as_ptr() as usize;
    let flags = nc::O_RDONLY as usize;
    let fd = unsafe { nc::syscalls::syscall3(nc::SYS_OPEN, filename_ptr, flags, 0) }
        .map(|ret| ret as i32)
        .with_context(|| {
            format!(
                "Could not open device '{}'",
                String::from_utf8_lossy(&path[..path_len])
            )
        })?;

    let res = f(fd);

    unsafe { nc::close(fd) }.with_context(|| {
        format!(
            "Failed to close '{}'",
            String::from_utf8_lossy(&path[..path_len]),
        )
    })?;
    res
}

pub fn syncfs(path: &CStr) -> Result<()> {
    let path_ptr = path.as_ptr() as usize;
    let flags = nc::O_RDONLY as usize;
    unsafe {
        let fd = nc::syscalls::syscall3(nc::SYS_OPEN, path_ptr, flags, 0)
            .map(|ret| ret as i32)
            .with_context(|| {
                format!(
                    "Could not open mount point '{}'",
                    String::from_utf8_lossy(path.to_bytes())
                )
            })?;
        let res = nc::syncfs(fd).with_context(|| {
            format!(
                "Could not sync mount point '{}'",
                String::from_utf8_lossy(path.to_bytes())
            )
        });
        nc::close(fd).with_context(|| {
            format!(
                "Could not close mount point '{}'",
                String::from_utf8_lossy(path.to_bytes())
            )
        })?;
        res
    }
}

const BLKFLSBUF: u32 = nc::IO(0x12, 97);

pub fn sync_blockdev(dev: &OsStr) -> Result<i32> {
    with_dev_fd(dev, |fd| {
        unsafe { nc::ioctl(fd, BLKFLSBUF, std::ptr::null()) }
            .with_context(|| format!("Could not sync block device '{}'", dev.to_string_lossy()))
    })
}

/// Issue SCSI command to spin down a disk.
//TODO: implement for ATA/USB devices.
pub fn spindown_disk(dev: &OsStr) -> Result<()> {
    /// Pulled from `/usr/include/scsi/sg.h`, comments are GNU 2.1 licensed,
    /// Copyright (C) 1997-2022 Free Software Foundation, Inc.
    #[repr(C)]
    struct sg_io_hdr {
        i32erface_id: i32,      /* [i] 'S' for SCSI generic (required) */
        dxfer_direction: i32,   /* [i] data transfer direction  */
        cmd_len: u8,            /* [i] SCSI command length ( <= 16 bytes) */
        mx_sb_len: u8,          /* [i] max length to write to sbp */
        iovec_count: u16,       /* [i] 0 implies no scatter gather */
        dxfer_len: u32,         /* [i] byte count of data transfer */
        dxferp: *mut c_void, /* [i], [*io] points to data transfer memory or scatter gather list */
        cmdp: *const u8,     /* [i], [*i] points to command to perform */
        sbp: *mut u8,        /* [i], [*o] ponts to sense_buffer memory */
        timeout: u32,        /* [i] MAX_UINT->no timeout (unit: millisec) */
        flags: u32,          /* [i] 0 -> default, see SG_FLAG... */
        pack_id: i32,        /* [i->o] unused internally (normally) */
        usr_ptr: *const c_void, /* [i->o] unused internally */
        status: u8,          /* [o] scsi status */
        masked_status: u8,   /* [o] shifted, masked scsi status */
        msg_status: u8,      /* [o] messaging level data (optional) */
        sb_len_wr: u8,       /* [o] byte count actually written to sbp */
        host_status: u16,    /* [o] errors from host adapter */
        driver_status: u16,  /* [o] errors from software driver */
        resid: i32,          /* [o] dxfer_len - actual_transferred */
        duration: u32,       /* [o] time taken by cmd (unit: millisec) */
        info: u32,           /* [o] auxiliary information */
    }

    const SCSI_STOP_CMD: &[u8] = b"\x1b\x00\x00\x00\x00\x00";
    const SG_DXFER_NONE: i32 = -1;
    const SG_IO: u32 = 0x2285;
    const CHECK_CONDITION: u8 = 0x01;

    with_dev_fd(dev, |fd| {
        let mut sens_buf = [0u8; 255];
        let mut hdr = sg_io_hdr {
            i32erface_id: 'S' as i32,
            dxfer_direction: SG_DXFER_NONE,
            cmd_len: SCSI_STOP_CMD.len() as u8,
            mx_sb_len: sens_buf.len() as u8,
            iovec_count: 0,
            dxfer_len: 0,
            dxferp: std::ptr::null_mut(),
            cmdp: SCSI_STOP_CMD.as_ptr(),
            sbp: sens_buf.as_mut_ptr(),
            timeout: 0,
            flags: 0,
            pack_id: 0,
            usr_ptr: std::ptr::null(),
            status: 0,
            masked_status: 0,
            msg_status: 0,
            sb_len_wr: 0,
            host_status: 0,
            driver_status: 0,
            resid: 0,
            duration: 0,
            info: 0,
        };
        unsafe { nc::ioctl(fd, SG_IO, std::ptr::addr_of_mut!(hdr) as *const c_void) }
            .context("Could not send SCSI command")?;
        if hdr.masked_status == 0 {
            Ok(())
        } else {
            Err(if hdr.masked_status == CHECK_CONDITION {
                format!(
                    "SCSI command failed with CHECK_CONDITION, sense_buf: {:?}",
                    &sens_buf[..hdr.sb_len_wr as usize]
                )
                .into()
            } else {
                format!("SCSI command failed with status {:#04x}", hdr.masked_status).into()
            })
        }
    })
}
