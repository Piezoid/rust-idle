// Copyright (c) 2022 Maël Kerbiriou <m431.kerbiriou@gmail.com>. All rights
// reserved. Use of this source is governed by MIT License that can be found in
// the LICENSE file.

mod errors;
mod iomonitor;
mod mounts;
mod sys;
mod utils;

use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{stderr, Write};
use std::process::exit;
use std::time::{Duration, SystemTime};

use os_str_bytes::{RawOsStr, RawOsString};

use errors::{Context, Result};
use iomonitor::IOMonitor;
use mounts::Mounts;

enum DeviceState {
    Busy(),
    Synced(),
    Idle(),
}

#[derive(Clone, Default)]
struct DeviceConfig {
    idle_time: Duration,
    sync_flags: u8,
    verbosity: u8,
}

const SYNC_SPIN_DOWN: u8 = 1;
const SYNC_SPIN_UP: u8 = 2;

impl fmt::Display for DeviceConfig {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        const SYNC_BOTH: u8 = SYNC_SPIN_DOWN | SYNC_SPIN_UP;
        let sync_flags = match self.sync_flags {
            0 => "NONE",
            SYNC_SPIN_DOWN => "SPIN_DOWN",
            SYNC_SPIN_UP => "SPIN_UP",
            SYNC_BOTH => "SPIN_DOWN | SPIN_UP",
            _ => "UNKNOWN",
        };
        write!(
            f,
            "{{ idle_time: {}s, sync_flags: {}, verbosity: {} }}",
            self.idle_time.as_secs(),
            sync_flags,
            self.verbosity
        )
    }
}

struct Device {
    sectors: usize,
    state: DeviceState,
    last_io: SystemTime,
    config: DeviceConfig,
}

impl Device {
    const fn new(config: DeviceConfig) -> Self {
        Self {
            config,
            state: DeviceState::Busy(),
            sectors: 0,
            last_io: SystemTime::UNIX_EPOCH,
        }
    }
}

struct App {
    devices_monitor: IOMonitor<Device>,
    mounts: Mounts,
    default_config: DeviceConfig,
    interval: Duration,
}

impl App {
    fn new(
        default_config: DeviceConfig,
        mut devices_config: Vec<(OsString, DeviceConfig)>,
    ) -> Result<Option<Self>> {
        devices_config.sort_by(|(a, _), (b, _)| a.cmp(b));
        let mut devices_monitor = IOMonitor::new()?;
        let mut min_idle_time = if default_config.idle_time > Duration::ZERO {
            default_config.idle_time
        } else {
            Duration::MAX
        };

        // Inserts configured devices in the IOMonitor while checking for duplicates
        let mut prev_name = OsStr::new("");
        for (dev, config) in devices_config {
            if prev_name == dev {
                return Err(format!("Duplicated device: {}", dev.to_string_lossy()).into());
            }
            if config.verbosity >= 2 {
                println!(
                    "<6>Device {} configured as {}",
                    dev.to_string_lossy(),
                    config
                );
            }
            if config.idle_time > Duration::ZERO {
                min_idle_time = min_idle_time.min(config.idle_time);
            }
            prev_name = devices_monitor.push(dev, Device::new(config)).0;
        }

        let interval = (min_idle_time / 10).max(Duration::from_secs(1));
        if default_config.verbosity >= 2 {
            println!(
                "<6>Default device configuration: {}. Refresh period: {}s",
                default_config,
                interval.as_secs()
            );
        }

        Ok(if min_idle_time == Duration::MAX {
            None // No device with an idle_time > 0
        } else {
            Some(Self {
                devices_monitor,
                mounts: Mounts::new()?,
                default_config,
                interval,
            })
        })
    }

    fn tick(&mut self) -> Result<bool> {
        self.mounts.update(); // Clear the mount table, will lazy load when needed.

        let now = SystemTime::now();
        let mut will_sleep = true;

        self.devices_monitor.check_activity(
            |dev_name: &OsStr, new_sectors: usize, record: &mut Device| {
                let config = &record.config;
                let idle_time = now
                    .duration_since(record.last_io)
                    .expect("non monotonic time");
                let sectors_inc = new_sectors.wrapping_sub(record.sectors);
                let busy = sectors_inc != 0;
                if busy {
                    if config.verbosity >= 3 && record.sectors != 0 {
                        println!(
                            "<7>Activity detected on {}, sectors: {} => {} (+{}), idle time: {}s",
                            dev_name.to_string_lossy(),
                            record.sectors,
                            new_sectors,
                            sectors_inc,
                            idle_time.as_secs()
                        );
                    }
                    record.sectors = new_sectors;
                    record.last_io = now;
                }

                if record.config.idle_time == Duration::ZERO {
                    return;
                }

                record.state = match record.state {
                    DeviceState::Busy() => {
                        if !busy && idle_time >= config.idle_time {
                            if config.verbosity >= 1 {
                                println!(
                                    "<5>{} has gone idle. (idle_time: {}s >= {}s)",
                                    dev_name.to_string_lossy(),
                                    idle_time.as_secs(),
                                    record.config.idle_time.as_secs()
                                );
                            }
                            let next_state = if config.sync_flags & SYNC_SPIN_DOWN == 0 {
                                DeviceState::Idle()
                            } else {
                                sync_block_device(&mut self.mounts, dev_name, config.verbosity);
                                // Immediately refresh the statistics while ignoring activity on this
                                // device from the sync.
                                will_sleep = false;
                                DeviceState::Synced()
                            };
                            if config.verbosity >= 2 {
                                println!("<6>Spinning down {}", dev_name.to_string_lossy());
                            }
                            if let Err(e) = sys::spindown_disk(dev_name) {
                                eprintln!(
                                    "<4>Failed to spin down {}: {}",
                                    dev_name.to_string_lossy(),
                                    e
                                );
                            }
                            next_state
                        } else {
                            DeviceState::Busy()
                        }
                    }
                    DeviceState::Synced() => DeviceState::Idle(),
                    DeviceState::Idle() => {
                        if busy {
                            record.state = DeviceState::Busy();
                            if config.verbosity >= 1 {
                                println!(
                                    "<5>{} has spun up. (idle_time: {}s)",
                                    dev_name.to_string_lossy(),
                                    idle_time.as_secs()
                                );
                            }
                            if config.sync_flags & SYNC_SPIN_UP != 0 {
                                sync_block_device(&mut self.mounts, dev_name, config.verbosity);
                            }
                            DeviceState::Busy()
                        } else {
                            DeviceState::Idle()
                        }
                    }
                };
            },
            |name| {
                if self.default_config.verbosity >= 1 {
                    println!("<5>New device detected: {}", name.to_string_lossy());
                }
                Device::new(self.default_config.clone())
            },
        )?;

        Ok(will_sleep)
    }

    fn run(&mut self) -> Result<()> {
        loop {
            if self.tick()? {
                std::thread::sleep(self.interval);
            }
        }
    }
}

/// Syncs all filesystems associated with the given device, then sync the device buffers.
///
/// mounts: utility object to read and cache the mount points.
fn sync_block_device(mounts: &mut Mounts, dev: &OsStr, verbosity: u8) {
    if verbosity >= 2 {
        println!("<6>Syncing {}", dev.to_string_lossy());
    }

    if let Err(e) = mounts
        .for_dev(dev, |mount_point| {
            if verbosity >= 3 {
                println!(
                    "<7>syncfs({})",
                    String::from_utf8_lossy(mount_point.to_bytes())
                );
            }
            sys::syncfs(mount_point)
        })
        //FIXME: is this redundant?
        .and_then(|_| sys::sync_blockdev(dev))
    {
        eprintln!("<4>Failed to sync {}: {}\n", dev.to_string_lossy(), e);
    }
}

fn parse_flags(flags: &RawOsStr, default: &DeviceConfig) -> Result<DeviceConfig> {
    let mut config = default.clone();
    let mut idle_time = 0;
    let mut idle_time_sealed = false;
    let mut prefix = b'+';
    let mut prev_flag = b' ';
    for &c in flags.as_raw_bytes() {
        if prev_flag != b'-' && c != prev_flag {
            prefix = b'+'; // Reset modifier to the default (+), but not for '-vv' (equivalent to '-v-v')
        }
        let digit = u64::from(c.wrapping_sub(b'0'));
        if digit < 10 {
            if idle_time_sealed {
                return Err("idle time already set".into());
            }
            idle_time = idle_time * 10 + digit;
        } else {
            idle_time_sealed = idle_time > 0;
            match c {
                b's' => {
                    if prefix == b'+' {
                        config.sync_flags |= SYNC_SPIN_DOWN;
                    } else {
                        config.sync_flags &= !SYNC_SPIN_DOWN;
                    }
                }
                b'S' => {
                    if prefix == b'+' {
                        config.sync_flags |= SYNC_SPIN_UP;
                    } else {
                        config.sync_flags &= !SYNC_SPIN_UP;
                    }
                }
                b'v' => {
                    if prefix == b'+' {
                        config.verbosity = config.verbosity.saturating_add(1);
                    } else {
                        config.verbosity = config.verbosity.saturating_sub(1);
                    }
                }
                b'+' | b'-' => {
                    prefix = c;
                }
                _ => {
                    return Err(format!("invalid flag '{}'", c as char).into());
                }
            }
        }
        prev_flag = c;
    }
    if idle_time > 0 || idle_time_sealed {
        config.idle_time = Duration::from_secs(idle_time);
    }
    Ok(config)
}

#[inline(never)]
fn parse_args() -> Result<App> {
    let mut args = env::args_os();
    let mut default_config = DeviceConfig::default();
    let mut devices_config = Vec::with_capacity(args.len() - 1);

    let bin_name = args.next();
    for arg in args.map(RawOsString::new) {
        if let Some((disk, flags)) = arg.split_once(':') {
            let config = parse_flags(flags, &default_config)
                .with_context(|| format!("parsing flags for '{}'", arg.to_str_lossy()))?;
            if disk.is_empty() {
                default_config = config;
            } else {
                let dev = sys::link_to_scsi_name(disk.to_os_str().as_ref())
                    .with_context(|| format!("getting device for {}", disk.to_str_lossy()))?;
                devices_config.push((dev, config));
            }
        } else {
            let dev = sys::link_to_scsi_name(arg.to_os_str().as_ref())
                .with_context(|| format!("getting device for {}", arg.to_str_lossy()))?;
            devices_config.push((dev, default_config.clone()));
        }
    }
    let maybe_app = App::new(default_config, devices_config)?;
    if let Some(app) = maybe_app {
        Ok(app)
    } else {
        write!(
            stderr(),
            r#"No disk configured with an idle time > 0, will do nothing.

Usage: {} :<default flags> <device path or symlink>[:<flags>]

flags:
    <number>: idle time in seconds before spinning down a drive, if equal to zero,
              no spinning down is performed. Can only be specified once per
              flag set.
    s:        sync the disk before spinning down
   -s:        don't sync the disk before spinning down
    S:        sync the disk when spinning up is detected
   -S:        don't sync the disk when spinning up is detected
    v:        increases verbosity (can be repeated up to 3 times)
   -v:        decreases verbosity

The default flags are inherited by the following drives arguments. The final
default flag set is applied to remaining drives discovered at runtime.
Flags prefixed with '-' are subtracted/removed from the set inherited from
the default flags. <idle time> always overrides the default idle time.
A contrived example:
    {0} :svv300 /dev/sda /dev/sdb:6000-sS-vv :-v600
is equivalent to:
    {0} /dev/sda:300svv /dev/sdb:6000S :600s
In this sample, the final default flags are '600s'='svv-vv600': drives not
listed here (eg. /dev/sdc) will be spun down after 10min idle time, with
verbosity=0 and sync on spin-up events.
"#,
            bin_name
                .and_then(|bn| bn.into_string().ok())
                .expect("invalid binary name")
        )?;
        exit(0)
    }
}

fn main() {
    exit(
        match parse_args().and_then(|mut app| app.run().context("main loop")) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("<3>error: {}\n", e);
                1
            }
        },
    )
}
