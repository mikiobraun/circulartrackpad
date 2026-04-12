# circulartrackpad

A small userspace daemon that makes the **circular ring gesture** on the
Panasonic Let's Note trackpad work as a scroll wheel under Wayland.

Panasonic Let's Note laptops ship with a distinctive round trackpad whose
outer ring is meant to be used as a rotary scroll control. Under X11 this
was handled by the old `xf86-input-synaptics` driver (`CircularScrolling`
option). `libinput` — which every Wayland compositor uses — dropped
circular scroll support, so on Wayland the ring zone just moves the
pointer like any other spot on the pad.

`circulartrackpad` restores the feature in userspace:

1. Grabs the real trackpad's `evdev` node exclusively.
2. Creates a virtual pointer device via `uinput`.
3. Classifies each touch: the **inner disc** becomes pointer motion, the
   **outer ring** becomes `REL_WHEEL` ticks based on angular movement.

The compositor sees a plain mouse-like device, so it works in GNOME,
KDE, Sway, or anything else — no extensions or plugins needed.

## Hardware

Developed and tested on a Panasonic Let's Note with a
**Synaptics TM3562-003** touchpad (HID `06cb:cdea`). Other Let's Note
models with a different touchpad controller will need the vendor/product
IDs in `install.sh` and the coordinate range in `src/main.rs` adjusted —
see *Porting* below.

## Install

Requires Rust (stable) and a Linux system with `udev` and `uinput`.

```bash
git clone https://github.com/mikiobraun/circulartrackpad
cd circulartrackpad
./install.sh
```

The installer:

- builds the release binary
- copies it to `/usr/local/bin/circulartrackpad`
- writes a udev rule to `/etc/udev/rules.d/70-circulartrackpad.rules`
  that grants the active local-seat user access to the trackpad and
  `/dev/uinput` via POSIX ACLs (systemd `uaccess`)
- reloads udev

Log out and back in once after install so the ACLs apply, then run:

```bash
circulartrackpad --help
```

No `sudo`, no `input` group membership needed — only the user currently
logged in at the physical seat can access the devices, and access is
revoked on logout.

## Usage

```
circulartrackpad [OPTIONS]

  -d, --device <DEVICE>    Input device path [default: /dev/input/event3]
  -p, --pointer <POINTER>  Pointer sensitivity multiplier [default: 1.5]
  -s, --scroll <SCROLL>    Scroll ticks per radian of ring rotation [default: 5]
  -r, --ring <RING>        Ring threshold as fraction of max radius [default: 0.65]
  -i, --invert-scroll      Invert scroll direction
```

Touch the inner area to move the pointer; slide your finger around the
outer ring to scroll. Lower `--ring` values make the ring zone wider
(easier to stay in while scrolling); higher values make it thinner.

### Running at login

Once you're happy with the options, run:

```bash
./enable-autostart.sh -p 1.5 -s 5 -r 0.65
```

Any arguments you pass to the script are baked into the systemd user
unit's `ExecStart`. It writes
`~/.config/systemd/user/circulartrackpad.service`, reloads the user
daemon, and enables + starts the service. To change the options later
either re-run the script or edit the unit file directly and
`systemctl --user daemon-reload && systemctl --user restart circulartrackpad`.

## Porting to other hardware

If your trackpad reports different HID IDs or a different coordinate
range, two spots need updating:

1. **`install.sh`** — change `VENDOR_ID` / `PRODUCT_ID` to match your
   device (find them with `cat /proc/bus/input/devices`).
2. **`src/main.rs`** — the `PAD_MAX` constant assumes `ABS_X`/`ABS_Y` go
   from 0 to 528. Check your device with `sudo evtest` and adjust if
   different. The classifier assumes a square, circular pad centered on
   the origin of the coordinate space.

## How it works

The daemon reads multitouch events (`ABS_MT_SLOT`, `ABS_MT_POSITION_X/Y`,
`ABS_MT_TRACKING_ID`) and on each `SYN_REPORT` inspects slot 0 (the
primary finger):

- If the finger is within `ring` of the max radius from center, it's in
  the **ring zone**: compute the polar angle, take the delta from the
  previous frame (with wraparound handling), accumulate fractional
  scroll ticks, and emit `REL_WHEEL`.
- Otherwise it's in the **inner zone**: emit `REL_X` / `REL_Y` deltas.

Button events (`BTN_LEFT`, `BTN_RIGHT`) are forwarded unchanged.

## License

MIT — see [LICENSE](LICENSE).
