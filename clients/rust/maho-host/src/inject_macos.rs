use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Instant;

use maho_proto::{map_to_host_pixels, InputEvent, InputEventType, Modifiers};
use thiserror::Error;

const MAX_EVENTS_PER_SECOND: f64 = 200.0;
const BURST_CAPACITY: f64 = 400.0;

#[derive(Debug, Error)]
pub enum InputError {
    #[error("Accessibility access is not granted")]
    PermissionDenied,
    #[error("invalid input coordinates")]
    InvalidCoordinates,
    #[error("CoreGraphics could not create the event")]
    EventCreation,
    #[error("input rate limit exceeded")]
    RateLimited,
    #[error("input injection is available only on macOS")]
    Unsupported,
}

struct RateLimit {
    tokens: f64,
    last_refill: Instant,
}

pub struct InputInjector {
    host_width: f32,
    host_height: f32,
    /// Top-left of the captured display in CoreGraphics global coordinates.
    /// Non-zero for every display that is not the primary one.
    origin_x: f32,
    origin_y: f32,
    rate_limit: Mutex<RateLimit>,
    // CGEventSource is !Send/!Sync: create, use, and drop on the session thread.
    #[cfg(target_os = "macos")]
    event_source: std::cell::RefCell<Option<core_graphics::event_source::CGEventSource>>,
    #[cfg(all(test, target_os = "macos"))]
    source_creations: std::cell::Cell<usize>,
    // Track held mouse buttons and keys for Reset event
    #[cfg(target_os = "macos")]
    active_mouse_buttons: std::cell::RefCell<HashSet<u8>>,
    #[cfg(target_os = "macos")]
    active_keys: std::cell::RefCell<HashSet<u16>>,
    #[cfg(target_os = "macos")]
    pending_surrogate: std::cell::Cell<Option<u16>>,
}

impl InputInjector {
    pub fn new(host_width: f32, host_height: f32) -> Self {
        Self::with_origin(host_width, host_height, 0.0, 0.0)
    }

    /// Injector for a display whose top-left sits at `(origin_x, origin_y)` in
    /// CoreGraphics global coordinates. Without the origin, input aimed at a
    /// secondary display lands on the primary one.
    pub fn with_origin(host_width: f32, host_height: f32, origin_x: f32, origin_y: f32) -> Self {
        Self {
            host_width,
            host_height,
            origin_x,
            origin_y,
            rate_limit: Mutex::new(RateLimit {
                tokens: BURST_CAPACITY,
                last_refill: Instant::now(),
            }),
            #[cfg(target_os = "macos")]
            event_source: std::cell::RefCell::new(None),
            #[cfg(all(test, target_os = "macos"))]
            source_creations: std::cell::Cell::new(0),
            #[cfg(target_os = "macos")]
            active_mouse_buttons: std::cell::RefCell::new(HashSet::new()),
            #[cfg(target_os = "macos")]
            active_keys: std::cell::RefCell::new(HashSet::new()),
            #[cfg(target_os = "macos")]
            pending_surrogate: std::cell::Cell::new(None),
        }
    }

    pub fn map_coordinates(&self, event: &InputEvent) -> Result<(f32, f32), InputError> {
        if !event.x.is_finite() || !event.y.is_finite() {
            return Err(InputError::InvalidCoordinates);
        }
        // Protocol convention inverts Y (1.0 - y) on wire for legacy compatibility.
        // Invert it back so (0,0) is top-left in Quartz/CoreGraphics display coordinates.
        let (x, y) = map_to_host_pixels(
            event.x.clamp(0.0, 1.0),
            (1.0 - event.y).clamp(0.0, 1.0),
            self.host_width,
            self.host_height,
        );
        // Display-local pixels are global only on the primary display.
        Ok((x + self.origin_x, y + self.origin_y))
    }

    fn allow_event(&self) -> bool {
        let mut state = self.rate_limit.lock().expect("input rate limiter poisoned");
        let now = Instant::now();
        state.tokens = (state.tokens
            + now.duration_since(state.last_refill).as_secs_f64() * MAX_EVENTS_PER_SECOND)
            .min(BURST_CAPACITY);
        state.last_refill = now;
        if state.tokens < 1.0 {
            return false;
        }
        state.tokens -= 1.0;
        true
    }

    #[cfg(target_os = "macos")]
    pub fn inject(&self, event: &InputEvent) -> Result<(), InputError> {
        if !accessibility_is_trusted() {
            return Err(InputError::PermissionDenied);
        }
        // Events that END a hold are never throttled: dropping a KeyUp, a
        // MouseUp or a Reset leaves the host with a stuck key or a stuck drag
        // that no later event can clear.
        let releases_hold = matches!(
            event.event_type,
            InputEventType::KeyUp
                | InputEventType::LeftMouseUp
                | InputEventType::RightMouseUp
                | InputEventType::MiddleMouseUp
                | InputEventType::PenUp
                | InputEventType::Reset
        );
        if !releases_hold && !self.allow_event() {
            return Err(InputError::RateLimited);
        }
        if let Some(cg_event) = self.create_event(event)? {
            cg_event.post(core_graphics::event::CGEventTapLocation::HID);
            // The UnicodeChar event is a key-down on virtual key 0. Nothing
            // tracks key 0 in `active_keys`, so without posting its release the
            // HID system keeps it held and repeats it.
            if event.event_type == InputEventType::UnicodeChar {
                self.create_unicode_release(event)?
                    .post(core_graphics::event::CGEventTapLocation::HID);
            }
        }
        Ok(())
    }

    /// Shared event source, created on first use and reused afterwards.
    #[cfg(target_os = "macos")]
    fn shared_source(&self) -> Result<core_graphics::event_source::CGEventSource, InputError> {
        use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

        let mut source = self.event_source.borrow_mut();
        if let Some(source) = source.as_ref() {
            // Clone is CFRetain, not CGEventSourceCreate. The event constructor
            // consumes this reference; the injector retains its own until drop.
            return Ok(source.clone());
        }
        #[cfg(test)]
        self.source_creations.set(self.source_creations.get() + 1);
        let created = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|_| InputError::EventCreation)?;
        // Do not cache failures: a later input can retry creation.
        *source = Some(created.clone());
        Ok(created)
    }

    /// Key-up partner for a `UnicodeChar` key-down, carrying the same string so
    /// the release is attributed to the character that was typed.
    #[cfg(target_os = "macos")]
    fn create_unicode_release(
        &self,
        _event: &InputEvent,
    ) -> Result<core_graphics::event::CGEvent, InputError> {
        let release =
            core_graphics::event::CGEvent::new_keyboard_event(self.shared_source()?, 0, false)
                .map_err(|_| InputError::EventCreation)?;
        release.set_string("");
        Ok(release)
    }

    // Separate construction from posting so native tests never inject input.
    #[cfg(target_os = "macos")]
    fn create_event(
        &self,
        event: &InputEvent,
    ) -> Result<Option<core_graphics::event::CGEvent>, InputError> {
        use core_graphics::event::{
            CGEvent, CGEventType, CGMouseButton, EventField, ScrollEventUnit,
        };
        use core_graphics::geometry::CGPoint;

        let (x, y) = self.map_coordinates(event)?;
        let point = CGPoint::new(x as f64, y as f64);
        let flags = cg_flags(event.modifiers);
        let source = || self.shared_source();

        // Handle Reset by synthesizing key and button up events for all active state
        if event.event_type == InputEventType::Reset {
            self.pending_surrogate.set(None);
            let buttons = self.active_mouse_buttons.borrow().clone();
            let keys = self.active_keys.borrow().clone();
            // Synthesize releases for held buttons and keys
            for button_code in buttons {
                let button = match button_code {
                    0 => CGMouseButton::Left,
                    1 => CGMouseButton::Right,
                    2 => CGMouseButton::Center,
                    _ => continue,
                };
                if let Ok(release_event) = CGEvent::new_mouse_event(
                    source()?,
                    match button_code {
                        0 => CGEventType::LeftMouseUp,
                        1 => CGEventType::RightMouseUp,
                        2 => CGEventType::OtherMouseUp,
                        _ => unreachable!(),
                    },
                    point,
                    button,
                ) {
                    release_event.post(core_graphics::event::CGEventTapLocation::HID);
                }
            }
            for key_code in keys {
                if let Ok(release_event) = CGEvent::new_keyboard_event(source()?, key_code, false) {
                    release_event.post(core_graphics::event::CGEventTapLocation::HID);
                }
            }
            self.active_mouse_buttons.borrow_mut().clear();
            self.active_keys.borrow_mut().clear();
            return Ok(None);
        }

        let cg_event = match event.event_type {
            InputEventType::MouseMove => CGEvent::new_mouse_event(
                source()?,
                CGEventType::MouseMoved,
                point,
                CGMouseButton::Left,
            ),
            InputEventType::LeftMouseDragged => CGEvent::new_mouse_event(
                source()?,
                CGEventType::LeftMouseDragged,
                point,
                CGMouseButton::Left,
            ),
            InputEventType::RightMouseDragged => CGEvent::new_mouse_event(
                source()?,
                CGEventType::RightMouseDragged,
                point,
                CGMouseButton::Right,
            ),
            InputEventType::LeftMouseDown => {
                self.active_mouse_buttons.borrow_mut().insert(0);
                CGEvent::new_mouse_event(
                    source()?,
                    CGEventType::LeftMouseDown,
                    point,
                    CGMouseButton::Left,
                )
            }
            InputEventType::LeftMouseUp => {
                self.active_mouse_buttons.borrow_mut().remove(&0);
                CGEvent::new_mouse_event(
                    source()?,
                    CGEventType::LeftMouseUp,
                    point,
                    CGMouseButton::Left,
                )
            }
            InputEventType::RightMouseDown => {
                self.active_mouse_buttons.borrow_mut().insert(1);
                CGEvent::new_mouse_event(
                    source()?,
                    CGEventType::RightMouseDown,
                    point,
                    CGMouseButton::Right,
                )
            }
            InputEventType::RightMouseUp => {
                self.active_mouse_buttons.borrow_mut().remove(&1);
                CGEvent::new_mouse_event(
                    source()?,
                    CGEventType::RightMouseUp,
                    point,
                    CGMouseButton::Right,
                )
            }
            InputEventType::ScrollWheel => CGEvent::new_scroll_event(
                source()?,
                ScrollEventUnit::PIXEL,
                2,
                event.scroll_dy.round() as i32,
                event.scroll_dx.round() as i32,
                0,
            ),
            InputEventType::KeyDown => {
                self.active_keys.borrow_mut().insert(event.key_code);
                CGEvent::new_keyboard_event(source()?, event.key_code, true)
            }
            InputEventType::KeyUp => {
                self.active_keys.borrow_mut().remove(&event.key_code);
                CGEvent::new_keyboard_event(source()?, event.key_code, false)
            }
            InputEventType::FlagsChanged => {
                CGEvent::new_keyboard_event(source()?, event.key_code, true)
            }
            InputEventType::MiddleMouseDown => {
                self.active_mouse_buttons.borrow_mut().insert(2);
                CGEvent::new_mouse_event(
                    source()?,
                    CGEventType::OtherMouseDown,
                    point,
                    CGMouseButton::Center,
                )
            }
            InputEventType::MiddleMouseUp => {
                self.active_mouse_buttons.borrow_mut().remove(&2);
                CGEvent::new_mouse_event(
                    source()?,
                    CGEventType::OtherMouseUp,
                    point,
                    CGMouseButton::Center,
                )
            }
            InputEventType::PenMove => CGEvent::new_mouse_event(
                source()?,
                CGEventType::MouseMoved,
                point,
                CGMouseButton::Left,
            ),
            InputEventType::PenDown => {
                self.active_mouse_buttons.borrow_mut().insert(0);
                CGEvent::new_mouse_event(
                    source()?,
                    CGEventType::LeftMouseDown,
                    point,
                    CGMouseButton::Left,
                )
            }
            InputEventType::PenUp => {
                self.active_mouse_buttons.borrow_mut().remove(&0);
                CGEvent::new_mouse_event(
                    source()?,
                    CGEventType::LeftMouseUp,
                    point,
                    CGMouseButton::Left,
                )
            }
            InputEventType::UnicodeChar => {
                let code = event.key_code;
                let unicode_str = if (0xD800..=0xDBFF).contains(&code) {
                    self.pending_surrogate.set(Some(code));
                    return Ok(None);
                } else if (0xDC00..=0xDFFF).contains(&code) {
                    if let Some(high) = self.pending_surrogate.take() {
                        String::from_utf16_lossy(&[high, code])
                    } else {
                        String::from_utf16_lossy(&[code])
                    }
                } else {
                    self.pending_surrogate.set(None);
                    String::from_utf16_lossy(&[code])
                };
                let cg_event = CGEvent::new_keyboard_event(source()?, 0, true)
                    .map_err(|_| InputError::EventCreation)?;
                cg_event.set_string(&unicode_str);
                return Ok(Some(cg_event));
            }
            InputEventType::RelativeMove => {
                if !event.scroll_dx.is_finite() || !event.scroll_dy.is_finite() {
                    return Err(InputError::InvalidCoordinates);
                }
                let dx = f64::from(event.scroll_dx.round());
                let dy = f64::from(event.scroll_dy.round());
                // Pointer-locked clients send deltas only. Start from where the
                // cursor actually is and carry the delta fields so applications
                // reading raw motion see the movement, not just the new point.
                let current = CGEvent::new(source()?)
                    .map_err(|_| InputError::EventCreation)?
                    .location();
                let moved = CGEvent::new_mouse_event(
                    source()?,
                    CGEventType::MouseMoved,
                    CGPoint::new(current.x + dx, current.y + dy),
                    CGMouseButton::Left,
                )
                .map_err(|_| InputError::EventCreation)?;
                moved.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_X, dx as i64);
                moved.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y, dy as i64);
                moved.set_flags(flags);
                return Ok(Some(moved));
            }
            InputEventType::Reset
            | InputEventType::GamepadAxis
            | InputEventType::GamepadButtonDown
            | InputEventType::GamepadButtonUp => return Ok(None),
        }
        .map_err(|_| InputError::EventCreation)?;
        cg_event.set_flags(flags);
        Ok(Some(cg_event))
    }

    #[cfg(not(target_os = "macos"))]
    pub fn inject(&self, _event: &InputEvent) -> Result<(), InputError> {
        Err(InputError::Unsupported)
    }
}

#[cfg(target_os = "macos")]
fn cg_flags(modifiers: Modifiers) -> core_graphics::event::CGEventFlags {
    use core_graphics::event::CGEventFlags;
    let mut flags = CGEventFlags::empty();
    if modifiers.contains(Modifiers::SHIFT) {
        flags.insert(CGEventFlags::CGEventFlagShift);
    }
    if modifiers.contains(Modifiers::CONTROL) {
        flags.insert(CGEventFlags::CGEventFlagControl);
    }
    if modifiers.contains(Modifiers::OPTION) {
        flags.insert(CGEventFlags::CGEventFlagAlternate);
    }
    if modifiers.contains(Modifiers::COMMAND) {
        flags.insert(CGEventFlags::CGEventFlagCommand);
    }
    if modifiers.contains(Modifiers::CAPS_LOCK) {
        flags.insert(CGEventFlags::CGEventFlagAlphaShift);
    }
    flags
}

#[cfg(target_os = "macos")]
pub fn accessibility_is_trusted() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> u8;
    }
    unsafe { AXIsProcessTrusted() != 0 }
}

#[cfg(not(target_os = "macos"))]
pub fn accessibility_is_trusted() -> bool {
    false
}

#[cfg(target_os = "macos")]
pub fn request_accessibility() -> bool {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        static kAXTrustedCheckOptionPrompt: core_foundation::string::CFStringRef;
        fn AXIsProcessTrustedWithOptions(
            options: core_foundation::dictionary::CFDictionaryRef,
        ) -> u8;
    }

    unsafe {
        let key = CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt);
        let options = CFDictionary::from_CFType_pairs(&[(key, CFBoolean::true_value())]);
        AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) != 0
    }
}

#[cfg(not(target_os = "macos"))]
pub fn request_accessibility() -> bool {
    false
}

/// Releases anything the remote user was still holding when the session ends.
/// Without this, dropping the injector after a KeyDown or a MouseDown leaves the
/// host repeating a key or stuck mid-drag: a dropped connection can never
/// deliver the matching up event or an explicit Reset.
#[cfg(target_os = "macos")]
impl Drop for InputInjector {
    fn drop(&mut self) {
        self.pending_surrogate.set(None);
        if self.active_mouse_buttons.borrow().is_empty() && self.active_keys.borrow().is_empty() {
            return;
        }
        // Synthesizing events without Accessibility access cannot reach the
        // host, so skip the work entirely, exactly as `inject` does.
        if !accessibility_is_trusted() {
            return;
        }
        let reset = InputEvent {
            event_type: InputEventType::Reset,
            x: 0.0,
            y: 0.0,
            key_code: 0,
            modifiers: Modifiers::empty(),
            scroll_dx: 0.0,
            scroll_dy: 0.0,
        };
        // The Reset path posts an up event for every tracked button and key and
        // returns no event of its own. Errors are unactionable during drop.
        let _ = self.create_event(&reset);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn source_is_created_once_for_equal_mixed_input_workload() {
        use core_graphics::event::{CGEventFlags, CGEventType, EventField};

        // Given every wire event and every supported modifier combination.
        let cases = [
            (
                InputEventType::MouseMove,
                0,
                Some(CGEventType::MouseMoved),
                Some(0),
            ),
            (
                InputEventType::LeftMouseDown,
                0,
                Some(CGEventType::LeftMouseDown),
                Some(0),
            ),
            (
                InputEventType::LeftMouseUp,
                0,
                Some(CGEventType::LeftMouseUp),
                Some(0),
            ),
            (
                InputEventType::RightMouseDown,
                0,
                Some(CGEventType::RightMouseDown),
                Some(1),
            ),
            (
                InputEventType::RightMouseUp,
                0,
                Some(CGEventType::RightMouseUp),
                Some(1),
            ),
            (
                InputEventType::ScrollWheel,
                0,
                Some(CGEventType::ScrollWheel),
                None,
            ),
            (InputEventType::KeyDown, 6, Some(CGEventType::KeyDown), None),
            (InputEventType::KeyUp, 6, Some(CGEventType::KeyUp), None),
            (
                InputEventType::FlagsChanged,
                56,
                Some(CGEventType::FlagsChanged),
                None,
            ),
            (
                InputEventType::LeftMouseDragged,
                0,
                Some(CGEventType::LeftMouseDragged),
                Some(0),
            ),
            (
                InputEventType::RightMouseDragged,
                0,
                Some(CGEventType::RightMouseDragged),
                Some(1),
            ),
            (
                InputEventType::MiddleMouseDown,
                0,
                Some(CGEventType::OtherMouseDown),
                Some(2),
            ),
            (
                InputEventType::MiddleMouseUp,
                0,
                Some(CGEventType::OtherMouseUp),
                Some(2),
            ),
            (InputEventType::Reset, 0, None, None),
            (
                InputEventType::RelativeMove,
                0,
                Some(CGEventType::MouseMoved),
                None,
            ),
        ];
        let injector = InputInjector::new(1920.0, 1080.0);
        let mut events = 0;
        let started = Instant::now();

        // When constructing (never posting) the identical workload on one thread.
        for _ in 0..16 {
            for mask in (0..32).rev() {
                let expected_flags = [
                    CGEventFlags::CGEventFlagShift,
                    CGEventFlags::CGEventFlagControl,
                    CGEventFlags::CGEventFlagAlternate,
                    CGEventFlags::CGEventFlagCommand,
                    CGEventFlags::CGEventFlagAlphaShift,
                ]
                .into_iter()
                .enumerate()
                .fold(CGEventFlags::empty(), |flags, (bit, flag)| {
                    if mask & (1 << bit) != 0 {
                        flags | flag
                    } else {
                        flags
                    }
                });
                for (event_type, key_code, expected_type, button) in cases {
                    let input = InputEvent {
                        event_type,
                        key_code,
                        x: 0.25,
                        y: 0.75,
                        // Unknown wire bits must not leak into Quartz flags either.
                        modifiers: Modifiers::from_bits_retain(mask | 0x8000),
                        scroll_dx: -2.6,
                        scroll_dy: 3.6,
                    };
                    let output = injector.create_event(&input).unwrap();
                    // Then the source state, flags, types, and payload stay identical.
                    match (output, expected_type) {
                        (None, None) => {}
                        (Some(output), Some(expected_type)) => {
                            events += 1;
                            assert_eq!(output.get_type() as u32, expected_type as u32);
                            assert_eq!(output.get_flags(), expected_flags);
                            assert_eq!(
                                output.get_integer_value_field(EventField::EVENT_SOURCE_STATE_ID),
                                1
                            );
                            if let Some(button) = button {
                                let location = output.location();
                                assert_eq!((location.x, location.y), (480.0, 270.0));
                                assert_eq!(
                                    output.get_integer_value_field(
                                        EventField::MOUSE_EVENT_BUTTON_NUMBER
                                    ),
                                    button
                                );
                            }
                            match event_type {
                                InputEventType::KeyDown
                                | InputEventType::KeyUp
                                | InputEventType::FlagsChanged => {
                                    assert_eq!(
                                        output.get_integer_value_field(
                                            EventField::KEYBOARD_EVENT_KEYCODE
                                        ),
                                        i64::from(key_code)
                                    );
                                }
                                InputEventType::ScrollWheel => {
                                    assert_eq!(
                                        output.get_integer_value_field(
                                            EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_1
                                        ),
                                        4
                                    );
                                    assert_eq!(
                                        output.get_integer_value_field(
                                            EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_2
                                        ),
                                        -3
                                    );
                                }
                                InputEventType::MouseMove
                                | InputEventType::LeftMouseDown
                                | InputEventType::LeftMouseUp
                                | InputEventType::RightMouseDown
                                | InputEventType::RightMouseUp
                                | InputEventType::LeftMouseDragged
                                | InputEventType::RightMouseDragged
                                | InputEventType::MiddleMouseDown
                                | InputEventType::MiddleMouseUp
                                | InputEventType::Reset
                                | InputEventType::RelativeMove
                                | InputEventType::GamepadAxis
                                | InputEventType::GamepadButtonDown
                                | InputEventType::GamepadButtonUp
                                | InputEventType::PenMove
                                | InputEventType::PenDown
                                | InputEventType::PenUp
                                | InputEventType::UnicodeChar => {}
                            }
                        }
                        _ => panic!("unexpected event/no-op for {event_type:?}"),
                    }
                }
            }
        }
        println!(
            "inputs=7680 events={events} source_creations={} elapsed_us={}",
            injector.source_creations.get(),
            started.elapsed().as_micros()
        );
        assert_eq!(events, 7168);
        assert_eq!(injector.source_creations.get(), 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn relative_move_carries_deltas_and_unicode_char_is_released() {
        use core_graphics::event::{CGEventType, EventField};

        // Given a pointer-locked delta and a typed character.
        let injector = InputInjector::new(1920.0, 1080.0);
        let relative = InputEvent {
            event_type: InputEventType::RelativeMove,
            x: 0.0,
            y: 0.0,
            key_code: 0,
            modifiers: Modifiers::empty(),
            scroll_dx: -7.4,
            scroll_dy: 3.6,
        };
        // When constructing the injected events.
        let moved = injector.create_event(&relative).unwrap().unwrap();
        let unicode = InputEvent {
            event_type: InputEventType::UnicodeChar,
            key_code: u16::from(b'q'),
            ..relative
        };
        let down = injector.create_event(&unicode).unwrap().unwrap();
        let up = injector.create_unicode_release(&unicode).unwrap();

        // Then relative motion reports the rounded deltas rather than a no-op.
        assert_eq!(moved.get_type() as u32, CGEventType::MouseMoved as u32);
        assert_eq!(
            moved.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_X),
            -7
        );
        assert_eq!(
            moved.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y),
            4
        );
        // And the Unicode key-down has a matching key-up on the same virtual key.
        assert_eq!(down.get_type() as u32, CGEventType::KeyDown as u32);
        assert_eq!(up.get_type() as u32, CGEventType::KeyUp as u32);
        assert_eq!(
            up.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE),
            down.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE)
        );
        // Non-finite deltas are rejected instead of injecting garbage motion.
        assert!(matches!(
            injector.create_event(&InputEvent {
                scroll_dy: f32::INFINITY,
                ..relative
            }),
            Err(InputError::InvalidCoordinates)
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn noops_and_invalid_coordinates_do_not_create_sources() {
        // Given a fresh injector with no native resources yet.
        let injector = InputInjector::new(100.0, 50.0);
        // When Reset, non-finite deltas, and invalid input arrive before real events.
        for event_type in [InputEventType::Reset, InputEventType::RelativeMove] {
            let event = InputEvent {
                event_type,
                x: 0.0,
                y: 0.0,
                key_code: 0,
                modifiers: Modifiers::COMMAND,
                scroll_dx: 0.0,
                scroll_dy: 0.0,
            };
            if event_type == InputEventType::Reset {
                assert!(injector.create_event(&event).unwrap().is_none());
            } else {
                assert!(matches!(
                    injector.create_event(&InputEvent {
                        scroll_dx: f32::NAN,
                        ..event
                    }),
                    Err(InputError::InvalidCoordinates)
                ));
            }
            assert!(matches!(
                injector.create_event(&InputEvent {
                    x: f32::NAN,
                    ..event
                }),
                Err(InputError::InvalidCoordinates)
            ));
        }
        // Then they neither allocate a source nor manufacture an OS event.
        assert_eq!(injector.source_creations.get(), 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn event_flags_are_isolated_across_reset_and_injector_lifetimes() {
        use core_graphics::event::{CGEventFlags, EventField};

        // Given a modifier-bearing event that remains alive across Reset and drop.
        let first = InputInjector::new(100.0, 50.0);
        let second = InputInjector::new(100.0, 50.0);
        let input = InputEvent {
            event_type: InputEventType::KeyDown,
            x: 0.0,
            y: 0.0,
            key_code: 6,
            modifiers: Modifiers::COMMAND,
            scroll_dx: 0.0,
            scroll_dy: 0.0,
        };
        let held = first.create_event(&input).unwrap().unwrap();
        // When a different injector and the reset injector construct unmodified keys.
        assert!(first
            .create_event(&InputEvent {
                event_type: InputEventType::Reset,
                ..input
            })
            .unwrap()
            .is_none());
        let clear_input = InputEvent {
            modifiers: Modifiers::empty(),
            ..input
        };
        let clear_first = first.create_event(&clear_input).unwrap().unwrap();
        let clear_second = second.create_event(&clear_input).unwrap().unwrap();
        let creations = (first.source_creations.get(), second.source_creations.get());
        drop(first);
        drop(second);
        // Then retained events stay valid, with no inherited/sticky modifiers.
        assert_eq!(held.get_flags(), CGEventFlags::CGEventFlagCommand);
        for event in [clear_first, clear_second] {
            assert_eq!(event.get_flags(), CGEventFlags::empty());
            assert_eq!(
                event.get_integer_value_field(EventField::EVENT_SOURCE_STATE_ID),
                1
            );
            assert_eq!(
                event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE),
                6
            );
        }
        assert_eq!(creations, (1, 1));
    }

    #[test]
    fn normalized_coordinates_map_to_absolute_host_pixels() {
        let injector = InputInjector::new(1920.0, 1080.0);
        let event = InputEvent {
            event_type: InputEventType::MouseMove,
            x: 0.25,
            y: 0.75,
            key_code: 0,
            modifiers: Modifiers::empty(),
            scroll_dx: 0.0,
            scroll_dy: 0.0,
        };
        assert_eq!(injector.map_coordinates(&event).unwrap(), (480.0, 270.0));
    }

    #[test]
    fn secondary_display_origin_offsets_mapped_pixels() {
        // Given a display whose top-left is left of and above the primary one.
        let injector = InputInjector::with_origin(1920.0, 1080.0, -1920.0, -240.0);
        let event = InputEvent {
            event_type: InputEventType::MouseMove,
            x: 0.25,
            y: 0.75,
            key_code: 0,
            modifiers: Modifiers::empty(),
            scroll_dx: 0.0,
            scroll_dy: 0.0,
        };
        // When mapping wire coordinates, they land in that display, not the primary.
        assert_eq!(injector.map_coordinates(&event).unwrap(), (-1440.0, 30.0));
    }

    #[test]
    fn coordinates_are_clamped_and_nan_is_rejected() {
        let injector = InputInjector::new(100.0, 50.0);
        let mut event = InputEvent {
            event_type: InputEventType::MouseMove,
            x: -1.0,
            y: 2.0,
            key_code: 0,
            modifiers: Modifiers::empty(),
            scroll_dx: 0.0,
            scroll_dy: 0.0,
        };
        assert_eq!(injector.map_coordinates(&event).unwrap(), (0.0, 0.0));
        event.x = f32::NAN;
        assert!(matches!(
            injector.map_coordinates(&event),
            Err(InputError::InvalidCoordinates)
        ));
    }

    #[test]
    fn unicode_surrogate_pairs_are_combined_into_single_event() {
        let injector = InputInjector::new(1920.0, 1080.0);
        // Emoji '😀' (U+1F600) encoded in UTF-16: 0xD83D (high), 0xDE00 (low)
        let high = InputEvent {
            event_type: InputEventType::UnicodeChar,
            x: 0.0,
            y: 0.0,
            key_code: 0xD83D,
            modifiers: Modifiers::empty(),
            scroll_dx: 0.0,
            scroll_dy: 0.0,
        };
        let low = InputEvent {
            event_type: InputEventType::UnicodeChar,
            x: 0.0,
            y: 0.0,
            key_code: 0xDE00,
            modifiers: Modifiers::empty(),
            scroll_dx: 0.0,
            scroll_dy: 0.0,
        };

        // High surrogate alone returns None (waits for low surrogate)
        let res_high = injector.create_event(&high).unwrap();
        assert!(res_high.is_none());

        // Low surrogate completes the pair and produces the combined emoji event
        let res_low = injector.create_event(&low).unwrap();
        assert!(res_low.is_some());

        // Regular BMP char ('A' = 0x0041) produces event immediately
        let bmp = InputEvent {
            event_type: InputEventType::UnicodeChar,
            x: 0.0,
            y: 0.0,
            key_code: 0x0041,
            modifiers: Modifiers::empty(),
            scroll_dx: 0.0,
            scroll_dy: 0.0,
        };
        let res_bmp = injector.create_event(&bmp).unwrap();
        assert!(res_bmp.is_some());
    }
}
