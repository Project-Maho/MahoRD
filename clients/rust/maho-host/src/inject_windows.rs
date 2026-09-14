//! Windows input injection through `SendInput`.
//!
//! Protocol v3 keeps the existing macOS `NSEvent.keyCode` values on the wire.
//! This module mirrors that physical-key table to Windows virtual-key values,
//! maps the v3 modifier bits (Option -> Alt, Command -> Windows), and supports
//! normalized absolute pointer motion, relative motion, buttons, and scrolling.
//!
//! CI validates the bindings and pure mapping tests. Runtime QA must confirm
//! behavior across keyboard layouts, mixed-DPI virtual desktops, UAC elevation,
//! and games that reject synthetic input.

use std::{collections::HashSet, io, mem::size_of};

use maho_proto::{InputEvent, InputEventType, Modifiers};
use windows::Win32::UI::{
    Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
        KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE,
        MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
        MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
        MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
        VK_CAPITAL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RCONTROL, VK_RMENU, VK_RSHIFT,
        VK_RWIN,
    },
    WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    },
};

pub use crate::windows_logic::{
    macos_keycode_to_vk, normalize_absolute_pointer, vk_to_macos_keycode, TargetDisplay,
    VirtualDesktop,
};

const WHEEL_DELTA: f32 = 120.0;

pub struct WindowsInputInjector {
    target: TargetDisplay,
    desktop: VirtualDesktop,
    modifiers: Modifiers,
    // Ordinary keys and mouse buttons the peer is currently holding, so a
    // dropped session can release them instead of leaving them stuck.
    active_keys: HashSet<u16>,
    active_buttons: HashSet<u8>,
}

impl WindowsInputInjector {
    pub fn new(target: Option<TargetDisplay>) -> io::Result<Self> {
        let desktop = current_virtual_desktop()?;
        let target = target.unwrap_or_else(|| TargetDisplay::from_virtual_desktop(desktop));
        if target.width == 0 || target.height == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "target display dimensions must be non-zero",
            ));
        }
        Ok(Self {
            target,
            desktop,
            modifiers: Modifiers::empty(),
            active_keys: HashSet::new(),
            active_buttons: HashSet::new(),
        })
    }

    pub fn update_target(&mut self, target: TargetDisplay) {
        self.target = target;
    }

    pub fn refresh_virtual_desktop(&mut self) -> io::Result<()> {
        self.desktop = current_virtual_desktop()?;
        Ok(())
    }

    pub fn move_relative(&mut self, dx: i32, dy: i32) -> io::Result<()> {
        send_inputs(&[mouse_input(dx, dy, 0, MOUSEEVENTF_MOVE)])
    }

    pub fn inject(&mut self, event: &InputEvent) -> io::Result<()> {
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
                let (x, y) =
                    normalize_absolute_pointer(event.x, event.y, self.target, self.desktop);
                let mut flags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
                flags |= match event.event_type {
                    InputEventType::LeftMouseDown | InputEventType::PenDown => MOUSEEVENTF_LEFTDOWN,
                    InputEventType::LeftMouseUp | InputEventType::PenUp => MOUSEEVENTF_LEFTUP,
                    InputEventType::RightMouseDown => MOUSEEVENTF_RIGHTDOWN,
                    InputEventType::RightMouseUp => MOUSEEVENTF_RIGHTUP,
                    InputEventType::MiddleMouseDown => MOUSEEVENTF_MIDDLEDOWN,
                    InputEventType::MiddleMouseUp => MOUSEEVENTF_MIDDLEUP,
                    _ => MOUSE_EVENT_FLAGS(0),
                };
                match event.event_type {
                    InputEventType::LeftMouseDown | InputEventType::PenDown => {
                        self.active_buttons.insert(0);
                    }
                    InputEventType::LeftMouseUp | InputEventType::PenUp => {
                        self.active_buttons.remove(&0);
                    }
                    InputEventType::RightMouseDown => {
                        self.active_buttons.insert(1);
                    }
                    InputEventType::RightMouseUp => {
                        self.active_buttons.remove(&1);
                    }
                    InputEventType::MiddleMouseDown => {
                        self.active_buttons.insert(2);
                    }
                    InputEventType::MiddleMouseUp => {
                        self.active_buttons.remove(&2);
                    }
                    _ => {}
                }
                send_inputs(&[mouse_input(x, y, 0, flags)])
            }
            InputEventType::GamepadAxis
            | InputEventType::GamepadButtonDown
            | InputEventType::GamepadButtonUp => Ok(()),
            InputEventType::RelativeMove => {
                let dx = event.scroll_dx.round() as i32;
                let dy = event.scroll_dy.round() as i32;
                self.move_relative(dx, dy)
            }
            InputEventType::Reset => {
                let mut inputs = Vec::new();
                inputs.push(mouse_input(
                    0,
                    0,
                    0,
                    MOUSEEVENTF_LEFTUP | MOUSEEVENTF_RIGHTUP | MOUSEEVENTF_MIDDLEUP,
                ));
                let mut released: HashSet<u16> = self.active_keys.drain().collect();
                released.extend([VK_LSHIFT, VK_LCONTROL, VK_LMENU, VK_LWIN].map(|vk| vk.0));
                for vk in released {
                    inputs.push(key_input(vk, true));
                }
                self.active_buttons.clear();
                self.modifiers = Modifiers::empty();
                send_inputs(&inputs)
            }
            InputEventType::ScrollWheel => {
                let mut inputs = Vec::with_capacity(2);
                let vertical = scroll_units(event.scroll_dy);
                if vertical != 0 {
                    inputs.push(mouse_input(0, 0, vertical as u32, MOUSEEVENTF_WHEEL));
                }
                let horizontal = scroll_units(event.scroll_dx);
                if horizontal != 0 {
                    inputs.push(mouse_input(0, 0, horizontal as u32, MOUSEEVENTF_HWHEEL));
                }
                send_inputs(&inputs)
            }
            InputEventType::KeyDown | InputEventType::KeyUp => {
                let is_up = event.event_type == InputEventType::KeyUp;
                let mut inputs = Vec::with_capacity(6);
                if let Some(vk) = macos_keycode_to_vk(event.key_code) {
                    // Right-side modifiers and CapsLock are modifier keys too:
                    // treating them as ordinary keys re-synthesizes the modifier
                    // state around them, duplicating presses and breaking AltGr.
                    let is_mod_key = matches!(
                        VIRTUAL_KEY(vk),
                        VK_LSHIFT
                            | VK_LCONTROL
                            | VK_LMENU
                            | VK_LWIN
                            | VK_RSHIFT
                            | VK_RCONTROL
                            | VK_RMENU
                            | VK_RWIN
                            | VK_CAPITAL
                    );
                    if !is_mod_key {
                        inputs.extend(modifier_inputs(self.modifiers, event.modifiers));
                        self.modifiers = event.modifiers;
                    }
                    if is_up {
                        self.active_keys.remove(&vk);
                    } else {
                        self.active_keys.insert(vk);
                    }
                    inputs.push(key_input(vk, is_up));
                }
                send_inputs(&inputs)
            }
            InputEventType::FlagsChanged => {
                let inputs = modifier_inputs(self.modifiers, event.modifiers);
                self.modifiers = event.modifiers;
                send_inputs(&inputs)
            }
            InputEventType::UnicodeChar => {
                // Unicode text input: emit key down and key up with KEYEVENTF_UNICODE
                let inputs = vec![
                    unicode_input(event.key_code, false),
                    unicode_input(event.key_code, true),
                ];
                send_inputs(&inputs)
            }
        }
    }
}

impl Drop for WindowsInputInjector {
    fn drop(&mut self) {
        // A session can end without a peer Reset, so release everything the
        // remote user was holding: modifiers, ordinary keys, and buttons.
        let mut inputs = modifier_inputs(self.modifiers, Modifiers::empty());
        for vk in self.active_keys.drain() {
            inputs.push(key_input(vk, true));
        }
        let mut button_flags = MOUSE_EVENT_FLAGS(0);
        for button in self.active_buttons.drain() {
            button_flags |= match button {
                0 => MOUSEEVENTF_LEFTUP,
                1 => MOUSEEVENTF_RIGHTUP,
                _ => MOUSEEVENTF_MIDDLEUP,
            };
        }
        if button_flags != MOUSE_EVENT_FLAGS(0) {
            inputs.push(mouse_input(0, 0, 0, button_flags));
        }
        let _ = send_inputs(&inputs);
    }
}

fn current_virtual_desktop() -> io::Result<VirtualDesktop> {
    let x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
    if width <= 0 || height <= 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(VirtualDesktop {
        x,
        y,
        width: width as u32,
        height: height as u32,
    })
}

fn modifier_inputs(previous: Modifiers, current: Modifiers) -> Vec<INPUT> {
    [
        (Modifiers::SHIFT, VK_LSHIFT.0),
        (Modifiers::CONTROL, VK_LCONTROL.0),
        (Modifiers::OPTION, VK_LMENU.0),
        (Modifiers::COMMAND, VK_LWIN.0),
    ]
    .into_iter()
    .filter_map(|(modifier, vk)| {
        let was_down = previous.contains(modifier);
        let is_down = current.contains(modifier);
        (was_down != is_down).then(|| key_input(vk, !is_down))
    })
    .collect()
}

fn is_extended_key(vk: u16) -> bool {
    matches!(
        vk,
        0x21..=0x28 | 0x2D | 0x2E | 0x5B | 0x5C | 0x6F | 0xA3 | 0xA5
    )
}

fn key_input(vk: u16, key_up: bool) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }
    if is_extended_key(vk) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn mouse_input(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn scroll_units(delta: f32) -> i32 {
    (delta * WHEEL_DELTA)
        .round()
        .clamp(i32::MIN as f32, i32::MAX as f32) as i32
}

fn unicode_input(code_unit: u16, key_up: bool) -> INPUT {
    let mut flags = KEYEVENTF_UNICODE;
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: code_unit,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send_inputs(inputs: &[INPUT]) -> io::Result<()> {
    if inputs.is_empty() {
        return Ok(());
    }
    let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
    if sent == inputs.len() as u32 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
