# `rust-idle`

*Give a break to your spinning rust.*

`rust-idle` is a small linux daemon that spin down hard drives after a period of
idle time.

`rust-idle` is a reimplementation of *Christian Mueller*'s
[`hd-idle`](http://hd-idle.sourceforge.net/) inspired by [another
reimplementation in go](https://github.com/adelolmo/hd-idle).

### Why?

*Yet another implementation?*

Why not? Rust is well suited for this task and I couldn't let go have it all. 😏

The real reason is that this is meant as an exercise to see how well rust fares
for writing simple and small system daemons that are as lean as possible. A
domain where C is king.

## **Warning**

Frequently spinning down hard drives can quickly wear down the head parking
mechanism. This can potentially offset any cost saving on energy or spindle
wear.

This software is provided *as-is*, without warranty of any kind. The authors
decline all responsibility for any damage resulting from its use.

## Features

* Configurable idle time period and verbosity per hard drive, default settings
  for all other drives,
* Possibility to sync the filesystems on the hard-drive before spinning it down
  and/or after it has waked up. This prevent spurious flushing of dirty pages
  and enables swifter idling,
* Systemd unit file included, log-level formatting for journald,
* Tiny runtime footprint: no allocations during normal operation, unless logging
  is enabled or new drives are hot-plugged; Small binary when built with
  `build-std` (80kB on `x86_64`),
* Like [the go implementation](https://github.com/adelolmo/hd-idle), `rust-idle`
  doesn't monitors activity on root device but instead monitors partitions. This
  prevents false-positives when monitoring daemons like `smartd` or `udiskd`
  query the drive's status registers,
* ❌ Monitor and spin down disks connected over USB (planned, not yet
  implemented),
* ❌ Has only been tested on `x86_64` linux,
* ❓ Any suggestion? Please open an [issue].

## Installation

**dependencies**: `cargo` and `rustc`. For a smaller binary size, `rustup` with
a nightly toolchain is recommended.

On Arch Linux, the `rust-idler-git` package can be built from the AUR, or with
`arch/PKGBUILD` found in this repos.

Cargo can build this project the usual way (`cargo build --release`), however
`build.sh` will try to use the `build-std` feature, if available.

`install.sh` installs the binary, service file, and documentation under
`PREFIX=/usr/local/` by default. To test its behavior, it is possible to run it
as a normal user, with the right environment. For example:

```bash
export DESTDIR="$XDG_RUNTIME_DIR/testroot" PREFIX=/usr
./install.sh
find "$DESTDIR" -type f
```
```
/run/user/1000/testroot/etc/conf.d/rust-idle
/run/user/1000/testroot/usr/lib/systemd/system/rust-idle.service
/run/user/1000/testroot/usr/share/licenses/rust-idle/LICENSE
/run/user/1000/testroot/usr/share/doc/rust-idle/README.md
/run/user/1000/testroot/usr/bin/rust-idle
```


## Usage

Once installed, the daemon must be enabled and started with:
```
# systemctl enable --now rust-idle
```

With the default configuration under [`/etc/conf.d/rust-idle`](rust-idle.conf),
`rust-idle` waits for 10min of idle time before syncing and spinning down
any SATA drive found on the system.

Behavior can be specialized per device. Setting an idle time of 0 disable any
action. For example:
```
RUST_IDLE_OPTS= :0 /dev/sda:600 /dev/sdb:1200
```
will spin down the drive `sda` after 10min of idle time, `sdb` after 20 min, and
ignore any other drive present in the system.
