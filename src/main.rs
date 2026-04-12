use clap::Parser;
use evdev::uinput::VirtualDeviceBuilder;
use evdev::{AttributeSet, Device, EventType, InputEvent, Key, RelativeAxisType};
use std::f64::consts::PI;

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

    let mut vdev = VirtualDeviceBuilder::new()?
        .name("circulartrackpad")
        .with_keys(&keys)?
        .with_relative_axes(&rel_axes)?
        .build()?;
    println!("circulartrackpad: virtual device created");

    // -- State --
    let mut slots = [SlotState::default(); 5];
    let mut current_slot: usize = 0;

    // For the primary finger (slot 0): track previous position and zone
    let mut prev_angle: Option<f64> = None;
    let mut prev_x: Option<i32> = None;
    let mut prev_y: Option<i32> = None;
    let mut scroll_accumulator: f64 = 0.0;

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
                                    prev_angle = None;
                                    prev_x = None;
                                    prev_y = None;
                                    scroll_accumulator = 0.0;
                                }
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
                    // On SYN_REPORT, process the primary finger (slot 0)
                    let slot = &slots[0];
                    if slot.tracking_id == -1 {
                        continue;
                    }

                    let x = slot.x as f64;
                    let y = slot.y as f64;
                    let mut events_out: Vec<InputEvent> = Vec::new();

                    match classify(x, y, ring_threshold) {
                        (Zone::Ring, _, angle) => {
                            if let Some(pa) = prev_angle {
                                let delta = angle_delta(pa, angle);
                                scroll_accumulator += delta * args.scroll;

                                // Emit integer ticks, keep fractional remainder
                                let ticks = scroll_accumulator as i32;
                                if ticks != 0 {
                                    scroll_accumulator -= ticks as f64;
                                    events_out.push(InputEvent::new(
                                        EventType::RELATIVE,
                                        RelativeAxisType::REL_WHEEL.0,
                                        (scroll_sign * ticks as f64) as i32,
                                    ));
                                }
                            }
                            prev_angle = Some(angle);
                            // Reset pointer tracking when in ring
                            prev_x = None;
                            prev_y = None;
                        }
                        (Zone::Inner, _, _) => {
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
                            // Reset ring tracking when in inner zone
                            prev_angle = None;
                            scroll_accumulator = 0.0;
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
