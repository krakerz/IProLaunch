//! Reads a real gamepad (Steam Input's virtual controller, on a Steam Deck
//! Game Mode shortcut, or any plain USB/Bluetooth pad) directly via `gilrs`
//! (evdev on Linux — no SDL2, no dependency on the terminal at all) and
//! turns button presses into the exact same `crossterm::event::KeyCode`
//! values a keyboard would produce for the equivalent action. This means
//! `tui::on_key`'s dispatch — and every tab's own key handler underneath
//! it — needs zero changes to support a gamepad: a translated button press
//! is indistinguishable from a real keystroke by the time it reaches them.
//!
//! Deliberately edge-triggered (`ButtonPressed` events only, no analog-stick
//! axis handling) — a D-pad is a real, always-present input on every
//! controller this matters for (Steam Deck, Xbox/PlayStation-style pads),
//! and adding continuous-navigation-with-repeat for an analog stick would
//! need its own timing/repeat model for comparatively little benefit.
//!
//! The exact button assignments below are a starting point, not a fixed
//! contract — Steam Input can freely remap any physical input on the pad to
//! any of these virtual buttons (or straight to a keyboard key, bypassing
//! this module entirely) without touching iprolaunch at all.

use crossterm::event::KeyCode;
use gilrs::{Button, Event, EventType, Gilrs};

pub struct GamepadSource {
    gilrs: Option<Gilrs>,
}

impl GamepadSource {
    /// `Gilrs::new()` only fails if the platform backend itself can't be
    /// opened (e.g. no evdev access) — not for "no controller is currently
    /// connected", which is the common case and not an error. Either way,
    /// this degrades to gamepad support simply being off rather than
    /// failing the whole TUI.
    pub fn new() -> Self {
        Self {
            gilrs: Gilrs::new().ok(),
        }
    }

    /// Drains every pending gamepad event and returns the `KeyCode`s they
    /// map to, in order. Called once per event-loop tick, right after the
    /// keyboard poll — non-blocking either way.
    pub fn poll(&mut self) -> Vec<KeyCode> {
        let Some(gilrs) = &mut self.gilrs else {
            return Vec::new();
        };
        let mut codes = Vec::new();
        while let Some(Event { event, .. }) = gilrs.next_event() {
            if let EventType::ButtonPressed(button, _) = event
                && let Some(code) = translate(button)
            {
                codes.push(code);
            }
        }
        codes
    }
}

/// The actual button map. Chosen to cover navigation plus the single most
/// common action per tab (kill/delete, refresh, search, help, tab-switch,
/// quit, winetricks) without needing a different mapping per tab — anything
/// a button maps to here is a no-op on a tab/mode that doesn't use that key,
/// exactly like an unmapped keyboard key already is.
fn translate(button: Button) -> Option<KeyCode> {
    match button {
        Button::DPadUp => Some(KeyCode::Up),
        Button::DPadDown => Some(KeyCode::Down),
        Button::DPadLeft => Some(KeyCode::Left),
        Button::DPadRight => Some(KeyCode::Right),
        // A: confirm — launch/toggle/edit a field/select.
        Button::South => Some(KeyCode::Enter),
        // B: cancel — back out of a popup/editor, clear a locked filter.
        Button::East => Some(KeyCode::Esc),
        // X: the destructive/contextual action — kill (Running) or delete a
        // profile (Library, prompts first same as the `d` key does).
        Button::West => Some(KeyCode::Delete),
        // Y: help, works from any tab already.
        Button::North => Some(KeyCode::Char('?')),
        Button::LeftTrigger => Some(KeyCode::BackTab),
        Button::RightTrigger => Some(KeyCode::Tab),
        Button::Start => Some(KeyCode::Char('q')),
        Button::Select => Some(KeyCode::Char('f')),
        Button::LeftThumb => Some(KeyCode::Char('r')),
        Button::RightThumb => Some(KeyCode::Char('p')),
        // RT: the literal `y` a destructive/rare confirm prompt (delete a
        // profile, rename a slug that also moves its prefix, run
        // winetricks) needs — deliberately a *different* button than A
        // (South, mapped to Enter above), so mashing the everyday confirm
        // button can never delete anything, exactly like Enter alone
        // doesn't confirm these on a keyboard either.
        Button::RightTrigger2 => Some(KeyCode::Char('y')),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpad_maps_to_arrow_keys() {
        assert_eq!(translate(Button::DPadUp), Some(KeyCode::Up));
        assert_eq!(translate(Button::DPadDown), Some(KeyCode::Down));
        assert_eq!(translate(Button::DPadLeft), Some(KeyCode::Left));
        assert_eq!(translate(Button::DPadRight), Some(KeyCode::Right));
    }

    #[test]
    fn face_buttons_map_to_confirm_cancel_and_context_actions() {
        assert_eq!(translate(Button::South), Some(KeyCode::Enter));
        assert_eq!(translate(Button::East), Some(KeyCode::Esc));
        assert_eq!(translate(Button::West), Some(KeyCode::Delete));
        assert_eq!(translate(Button::North), Some(KeyCode::Char('?')));
    }

    #[test]
    fn right_trigger_2_confirms_a_destructive_prompt_distinctly_from_south() {
        assert_eq!(translate(Button::RightTrigger2), Some(KeyCode::Char('y')));
        assert_ne!(translate(Button::RightTrigger2), translate(Button::South));
    }

    #[test]
    fn unmapped_buttons_return_none() {
        assert_eq!(translate(Button::C), None);
        assert_eq!(translate(Button::Z), None);
        assert_eq!(translate(Button::Unknown), None);
    }
}
