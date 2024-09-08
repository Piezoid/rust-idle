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
use mounts::Mounts;

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeviceState {
    Spinning(),
    /// The disk was synced: next update will ignore activity and transition to `Idle`.
    Synced(),
    Idle(),
}

/// Stores disk config and retained statistics. `IOMonitor` wraps instances
/// inside `Device<DeviceData>`s which adds statistics read from /dev/diskstats.
struct DeviceData {
    sectors: usize,
    state: DeviceState,
    last_io: SystemTime,
    config: DeviceConfig,
}

impl From<DeviceConfig> for DeviceData {
    fn from(config: DeviceConfig) -> Self {
        Self {
            config,
            state: DeviceState::Spinning(),
            sectors: 0,
            last_io: SystemTime::UNIX_EPOCH,
        }
    }
}

type IOMonitor = iomonitor::IOMonitor<DeviceData>;
type Device = iomonitor::Device<DeviceData>;

impl Device {
    /// Main state transition function.
    ///
    /// Runtime errors are handled here and recovered from after writing to
    /// stderr.
    fn tick(self: &mut Device, now: SystemTime, mounts: &mut Mounts) -> DeviceState {
        let (dev_name, new_sectors, device_data) = self.into();
        let config = &device_data.config;

        // Difference in read/write/discarded sectors tells us if the disk was
        // busy between two time steps.
        let sectors_inc = new_sectors.wrapping_sub(device_data.sectors);
        let busy = sectors_inc != 0;

        let idle_time = if busy {
            // Update retained statistics in DeviceData
            if config.verbosity >= 3 && device_data.sectors != 0 {
                println!(
                    "<7>Activity detected on {}, sectors: {} => {} (+{})",
                    dev_name.to_string_lossy(),
                    device_data.sectors,
                    new_sectors,
                    sectors_inc
                );
            }
            device_data.sectors = new_sectors;
            device_data.last_io = now;

            Duration::ZERO
        } else {
            now.duration_since(device_data.last_io)
                .expect("non monotonic time")
        };

        // Skip unconfigured disks
        if config.idle_time == Duration::ZERO {
            return DeviceState::Spinning();
        }

        // Compute and execute state transitions
        device_data.state = match device_data.state {
            DeviceState::Spinning() => {
                if idle_time >= config.idle_time {
                    if config.verbosity >= 1 {
                        println!(
                            "<5>{} has gone idle. (idle_time: {}s >= {}s)",
                            dev_name.to_string_lossy(),
                            idle_time.as_secs(),
                            config.idle_time.as_secs()
                        );
                    }
                    let next_state = if config.sync_flags & SYNC_SPIN_DOWN == 0 {
                        DeviceState::Idle()
                    } else {
                        sync_block_device(mounts, dev_name, config.verbosity);
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
                    DeviceState::Spinning()
                }
            }
            DeviceState::Synced() => DeviceState::Idle(),
            DeviceState::Idle() => {
                if busy {
                    if config.verbosity >= 1 {
                        println!(
                            "<5>{} has spun up. (idle_time: {}s)",
                            dev_name.to_string_lossy(),
                            idle_time.as_secs()
                        );
                    }
                    if config.sync_flags & SYNC_SPIN_UP != 0 {
                        sync_block_device(mounts, dev_name, config.verbosity);
                    }
                    DeviceState::Spinning()
                } else {
                    DeviceState::Idle()
                }
            }
        };
        device_data.state
    }
}

/// Syncfs all filesystems associated with the given device, then sync the
/// device buffers.
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

struct App {
    devices_monitor: IOMonitor,
    mounts: Mounts,
    default_config: DeviceConfig,
    interval: Duration,
}

impl App {
    fn new(
        default_config: DeviceConfig,
        mut device_configs: Vec<(OsString, DeviceConfig)>,
    ) -> Result<Option<Self>> {
        let mut devices_monitor = IOMonitor::new()?;
        let mut min_idle_time = if default_config.idle_time > Duration::ZERO {
            default_config.idle_time
        } else {
            Duration::MAX
        };

        // Insert configured devices in the IOMonitor while checking for duplicates
        device_configs.sort_by(|(a, _), (b, _)| a.cmp(b));
        let mut prev_name = OsStr::new("");
        for (dev, config) in device_configs {
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
            prev_name = devices_monitor.push(dev, config.into()).name();
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
            None // No device with an idle_time > 0, show usage and exit
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
            |device| {
                let new_state = device.tick(now, &mut self.mounts);
                // Immediately refresh the statistics while ignoring activity
                // from syncing this device.
                will_sleep &= new_state != DeviceState::Synced();
            },
            |name| {
                if self.default_config.verbosity >= 1 {
                    println!("<5>New device detected: {}", name.to_string_lossy());
                }
                self.default_config.clone().into()
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

fn parse_flags(flags: &RawOsStr, default: &DeviceConfig) -> Result<DeviceConfig> {
    let mut config = default.clone();
    let mut idle_time = 0;
    let mut idle_time_sealed = false;
    let mut prefix = b'+';
    let mut prev_flag = b' ';
    for &c in flags.as_encoded_bytes() {
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
                    config.verbosity = if prefix == b'+' {
                        config.verbosity.saturating_add(1)
                    } else {
                        config.verbosity.saturating_sub(1)
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

fn parse_args() -> Result<App> {
    let mut args = env::args_os().map(RawOsString::new);
    let mut default_config = DeviceConfig::default();
    let mut device_configs = Vec::with_capacity(args.len() - 1);

    let bin_name = args.next();
    for arg in args {
        let (disk, flags) = arg
            .split_once(':')
            .map_or((arg.as_ref(), None), |(disk, flags)| (disk, Some(flags)));

        let config = if let Some(flags) = flags {
            // "[disk]:flags" -> use the config made with flags on top of config
            parse_flags(flags, &default_config)?
        } else {
            // "disk" -> use the default config
            default_config.clone()
        };

        if disk.is_empty() {
            // ":flags" -> assign flags to the default config
            default_config = config;
        } else {
            // "disk:[flags]" -> set the config of the device
            let dev = sys::link_to_scsi_name(disk.as_os_str())
                .with_context(|| format!("getting device for {}", disk.to_str_lossy()))?;
            device_configs.push((dev, config));
        }
    }

    App::new(default_config, device_configs)?.map_or_else(
        || {
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
        },
        Ok,
    )
}

fn main() {
    exit(
        match parse_args().and_then(|mut app| app.run().context("main loop")) {
            Ok(_) => 0,
            Err(e) => {
                eprintln!("<3>error: {}\n", e);
                1
            }
        },
    )
}
