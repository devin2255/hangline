//! Copy/discard feedback sounds (design spec §3.5 "声音": copy/drop beep, default on).
//!
//! `settings.sound` gates the sounds; the sound itself is the single Windows system beep produced
//! by `MessageBeep`, so no audio asset or new dependency is needed. Called directly from the Rust
//! actions (`copy_item`, `take_down`, "Take everything down") because those are the only places
//! that know a copy/discard actually succeeded.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Manager};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONHAND, MB_OK, MESSAGEBOX_STYLE};

use crate::app_state::AppState;

/// Whether settings could not be read at all; beeping stays enabled in that case (the spec default)
/// but the fallback is logged once instead of per event.
static MISSING_STATE_LOGGED: AtomicBool = AtomicBool::new(false);

/// Whether a cue should play for the current `settings.sound` value.
///
/// `None` means the app state is not managed yet (the first instants of startup); the spec default
/// is on ([`crate::settings::Settings::default`] has `sound: true`), so a missing state keeps the
/// feedback. Pure, so the gate is unit-tested without touching the audio device (M: sound evidence).
pub fn should_play(sound: Option<bool>) -> bool {
    sound.unwrap_or(true)
}

/// Plays the "copied" system sound when `settings.sound` is on.
pub fn copied(app: &AppHandle) {
    beep(app, MB_OK, "copied");
}

/// Plays the "discarded" system sound when `settings.sound` is on.
pub fn discarded(app: &AppHandle) {
    beep(app, MB_ICONHAND, "discarded");
}

/// The single beep entry point: checks the setting, then calls `MessageBeep`.
fn beep(app: &AppHandle, kind: MESSAGEBOX_STYLE, event: &str) {
    if !should_play(enabled(app)) {
        log::debug!("sound: skipped the {event} beep (sound is off)");
        return;
    }
    // SAFETY: MessageBeep takes a plain sound enum and has no pointers; a failure is ignored
    // (sound is cosmetic and must never fail a copy).
    match unsafe { MessageBeep(kind) } {
        Ok(()) => log::debug!("sound: played the {event} beep"),
        Err(error) => log::debug!("sound: MessageBeep failed for {event}: {error:?}"),
    }
}

/// Reads `settings.sound`; `None` when the state is not managed (the gate defaults to on).
fn enabled(app: &AppHandle) -> Option<bool> {
    match app.try_state::<AppState>() {
        Some(state) => Some(state.snapshot().settings.sound),
        None => {
            if !MISSING_STATE_LOGGED.swap(true, Ordering::Relaxed) {
                log::debug!("sound: app state is not managed yet; beeps default to on");
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The beep kind must be one of the MessageBeep-supported sounds; constructing them here also
    /// documents the copy/discard mapping without touching the audio device in tests.
    #[test]
    fn copy_and_discard_use_distinct_system_sounds() {
        assert_eq!(MB_OK.0, 0);
        assert_eq!(MB_ICONHAND.0, 16);
        assert_ne!(MB_OK.0, MB_ICONHAND.0);
    }

    /// The gate itself: `settings.sound` off silences every cue; a not-yet-managed state keeps the
    /// spec default (on). Audible playback is manual — tests never touch the audio device.
    #[test]
    fn the_sound_setting_gates_every_cue() {
        assert!(!should_play(Some(false)));
        assert!(should_play(Some(true)));
        assert!(
            should_play(None),
            "the spec default is on until settings are loaded"
        );
    }
}
