//! Wayland-safe Linux input injection through `/dev/uinput`.
//!
//! The kernel sees the generated keyboard and absolute pointer as ordinary
//! input devices, so injection works under Hyprland without compositor-specific
//! virtual-input protocols.
//!
//! ## Permissions (Arch/Omarchy)
//!
//! Create a dedicated `uinput` group and grant it the device with udev:
//!
//! ```text
//! # /etc/udev/rules.d/70-mahord-uinput.rules
//! KERNEL=="uinput", GROUP="uinput", MODE="0660", OPTIONS+="static_node=uinput"
//! ```
//!
//! Then run `sudo groupadd -f uinput`, add the host user with
//! `sudo usermod -aG uinput $USER`, load `uinput`, reload udev rules, and log
//! out/in. The process must never run setuid or as root merely for injection.

use std::{collections::HashSet, io};

use evdev::{
    uinput::VirtualDevice, AbsInfo, AbsoluteAxisCode, AbsoluteAxisEvent, AttributeSet, EventType,
    InputEvent, KeyCode, RelativeAxisCode, UinputAbsSetup,
};
use maho_proto::{InputEvent as WireInputEvent, InputEventType, Modifiers};

/// Derives evdev absolute axis max limits for uinput setup.
///
/// Linux evdev absolute axes are inclusive `[minimum, maximum]`. For a desktop
/// of width `W` (pixels `0..W-1`), the maximum coordinate index is `W - 1`. Setting
/// `maximum = W` creates `W + 1` discrete values, causing compositors/libinput to
/// scale incoming coordinates by `(W - 1) / W` and introducing a systematic -1 pixel boundary error.
pub fn evdev_abs_axis_max(dimension: u32) -> i32 {
    dimension.saturating_sub(1) as i32
}

/// Target output position and dimensions in compositor/global logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Signed origin of the logical desktop. Outputs left of or above the
    /// primary display make this negative.
    pub desktop_x: i32,
    pub desktop_y: i32,
    pub desktop_width: u32,
    pub desktop_height: u32,
}

impl OutputGeometry {
    pub fn single_output(width: u32, height: u32) -> Self {
        Self {
            x: 0,
            y: 0,
            width,
            height,
            desktop_x: 0,
            desktop_y: 0,
            desktop_width: width,
            desktop_height: height,
        }
    }
}

/// Pure coordinate mapping used by the uinput backend.
pub fn map_normalized_to_output(
    normalized_x: f32,
    normalized_y: f32,
    geometry: OutputGeometry,
) -> (u32, u32) {
    let x = normalized_x.clamp(0.0, 1.0) * geometry.width.saturating_sub(1) as f32;
    // Protocol convention inverts Y (1.0 - y) for legacy macOS compatibility.
    // Invert it back so (0,0) is top-left on Linux.
    let y = (1.0 - normalized_y).clamp(0.0, 1.0) * geometry.height.saturating_sub(1) as f32;
    // The uinput absolute axes are unsigned, so shift by the signed desktop
    // origin instead of clamping negative multi-monitor offsets to zero.
    (
        (i64::from(geometry.x) - i64::from(geometry.desktop_x) + x.round() as i64).max(0) as u32,
        (i64::from(geometry.y) - i64::from(geometry.desktop_y) + y.round() as i64).max(0) as u32,
    )
}

/// Maps the v3/macOS virtual-key convention to Linux evdev keys.
///
/// The wire key code intentionally remains the existing macOS key code so all
/// hosts interoperate with the current clients.
pub fn macos_keycode_to_evdev(key_code: u16) -> Option<KeyCode> {
    Some(match key_code {
        0 => KeyCode::KEY_A,
        1 => KeyCode::KEY_S,
        2 => KeyCode::KEY_D,
        3 => KeyCode::KEY_F,
        4 => KeyCode::KEY_H,
        5 => KeyCode::KEY_G,
        6 => KeyCode::KEY_Z,
        7 => KeyCode::KEY_X,
        8 => KeyCode::KEY_C,
        9 => KeyCode::KEY_V,
        11 => KeyCode::KEY_B,
        12 => KeyCode::KEY_Q,
        13 => KeyCode::KEY_W,
        14 => KeyCode::KEY_E,
        15 => KeyCode::KEY_R,
        16 => KeyCode::KEY_Y,
        17 => KeyCode::KEY_T,
        18 => KeyCode::KEY_1,
        19 => KeyCode::KEY_2,
        20 => KeyCode::KEY_3,
        21 => KeyCode::KEY_4,
        22 => KeyCode::KEY_6,
        23 => KeyCode::KEY_5,
        24 => KeyCode::KEY_EQUAL,
        25 => KeyCode::KEY_9,
        26 => KeyCode::KEY_7,
        27 => KeyCode::KEY_MINUS,
        28 => KeyCode::KEY_8,
        29 => KeyCode::KEY_0,
        30 => KeyCode::KEY_RIGHTBRACE,
        31 => KeyCode::KEY_O,
        32 => KeyCode::KEY_U,
        33 => KeyCode::KEY_LEFTBRACE,
        34 => KeyCode::KEY_I,
        35 => KeyCode::KEY_P,
        36 => KeyCode::KEY_ENTER,
        37 => KeyCode::KEY_L,
        38 => KeyCode::KEY_J,
        39 => KeyCode::KEY_APOSTROPHE,
        40 => KeyCode::KEY_K,
        41 => KeyCode::KEY_SEMICOLON,
        42 => KeyCode::KEY_BACKSLASH,
        43 => KeyCode::KEY_COMMA,
        44 => KeyCode::KEY_SLASH,
        45 => KeyCode::KEY_N,
        46 => KeyCode::KEY_M,
        47 => KeyCode::KEY_DOT,
        48 => KeyCode::KEY_TAB,
        49 => KeyCode::KEY_SPACE,
        50 => KeyCode::KEY_GRAVE,
        51 => KeyCode::KEY_BACKSPACE,
        53 => KeyCode::KEY_ESC,
        54 => KeyCode::KEY_RIGHTMETA,
        55 => KeyCode::KEY_LEFTMETA,
        56 => KeyCode::KEY_LEFTSHIFT,
        57 => KeyCode::KEY_CAPSLOCK,
        58 => KeyCode::KEY_LEFTALT,
        59 => KeyCode::KEY_LEFTCTRL,
        60 => KeyCode::KEY_RIGHTSHIFT,
        61 => KeyCode::KEY_RIGHTALT,
        62 => KeyCode::KEY_RIGHTCTRL,
        63 => KeyCode::KEY_FN,
        64 => KeyCode::KEY_F17,
        65 => KeyCode::KEY_KPDOT,
        67 => KeyCode::KEY_KPASTERISK,
        69 => KeyCode::KEY_KPPLUS,
        71 => KeyCode::KEY_NUMLOCK,
        75 => KeyCode::KEY_KPSLASH,
        76 => KeyCode::KEY_KPENTER,
        78 => KeyCode::KEY_KPMINUS,
        79 => KeyCode::KEY_F18,
        80 => KeyCode::KEY_F19,
        81 => KeyCode::KEY_KPEQUAL,
        82 => KeyCode::KEY_KP0,
        83 => KeyCode::KEY_KP1,
        84 => KeyCode::KEY_KP2,
        85 => KeyCode::KEY_KP3,
        86 => KeyCode::KEY_KP4,
        87 => KeyCode::KEY_KP5,
        88 => KeyCode::KEY_KP6,
        89 => KeyCode::KEY_KP7,
        91 => KeyCode::KEY_KP8,
        92 => KeyCode::KEY_KP9,
        96 => KeyCode::KEY_F5,
        97 => KeyCode::KEY_F6,
        98 => KeyCode::KEY_F7,
        99 => KeyCode::KEY_F3,
        100 => KeyCode::KEY_F8,
        101 => KeyCode::KEY_F9,
        103 => KeyCode::KEY_F11,
        105 => KeyCode::KEY_F13,
        106 => KeyCode::KEY_F16,
        107 => KeyCode::KEY_F14,
        109 => KeyCode::KEY_F10,
        111 => KeyCode::KEY_F12,
        113 => KeyCode::KEY_F15,
        114 => KeyCode::KEY_INSERT,
        115 => KeyCode::KEY_HOME,
        116 => KeyCode::KEY_PAGEUP,
        117 => KeyCode::KEY_DELETE,
        118 => KeyCode::KEY_F4,
        119 => KeyCode::KEY_END,
        120 => KeyCode::KEY_F2,
        121 => KeyCode::KEY_PAGEDOWN,
        122 => KeyCode::KEY_F1,
        123 => KeyCode::KEY_LEFT,
        124 => KeyCode::KEY_RIGHT,
        125 => KeyCode::KEY_DOWN,
        126 => KeyCode::KEY_UP,
        _ => return None,
    })
}

/// Maps an ASCII character to an evdev KeyCode and whether Shift is required.
pub fn ascii_to_evdev(ch: char) -> Option<(KeyCode, bool)> {
    match ch {
        'a'..='z' => {
            let key = match ch {
                'a' => KeyCode::KEY_A,
                'b' => KeyCode::KEY_B,
                'c' => KeyCode::KEY_C,
                'd' => KeyCode::KEY_D,
                'e' => KeyCode::KEY_E,
                'f' => KeyCode::KEY_F,
                'g' => KeyCode::KEY_G,
                'h' => KeyCode::KEY_H,
                'i' => KeyCode::KEY_I,
                'j' => KeyCode::KEY_J,
                'k' => KeyCode::KEY_K,
                'l' => KeyCode::KEY_L,
                'm' => KeyCode::KEY_M,
                'n' => KeyCode::KEY_N,
                'o' => KeyCode::KEY_O,
                'p' => KeyCode::KEY_P,
                'q' => KeyCode::KEY_Q,
                'r' => KeyCode::KEY_R,
                's' => KeyCode::KEY_S,
                't' => KeyCode::KEY_T,
                'u' => KeyCode::KEY_U,
                'v' => KeyCode::KEY_V,
                'w' => KeyCode::KEY_W,
                'x' => KeyCode::KEY_X,
                'y' => KeyCode::KEY_Y,
                'z' => KeyCode::KEY_Z,
                _ => unreachable!(),
            };
            Some((key, false))
        }
        'A'..='Z' => {
            let lower = ch.to_ascii_lowercase();
            ascii_to_evdev(lower).map(|(k, _)| (k, true))
        }
        '0'..='9' => {
            let key = match ch {
                '0' => KeyCode::KEY_0,
                '1' => KeyCode::KEY_1,
                '2' => KeyCode::KEY_2,
                '3' => KeyCode::KEY_3,
                '4' => KeyCode::KEY_4,
                '5' => KeyCode::KEY_5,
                '6' => KeyCode::KEY_6,
                '7' => KeyCode::KEY_7,
                '8' => KeyCode::KEY_8,
                '9' => KeyCode::KEY_9,
                _ => unreachable!(),
            };
            Some((key, false))
        }
        ' ' => Some((KeyCode::KEY_SPACE, false)),
        '\n' | '\r' => Some((KeyCode::KEY_ENTER, false)),
        '\t' => Some((KeyCode::KEY_TAB, false)),
        '-' => Some((KeyCode::KEY_MINUS, false)),
        '_' => Some((KeyCode::KEY_MINUS, true)),
        '=' => Some((KeyCode::KEY_EQUAL, false)),
        '+' => Some((KeyCode::KEY_EQUAL, true)),
        '[' => Some((KeyCode::KEY_LEFTBRACE, false)),
        '{' => Some((KeyCode::KEY_LEFTBRACE, true)),
        ']' => Some((KeyCode::KEY_RIGHTBRACE, false)),
        '}' => Some((KeyCode::KEY_RIGHTBRACE, true)),
        ';' => Some((KeyCode::KEY_SEMICOLON, false)),
        ':' => Some((KeyCode::KEY_SEMICOLON, true)),
        '\'' => Some((KeyCode::KEY_APOSTROPHE, false)),
        '"' => Some((KeyCode::KEY_APOSTROPHE, true)),
        '`' => Some((KeyCode::KEY_GRAVE, false)),
        '~' => Some((KeyCode::KEY_GRAVE, true)),
        '\\' => Some((KeyCode::KEY_BACKSLASH, false)),
        '|' => Some((KeyCode::KEY_BACKSLASH, true)),
        ',' => Some((KeyCode::KEY_COMMA, false)),
        '<' => Some((KeyCode::KEY_COMMA, true)),
        '.' => Some((KeyCode::KEY_DOT, false)),
        '>' => Some((KeyCode::KEY_DOT, true)),
        '/' => Some((KeyCode::KEY_SLASH, false)),
        '?' => Some((KeyCode::KEY_SLASH, true)),
        '!' => Some((KeyCode::KEY_1, true)),
        '@' => Some((KeyCode::KEY_2, true)),
        '#' => Some((KeyCode::KEY_3, true)),
        '$' => Some((KeyCode::KEY_4, true)),
        '%' => Some((KeyCode::KEY_5, true)),
        '^' => Some((KeyCode::KEY_6, true)),
        '&' => Some((KeyCode::KEY_7, true)),
        '*' => Some((KeyCode::KEY_8, true)),
        '(' => Some((KeyCode::KEY_9, true)),
        ')' => Some((KeyCode::KEY_0, true)),
        _ => None,
    }
}

fn modifier_key(modifier: Modifiers) -> Option<KeyCode> {
    if modifier == Modifiers::SHIFT {
        Some(KeyCode::KEY_LEFTSHIFT)
    } else if modifier == Modifiers::CONTROL {
        Some(KeyCode::KEY_LEFTCTRL)
    } else if modifier == Modifiers::OPTION {
        Some(KeyCode::KEY_LEFTALT)
    } else if modifier == Modifiers::COMMAND {
        Some(KeyCode::KEY_LEFTMETA)
    } else if modifier == Modifiers::CAPS_LOCK {
        Some(KeyCode::KEY_CAPSLOCK)
    } else {
        None
    }
}

fn modifier_events(previous: Modifiers, current: Modifiers) -> Vec<InputEvent> {
    [
        Modifiers::SHIFT,
        Modifiers::CONTROL,
        Modifiers::OPTION,
        Modifiers::COMMAND,
        Modifiers::CAPS_LOCK,
    ]
    .into_iter()
    .filter_map(|modifier| {
        let was_down = previous.contains(modifier);
        let is_down = current.contains(modifier);
        (was_down != is_down).then(|| {
            InputEvent::new(
                EventType::KEY.0,
                modifier_key(modifier).expect("known modifier").code(),
                i32::from(is_down),
            )
        })
    })
    .collect()
}

pub struct LinuxInputInjector {
    pointer: VirtualDevice,
    wheel: VirtualDevice,
    keyboard: VirtualDevice,
    geometry: OutputGeometry,
    modifiers: Modifiers,
    held_keys: HashSet<KeyCode>,
}

impl LinuxInputInjector {
    pub fn new(geometry: OutputGeometry) -> io::Result<Self> {
        if geometry.width == 0
            || geometry.height == 0
            || geometry.desktop_width == 0
            || geometry.desktop_height == 0
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "output and desktop dimensions must be non-zero",
            ));
        }

        let pointer_keys =
            AttributeSet::from_iter([KeyCode::BTN_LEFT, KeyCode::BTN_RIGHT, KeyCode::BTN_MIDDLE]);
        let props = AttributeSet::from_iter([evdev::PropType::DIRECT]);
        let max_x = evdev_abs_axis_max(geometry.desktop_width);
        let max_y = evdev_abs_axis_max(geometry.desktop_height);
        let abs_info_x = AbsInfo::new(-1, 0, max_x, 0, 0, 28);
        let abs_info_y = AbsInfo::new(-1, 0, max_y, 0, 0, 28);
        let abs_x = UinputAbsSetup::new(AbsoluteAxisCode::ABS_X, abs_info_x);
        let abs_y = UinputAbsSetup::new(AbsoluteAxisCode::ABS_Y, abs_info_y);
        let pointer = VirtualDevice::builder()?
            .name("MahoRD Virtual Pointer")
            .with_properties(&props)?
            .with_keys(&pointer_keys)?
            .with_absolute_axis(&abs_x)?
            .with_absolute_axis(&abs_y)?
            .build()?;

        let relative_axes = AttributeSet::from_iter([
            RelativeAxisCode::REL_WHEEL,
            RelativeAxisCode::REL_HWHEEL,
            RelativeAxisCode::REL_WHEEL_HI_RES,
            RelativeAxisCode::REL_HWHEEL_HI_RES,
            RelativeAxisCode::REL_X,
            RelativeAxisCode::REL_Y,
        ]);
        let wheel = VirtualDevice::builder()?
            .name("MahoRD Virtual Wheel")
            .with_relative_axes(&relative_axes)?
            .build()?;

        let mut keyboard_keys = AttributeSet::<KeyCode>::new();
        for code in 0..=126 {
            if let Some(key) = macos_keycode_to_evdev(code) {
                keyboard_keys.insert(key);
            }
        }
        for key in [
            KeyCode::KEY_LEFTSHIFT,
            KeyCode::KEY_LEFTCTRL,
            KeyCode::KEY_LEFTALT,
            KeyCode::KEY_LEFTMETA,
            KeyCode::KEY_CAPSLOCK,
        ] {
            keyboard_keys.insert(key);
        }
        let keyboard = VirtualDevice::builder()?
            .name("MahoRD Virtual Keyboard")
            .with_keys(&keyboard_keys)?
            .build()?;

        Ok(Self {
            pointer,
            wheel,
            keyboard,
            geometry,
            modifiers: Modifiers::empty(),
            held_keys: HashSet::new(),
        })
    }

    pub fn update_geometry(&mut self, geometry: OutputGeometry) {
        self.geometry = geometry;
    }

    pub fn inject(&mut self, event: &WireInputEvent) -> io::Result<()> {
        if !event.x.is_finite()
            || !event.y.is_finite()
            || !event.scroll_dx.is_finite()
            || !event.scroll_dy.is_finite()
        {
            return Ok(());
        }

        match event.event_type {
            InputEventType::MouseMove
            | InputEventType::LeftMouseDragged
            | InputEventType::RightMouseDragged
            | InputEventType::LeftMouseDown
            | InputEventType::LeftMouseUp
            | InputEventType::RightMouseDown
            | InputEventType::RightMouseUp
            | InputEventType::MiddleMouseDown
            | InputEventType::MiddleMouseUp
            | InputEventType::PenMove
            | InputEventType::PenDown
            | InputEventType::PenUp => {
                let (pixel_x, pixel_y) = map_normalized_to_output(event.x, event.y, self.geometry);
                let mut events = vec![
                    *AbsoluteAxisEvent::new(AbsoluteAxisCode::ABS_X, pixel_x as i32),
                    *AbsoluteAxisEvent::new(AbsoluteAxisCode::ABS_Y, pixel_y as i32),
                ];
                let button = match event.event_type {
                    InputEventType::LeftMouseDown | InputEventType::PenDown => {
                        Some((KeyCode::BTN_LEFT, 1))
                    }
                    InputEventType::LeftMouseUp | InputEventType::PenUp => {
                        Some((KeyCode::BTN_LEFT, 0))
                    }
                    InputEventType::RightMouseDown => Some((KeyCode::BTN_RIGHT, 1)),
                    InputEventType::RightMouseUp => Some((KeyCode::BTN_RIGHT, 0)),
                    InputEventType::MiddleMouseDown => Some((KeyCode::BTN_MIDDLE, 1)),
                    InputEventType::MiddleMouseUp => Some((KeyCode::BTN_MIDDLE, 0)),
                    _ => None,
                };
                if let Some((key, value)) = button {
                    events.push(InputEvent::new(EventType::KEY.0, key.code(), value));
                }
                self.pointer.emit(&events)
            }
            InputEventType::GamepadAxis
            | InputEventType::GamepadButtonDown
            | InputEventType::GamepadButtonUp => {
                // Gamepad events safely handled; platform virtual gamepad device carries open requirement
                Ok(())
            }
            InputEventType::RelativeMove => {
                let dx = event.scroll_dx.round() as i32;
                let dy = event.scroll_dy.round() as i32;
                let mut events = Vec::with_capacity(2);
                if dx != 0 {
                    events.push(InputEvent::new(
                        EventType::RELATIVE.0,
                        RelativeAxisCode::REL_X.0,
                        dx,
                    ));
                }
                if dy != 0 {
                    events.push(InputEvent::new(
                        EventType::RELATIVE.0,
                        RelativeAxisCode::REL_Y.0,
                        dy,
                    ));
                }
                if events.is_empty() {
                    Ok(())
                } else {
                    self.wheel.emit(&events)
                }
            }
            InputEventType::Reset => {
                let pointer_events = vec![
                    InputEvent::new(EventType::KEY.0, KeyCode::BTN_LEFT.code(), 0),
                    InputEvent::new(EventType::KEY.0, KeyCode::BTN_RIGHT.code(), 0),
                    InputEvent::new(EventType::KEY.0, KeyCode::BTN_MIDDLE.code(), 0),
                ];
                let _ = self.pointer.emit(&pointer_events);
                let mut kb_events = Vec::new();
                for key in self.held_keys.drain() {
                    kb_events.push(InputEvent::new(EventType::KEY.0, key.code(), 0));
                }
                for key in [
                    KeyCode::KEY_LEFTSHIFT,
                    KeyCode::KEY_RIGHTSHIFT,
                    KeyCode::KEY_LEFTCTRL,
                    KeyCode::KEY_RIGHTCTRL,
                    KeyCode::KEY_LEFTALT,
                    KeyCode::KEY_RIGHTALT,
                    KeyCode::KEY_LEFTMETA,
                    KeyCode::KEY_RIGHTMETA,
                    KeyCode::KEY_CAPSLOCK,
                ] {
                    kb_events.push(InputEvent::new(EventType::KEY.0, key.code(), 0));
                }
                self.modifiers = Modifiers::empty();
                if !kb_events.is_empty() {
                    let _ = self.keyboard.emit(&kb_events);
                }
                Ok(())
            }
            InputEventType::ScrollWheel => {
                let vertical = scroll_units(event.scroll_dy);
                let horizontal = scroll_units(event.scroll_dx);
                let mut events = Vec::with_capacity(4);
                if vertical != 0 {
                    events.push(InputEvent::new(
                        EventType::RELATIVE.0,
                        RelativeAxisCode::REL_WHEEL.0,
                        vertical.signum(),
                    ));
                    events.push(InputEvent::new(
                        EventType::RELATIVE.0,
                        RelativeAxisCode::REL_WHEEL_HI_RES.0,
                        vertical,
                    ));
                }
                if horizontal != 0 {
                    events.push(InputEvent::new(
                        EventType::RELATIVE.0,
                        RelativeAxisCode::REL_HWHEEL.0,
                        horizontal.signum(),
                    ));
                    events.push(InputEvent::new(
                        EventType::RELATIVE.0,
                        RelativeAxisCode::REL_HWHEEL_HI_RES.0,
                        horizontal,
                    ));
                }
                if events.is_empty() {
                    Ok(())
                } else {
                    self.wheel.emit(&events)
                }
            }
            InputEventType::KeyDown | InputEventType::KeyUp => {
                let is_down = event.event_type == InputEventType::KeyDown;
                if let Some(key) = macos_keycode_to_evdev(event.key_code) {
                    let mut events = Vec::with_capacity(6);
                    // Only sync modifiers if not a modifier key itself
                    let is_mod_key = matches!(
                        key,
                        KeyCode::KEY_LEFTSHIFT
                            | KeyCode::KEY_RIGHTSHIFT
                            | KeyCode::KEY_LEFTCTRL
                            | KeyCode::KEY_RIGHTCTRL
                            | KeyCode::KEY_LEFTALT
                            | KeyCode::KEY_RIGHTALT
                            | KeyCode::KEY_LEFTMETA
                            | KeyCode::KEY_RIGHTMETA
                            | KeyCode::KEY_CAPSLOCK
                    );

                    if !is_mod_key {
                        let mod_changes = modifier_events(self.modifiers, event.modifiers);
                        events.extend(mod_changes);
                        self.modifiers = event.modifiers;
                    }

                    events.push(InputEvent::new(
                        EventType::KEY.0,
                        key.code(),
                        i32::from(is_down),
                    ));
                    self.keyboard.emit(&events)?;
                    if is_down {
                        self.held_keys.insert(key);
                    } else {
                        self.held_keys.remove(&key);
                    }
                }
                Ok(())
            }
            InputEventType::FlagsChanged => {
                let events = modifier_events(self.modifiers, event.modifiers);
                self.modifiers = event.modifiers;
                if events.is_empty() {
                    Ok(())
                } else {
                    self.keyboard.emit(&events)
                }
            }
            InputEventType::UnicodeChar => {
                if let Some(ch) = char::from_u32(u32::from(event.key_code)) {
                    if let Some((key, needs_shift)) = ascii_to_evdev(ch) {
                        // One SYN_REPORT per transition: events inside a single
                        // sync report are simultaneous state changes, so batching
                        // the down and the up together drops the keypress.
                        if needs_shift {
                            self.keyboard.emit(&[InputEvent::new(
                                EventType::KEY.0,
                                KeyCode::KEY_LEFTSHIFT.code(),
                                1,
                            )])?;
                        }
                        self.keyboard
                            .emit(&[InputEvent::new(EventType::KEY.0, key.code(), 1)])?;
                        self.keyboard
                            .emit(&[InputEvent::new(EventType::KEY.0, key.code(), 0)])?;
                        if needs_shift {
                            self.keyboard.emit(&[InputEvent::new(
                                EventType::KEY.0,
                                KeyCode::KEY_LEFTSHIFT.code(),
                                0,
                            )])?;
                        }
                    } else {
                        tracing::debug!(
                            code_unit = event.key_code,
                            "Linux uinput skipping non-ASCII Unicode char"
                        );
                    }
                }
                Ok(())
            }
        }
    }
}

fn scroll_units(delta: f32) -> i32 {
    if delta == 0.0 {
        0
    } else {
        let rounded = delta.round() as i32;
        if rounded == 0 {
            delta.signum() as i32
        } else {
            rounded
        }
    }
}

impl Drop for LinuxInputInjector {
    fn drop(&mut self) {
        let reset = WireInputEvent {
            event_type: InputEventType::Reset,
            x: 0.0,
            y: 0.0,
            key_code: 0,
            modifiers: Modifiers::empty(),
            scroll_dx: 0.0,
            scroll_dy: 0.0,
        };
        let _ = self.inject(&reset);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regression_evdev_abs_axis_max_matches_pixel_index_boundary() {
        assert_eq!(evdev_abs_axis_max(6400), 6399);
        assert_eq!(evdev_abs_axis_max(1600), 1599);
        assert_eq!(evdev_abs_axis_max(3840), 3839);
        assert_eq!(evdev_abs_axis_max(1), 0);
        assert_eq!(evdev_abs_axis_max(0), 0);
    }

    #[test]
    fn maps_normalized_coordinates_into_target_output() {
        let geometry = OutputGeometry {
            x: 1920,
            y: 100,
            width: 2560,
            height: 1440,
            desktop_x: 0,
            desktop_y: 0,
            desktop_width: 4480,
            desktop_height: 1540,
        };
        for ((local_x, local_y), expected) in [
            ((0.0, 0.0), (1920, 100)),
            ((2560.0, 1440.0), (4479, 1539)),
            ((-5120.0, 2880.0), (1920, 1539)),
        ] {
            let (wire_x, wire_y) =
                maho_proto::normalize_client_coordinates(local_x, local_y, 2560.0, 1440.0);
            assert_eq!(map_normalized_to_output(wire_x, wire_y, geometry), expected);
        }
        assert_eq!(map_normalized_to_output(0.0, 0.0, geometry), (1920, 1539));
        assert_eq!(map_normalized_to_output(-2.0, 2.0, geometry), (1920, 100));
    }

    #[test]
    fn maps_outputs_left_of_and_above_the_primary_display() {
        // 1920x1080 output placed left of and above a 2560x1440 primary.
        let geometry = OutputGeometry {
            x: -1920,
            y: -1080,
            width: 1920,
            height: 1080,
            desktop_x: -1920,
            desktop_y: -1080,
            desktop_width: 4480,
            desktop_height: 2520,
        };
        // Top-left of that output maps to the desktop origin, not a clamp.
        assert_eq!(map_normalized_to_output(0.0, 1.0, geometry), (0, 0));
        // Bottom-right stays inside the same output instead of collapsing to x=0.
        assert_eq!(map_normalized_to_output(1.0, 0.0, geometry), (1919, 1079));
        assert_eq!(map_normalized_to_output(0.5, 0.5, geometry), (960, 540));
    }

    #[test]
    fn maps_v3_keycodes_and_modifiers() {
        assert_eq!(macos_keycode_to_evdev(0), Some(KeyCode::KEY_A));
        assert_eq!(macos_keycode_to_evdev(36), Some(KeyCode::KEY_ENTER));
        assert_eq!(macos_keycode_to_evdev(123), Some(KeyCode::KEY_LEFT));
        assert_eq!(macos_keycode_to_evdev(u16::MAX), None);

        let flags = Modifiers::SHIFT | Modifiers::COMMAND | Modifiers::CAPS_LOCK;
        let events = modifier_events(Modifiers::empty(), flags);
        assert_eq!(events.len(), 3);
        assert!(events.iter().all(|event| event.value() == 1));
        let releases = modifier_events(flags, Modifiers::empty());
        assert!(releases.iter().all(|event| event.value() == 0));
    }

    #[test]
    fn maps_middle_click_and_reset_button_codes() {
        assert_eq!(KeyCode::BTN_MIDDLE.code(), 0x112);
        assert_eq!(KeyCode::BTN_LEFT.code(), 0x110);
        assert_eq!(KeyCode::BTN_RIGHT.code(), 0x111);
    }

    #[test]
    fn safely_ignores_unicode_char_events() {
        let geometry = OutputGeometry::single_output(1920, 1080);
        let mut injector = LinuxInputInjector::new(geometry).expect("create injector");
        let event = WireInputEvent {
            event_type: maho_proto::InputEventType::UnicodeChar,
            x: 0.5,
            y: 0.5,
            key_code: 0x0041, // UTF-16 code unit for 'A'
            modifiers: Modifiers::empty(),
            scroll_dx: 0.0,
            scroll_dy: 0.0,
        };
        // Should inject successfully for ASCII
        assert!(injector.inject(&event).is_ok());

        // Non-ASCII should also safely succeed without error
        let non_ascii_event = WireInputEvent {
            event_type: maho_proto::InputEventType::UnicodeChar,
            x: 0.5,
            y: 0.5,
            key_code: 0xd55c, // '한'
            modifiers: Modifiers::empty(),
            scroll_dx: 0.0,
            scroll_dy: 0.0,
        };
        assert!(injector.inject(&non_ascii_event).is_ok());
    }

    #[test]
    fn ascii_to_evdev_maps_alphanumerics_and_symbols() {
        assert_eq!(ascii_to_evdev('a'), Some((KeyCode::KEY_A, false)));
        assert_eq!(ascii_to_evdev('A'), Some((KeyCode::KEY_A, true)));
        assert_eq!(ascii_to_evdev('1'), Some((KeyCode::KEY_1, false)));
        assert_eq!(ascii_to_evdev('!'), Some((KeyCode::KEY_1, true)));
        assert_eq!(ascii_to_evdev(' '), Some((KeyCode::KEY_SPACE, false)));
        assert_eq!(ascii_to_evdev('\n'), Some((KeyCode::KEY_ENTER, false)));
        assert_eq!(ascii_to_evdev('한'), None);
    }
}
