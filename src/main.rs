use clap::Parser;
use evdev::uinput::VirtualDeviceBuilder;
use evdev::{AttributeSet, Device, EventType, InputEvent, Key, RelativeAxisType};
use std::f64::consts::PI;
use std::time::{Duration, Instant};

// -- Trackpad geometry (from evtest: ABS_X/ABS_Y range 0..528) --
const PAD_MAX: f64 = 528.0;
const CENTER_X: f64 = PAD_MAX / 2.0;
const CENTER_Y: f64 = PAD_MAX / 2.0;
const MAX_RADIUS: f64 = PAD_MAX / 2.0;

#[derive(Parser, Debug)]
#[command(about = "Userspace daemon for the Panasonic Let's Note circular trackpad")]
struct Args {
    /// Input device path
    #[arg(short, long, default_value = "/dev/input/event3")]
    device: String,

    /// Pointer sensitivity (multiplier on raw ABS deltas)
    #[arg(short, long, default_value_t = 1.5)]
    pointer: f64,

    /// Scroll sensitivity (REL_WHEEL ticks per radian of ring rotation)
    #[arg(short, long, default_value_t = 5.0)]
    scroll: f64,

    /// Ring threshold as fraction of max radius (0.0-1.0). Lower = wider ring.
    #[arg(short, long, default_value_t = 0.65)]
    ring: f64,

    /// Invert scroll direction
    #[arg(short, long, default_value_t = false)]
    invert_scroll: bool,

    /// Disable tap-to-click
    #[arg(long, default_value_t = false)]
    no_tap: bool,

    /// Tap timeout in milliseconds
    #[arg(long, default_value_t = 180)]
    tap_timeout: u64,

    /// Tap movement threshold in raw coordinate units
    #[arg(long, default_value_t = 20)]
    tap_move_threshold: i32,
}

// ABS event codes (not all are in evdev's typed enums)
const ABS_MT_SLOT: u16 = 0x2f;
const ABS_MT_TRACKING_ID: u16 = 0x39;
const ABS_MT_POSITION_X: u16 = 0x35;
const ABS_MT_POSITION_Y: u16 = 0x36;

#[derive(Clone, Copy)]
struct SlotState {
    tracking_id: i32, // -1 means no finger
    x: i32,
    y: i32,
}

impl Default for SlotState {
    fn default() -> Self {
        Self {
            tracking_id: -1,
            x: 0,
            y: 0,
        }
    }
}

#[derive(Default, Clone)]
struct TapState {
    start_time: Option<Instant>,
    start_pos: Option<(i32, i32)>,
    has_moved: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Zone {
    Inner,
    Ring,
}

fn classify(x: f64, y: f64, ring_threshold: f64) -> (Zone, f64, f64) {
    let dx = x - CENTER_X;
    let dy = y - CENTER_Y;
    let r = (dx * dx + dy * dy).sqrt();
    let angle = dy.atan2(dx);
    if r > ring_threshold {
        (Zone::Ring, r, angle)
    } else {
        (Zone::Inner, r, angle)
    }
}

fn angle_delta(prev: f64, curr: f64) -> f64 {
    let mut d = curr - prev;
    if d > PI {
        d -= 2.0 * PI;
    } else if d < -PI {
        d += 2.0 * PI;
    }
    d
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let ring_threshold = MAX_RADIUS * args.ring;
    let scroll_sign = if args.invert_scroll { 1.0 } else { -1.0 };

    println!("circulartrackpad: opening {}", args.device);
    let mut dev = Device::open(&args.device)?;
    println!(
        "circulartrackpad: grabbed '{}' (pointer={}, scroll={}, ring={})",
        dev.name().unwrap_or("unknown"),
        args.pointer,
        args.scroll,
        args.ring
    );
    dev.grab()?;

    // Build virtual device
    let mut keys = AttributeSet::<Key>::new();
    keys.insert(Key::BTN_LEFT);
    keys.insert(Key::BTN_RIGHT);
    keys.insert(Key::BTN_MIDDLE);

    let mut rel_axes = AttributeSet::<RelativeAxisType>::new();
    rel_axes.insert(RelativeAxisType::REL_X);
    rel_axes.insert(RelativeAxisType::REL_Y);
    rel_axes.insert(RelativeAxisType::REL_WHEEL);
    rel_axes.insert(RelativeAxisType::REL_HWHEEL);
    rel_axes.insert(RelativeAxisType::REL_WHEEL_HI_RES);
    rel_axes.insert(RelativeAxisType::REL_HWHEEL_HI_RES);

    let mut vdev = VirtualDeviceBuilder::new()?
        .name("circulartrackpad")
        .with_keys(&keys)?
        .with_relative_axes(&rel_axes)?
        .build()?;
    println!("circulartrackpad: virtual device created");

    // -- State --
    let mut slots = [SlotState::default(); 5];
    let mut current_slot: usize = 0;

    // For the primary finger (slot 0): track previous position and zone.
    // `locked_zone` is set the first time we see a new touch and stays
    // fixed for the lifetime of that touch — whichever zone the finger
    // started in is where it stays, so a gesture won't accidentally flip
    // modes if the finger drifts across the threshold.
    let mut locked_zone: Option<Zone> = None;
    let mut prev_angle: Option<f64> = None;
    let mut prev_x: Option<i32> = None;
    let mut prev_y: Option<i32> = None;
    let mut scroll_accumulator: f64 = 0.0; // fractional hi-res units (1/120 detent)
    let mut detent_carry: i32 = 0; // integer hi-res units pending a detent

    let tap_enabled = !args.no_tap;
    let tap_timeout = Duration::from_millis(args.tap_timeout);
    let tap_move_threshold = args.tap_move_threshold;
    let mut tap_states: [TapState; 5] = std::array::from_fn(|_| TapState::default());

    loop {
        for event in dev.fetch_events()? {
            let etype = event.event_type();
            let code = event.code();
            let value = event.value();

            match etype {
                EventType::ABSOLUTE => match code {
                    ABS_MT_SLOT => {
                        current_slot = value as usize;
                    }
                    ABS_MT_TRACKING_ID => {
                        if let Some(slot) = slots.get_mut(current_slot) {
                            slot.tracking_id = value;
                            if value == -1 {
                                // Finger lifted
                                if current_slot == 0 {
                                    if tap_enabled {
                                        let s0_valid = tap_states[0]
                                            .start_time
                                            .map(|t| t.elapsed() <= tap_timeout)
                                            .unwrap_or(false)
                                            && !tap_states[0].has_moved
                                            && locked_zone == Some(Zone::Inner);
                                        let s1_valid = tap_states[1]
                                            .start_time
                                            .map(|t| t.elapsed() <= tap_timeout)
                                            .unwrap_or(false)
                                            && !tap_states[1].has_moved;
                                        let s1_down = slots[1].tracking_id != -1;

                                        if s1_down && s0_valid && s1_valid {
                                            // Two-finger tap -> right click
                                            vdev.emit(&[InputEvent::new(
                                                EventType::KEY,
                                                Key::BTN_RIGHT.0,
                                                1,
                                            )])?;
                                            vdev.emit(&[InputEvent::new(
                                                EventType::KEY,
                                                Key::BTN_RIGHT.0,
                                                0,
                                            )])?;
                                        } else if s0_valid && !s1_down {
                                            // Single-finger tap -> left click
                                            vdev.emit(&[InputEvent::new(
                                                EventType::KEY,
                                                Key::BTN_LEFT.0,
                                                1,
                                            )])?;
                                            vdev.emit(&[InputEvent::new(
                                                EventType::KEY,
                                                Key::BTN_LEFT.0,
                                                0,
                                            )])?;
                                        }
                                    }

                                    locked_zone = None;
                                    prev_angle = None;
                                    prev_x = None;
                                    prev_y = None;
                                    scroll_accumulator = 0.0;
                                    detent_carry = 0;
                                    tap_states[0] = TapState::default();
                                    tap_states[1] = TapState::default();
                                } else if current_slot == 1 {
                                    if tap_enabled {
                                        let s0_down = slots[0].tracking_id != -1;
                                        let s0_valid = tap_states[0]
                                            .start_time
                                            .map(|t| t.elapsed() <= tap_timeout)
                                            .unwrap_or(false)
                                            && !tap_states[0].has_moved
                                            && locked_zone == Some(Zone::Inner);
                                        let s1_valid = tap_states[1]
                                            .start_time
                                            .map(|t| t.elapsed() <= tap_timeout)
                                            .unwrap_or(false)
                                            && !tap_states[1].has_moved;

                                        if s0_down && s0_valid && s1_valid {
                                            vdev.emit(&[InputEvent::new(
                                                EventType::KEY,
                                                Key::BTN_RIGHT.0,
                                                1,
                                            )])?;
                                            vdev.emit(&[InputEvent::new(
                                                EventType::KEY,
                                                Key::BTN_RIGHT.0,
                                                0,
                                            )])?;
                                            tap_states[0] = TapState::default();
                                        }
                                    }
                                    tap_states[1] = TapState::default();
                                }
                            } else if current_slot < tap_states.len() {
                                // Finger down
                                tap_states[current_slot] = TapState {
                                    start_time: Some(Instant::now()),
                                    start_pos: None,
                                    has_moved: false,
                                };
                            }
                        }
                    }
                    ABS_MT_POSITION_X => {
                        if let Some(slot) = slots.get_mut(current_slot) {
                            slot.x = value;
                        }
                    }
                    ABS_MT_POSITION_Y => {
                        if let Some(slot) = slots.get_mut(current_slot) {
                            slot.y = value;
                        }
                    }
                    _ => {}
                },

                EventType::KEY => {
                    // Pass through button events
                    match code {
                        c if c == Key::BTN_LEFT.code()
                            || c == Key::BTN_RIGHT.code()
                            || c == Key::BTN_MIDDLE.code() =>
                        {
                            vdev.emit(&[event])?;
                        }
                        _ => {}
                    }
                }

                EventType::SYNCHRONIZATION => {
                    // Update tap-detection state for active slots.
                    for slot_idx in 0..2 {
                        let slot = &slots[slot_idx];
                        if slot.tracking_id == -1 {
                            continue;
                        }
                        if tap_states[slot_idx].start_time.is_some()
                            && tap_states[slot_idx].start_pos.is_none()
                        {
                            tap_states[slot_idx].start_pos = Some((slot.x, slot.y));
                        }
                        if let Some((sx, sy)) = tap_states[slot_idx].start_pos {
                            let dx = slot.x - sx;
                            let dy = slot.y - sy;
                            if dx.abs() > tap_move_threshold || dy.abs() > tap_move_threshold {
                                tap_states[slot_idx].has_moved = true;
                            }
                        }
                    }

                    // On SYN_REPORT, process the primary finger (slot 0)
                    let slot = &slots[0];
                    if slot.tracking_id == -1 {
                        continue;
                    }

                    let x = slot.x as f64;
                    let y = slot.y as f64;
                    let (current_zone, _, angle) = classify(x, y, ring_threshold);

                    // Lock the zone on the first frame of a new touch.
                    let zone = *locked_zone.get_or_insert(current_zone);

                    let mut events_out: Vec<InputEvent> = Vec::new();

                    match zone {
                        Zone::Ring => {
                            if let Some(pa) = prev_angle {
                                // High-resolution scroll: 120 units = 1 detent.
                                // scroll_accumulator holds fractional hi-res
                                // units; we emit the integer portion each
                                // frame as REL_WHEEL_HI_RES and emit integer
                                // REL_WHEEL only when crossing a full detent
                                // (for apps that don't consume HI_RES).
                                let delta = angle_delta(pa, angle);
                                scroll_accumulator +=
                                    delta * args.scroll * 120.0 * scroll_sign;

                                let hires = scroll_accumulator.trunc() as i32;
                                if hires != 0 {
                                    scroll_accumulator -= hires as f64;
                                    events_out.push(InputEvent::new(
                                        EventType::RELATIVE,
                                        RelativeAxisType::REL_WHEEL_HI_RES.0,
                                        hires,
                                    ));

                                    detent_carry += hires;
                                    let detents = detent_carry / 120;
                                    if detents != 0 {
                                        detent_carry -= detents * 120;
                                        events_out.push(InputEvent::new(
                                            EventType::RELATIVE,
                                            RelativeAxisType::REL_WHEEL.0,
                                            detents,
                                        ));
                                    }
                                }
                            }
                            prev_angle = Some(angle);
                        }
                        Zone::Inner => {
                            if let (Some(px), Some(py)) = (prev_x, prev_y) {
                                let dx = ((slot.x - px) as f64 * args.pointer) as i32;
                                let dy = ((slot.y - py) as f64 * args.pointer) as i32;
                                if dx != 0 {
                                    events_out.push(InputEvent::new(
                                        EventType::RELATIVE,
                                        RelativeAxisType::REL_X.0,
                                        dx,
                                    ));
                                }
                                if dy != 0 {
                                    events_out.push(InputEvent::new(
                                        EventType::RELATIVE,
                                        RelativeAxisType::REL_Y.0,
                                        dy,
                                    ));
                                }
                            }
                            prev_x = Some(slot.x);
                            prev_y = Some(slot.y);
                        }
                    }

                    if !events_out.is_empty() {
                        vdev.emit(&events_out)?;
                    }
                }

                _ => {}
            }
        }
    }
}
