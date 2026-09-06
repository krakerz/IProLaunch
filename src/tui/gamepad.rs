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
//!
//! `GamepadSource::poll` also periodically retries `Gilrs::new()` while no
//! gamepad is visible — see its own doc comment for the real, reported
//! Steam Deck Game Mode race this works around.

use std::time::{Duration, Instant};

use crossterm::event::KeyCode;
use gilrs::{Button, Event, EventType, Gilrs};

/// How often `poll` retries `Gilrs::new()` while no gamepad is currently
/// visible — see `should_retry_init`'s doc comment for why this exists.
const RETRY_INTERVAL: Duration = Duration::from_secs(2);

pub struct GamepadSource {
    gilrs: Option<Gilrs>,
    /// When `gilrs` was last (re)created — throttles `should_retry_init`.
    last_init_attempt: Instant,
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
            last_init_attempt: Instant::now(),
        }
    }

    /// Drains every pending gamepad event and returns the `KeyCode`s they
    /// map to, in order. Called once per event-loop tick, right after the
    /// keyboard poll — non-blocking either way.
    ///
    /// Also retries a fresh `Gilrs::new()` roughly every `RETRY_INTERVAL`
    /// while no gamepad is currently visible (see `should_retry_init`) —
    /// a real, reported Steam Deck Game Mode issue: Steam Input's virtual
    /// controller device (its "Gamepad" layout on a non-Steam-game
    /// shortcut) is sometimes invisible to a *freshly opened* udev session
    /// right after Steam Input tears it down and recreates it for a
    /// different app/game — a documented SteamOS-side timing race in
    /// applying the new device's access ACL (the closest confirmed real
    /// analog: `gvalkov/python-evdev#171`, a virtual uinput device losing
    /// its per-user ACL entry right after creation on SteamOS/ChimeraOS —
    /// not a gilrs bug, and not something iprolaunch can fix at the
    /// source). A one-shot `Gilrs::new()` at TUI startup can lose that
    /// race and never notice the controller for the rest of the session —
    /// exactly the reported symptom, where quitting and reopening
    /// iprolaunch again (a fresh `Gilrs::new()`, retried a bit later by
    /// hand) sometimes fixed it. This does the same retry automatically.
    pub fn poll(&mut self) -> Vec<KeyCode> {
        if should_retry_init(self.has_gamepad(), self.last_init_attempt.elapsed()) {
            self.last_init_attempt = Instant::now();
            if let Ok(fresh) = Gilrs::new() {
                self.gilrs = Some(fresh);
            }
        }

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

    fn has_gamepad(&self) -> bool {
        self.gilrs
            .as_ref()
            .is_some_and(|g| g.gamepads().next().is_some())
    }
}

/// Pure gating decision for `GamepadSource::poll`'s retry, kept separate so
/// it's unit-testable without a real `Gilrs`/udev session: only retry when
/// no gamepad is currently connected, and only once `RETRY_INTERVAL` has
/// actually elapsed since the last attempt (never on every single tick —
/// recreating `Gilrs` isn't free, and there's nothing to gain from retrying
/// faster than the real-world race it's working around resolves itself).
fn should_retry_init(has_gamepad: bool, since_last_attempt: Duration) -> bool {
    !has_gamepad && since_last_attempt >= RETRY_INTERVAL
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
        // LT: Library's "add to Steam" — mirrors RT's `y` above (also a
        // trigger, also its own dedicated button rather than reusing A),
        // just for a non-destructive action instead of a confirm.
        Button::LeftTrigger2 => Some(KeyCode::Char('s')),
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
    fn left_trigger_2_maps_to_add_to_steam() {
        assert_eq!(translate(Button::LeftTrigger2), Some(KeyCode::Char('s')));
    }

    #[test]
    fn unmapped_buttons_return_none() {
        assert_eq!(translate(Button::C), None);
        assert_eq!(translate(Button::Z), None);
        assert_eq!(translate(Button::Unknown), None);
    }

    #[test]
    fn should_retry_init_never_fires_while_a_gamepad_is_already_connected() {
        assert!(!should_retry_init(true, Duration::from_secs(999)));
    }

    #[test]
    fn should_retry_init_waits_for_the_full_interval() {
        assert!(!should_retry_init(false, Duration::from_millis(500)));
        assert!(!should_retry_init(
            false,
            RETRY_INTERVAL - Duration::from_millis(1)
        ));
        assert!(should_retry_init(false, RETRY_INTERVAL));
        assert!(should_retry_init(false, Duration::from_secs(999)));
    }
}
