use enigo::{Enigo, Key, Keyboard, Mouse, Settings};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

#[cfg(target_os = "windows")]
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RCONTROL, VK_RMENU, VK_RSHIFT,
    VK_RWIN,
};

/// Depth of in-flight keystroke injections below (paste chords, direct
/// typing, auto-submit). The injected events carry no marker a low-level
/// hook can read (`LLKHF_INJECTED` is not exposed by handy-keys), so the
/// shortcut manager consults this flag and drops raw events for the
/// duration instead of letting our own synthetics re-enter the matcher.
static INJECTION_DEPTH: AtomicUsize = AtomicUsize::new(0);

/// True while `input` is synthesizing keystrokes. Events seen by the global
/// hook in this window are (almost always) our own; the odd real keystroke
/// inside the short window is dropped along with them — an accepted
/// trade-off of the flag being shared rather than tagged per-event.
pub(crate) fn is_injection_active() -> bool {
    INJECTION_DEPTH.load(Ordering::SeqCst) > 0
}

/// RAII guard marking an injection window for [`is_injection_active`].
/// Depth-counted so overlapping injections stay covered.
pub(crate) struct InjectionGuard;

impl InjectionGuard {
    pub(crate) fn begin() -> Self {
        INJECTION_DEPTH.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for InjectionGuard {
    fn drop(&mut self) {
        INJECTION_DEPTH.fetch_sub(1, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// FR-005-01 — physical modifier release before injection
//
// A paste/type chord must not blend with a shortcut key the user has not
// physically released yet: still-held `Win` would turn our `Ctrl+V` into
// `Win+V` (clipboard history popup) and still-held `Ctrl+Alt` into
// `Ctrl+Alt+V` (AC-005-08). So every injection entry point waits — up to
// 500 ms, polling `GetAsyncKeyState` — for the chord-changing modifiers to
// come up, then forces synthetic key-ups for any still held.
//
// Only modifiers are waited on: a held non-modifier key cannot change what
// an injected chord means. The wait must run *before* `InjectionGuard`
// begins, otherwise the user's own (real) releases would be dropped by the
// hook filter for the whole wait window.
// ---------------------------------------------------------------------------

/// `dwExtraInfo` marker ("TCMK" in ASCII) tagging input we synthesize so it
/// stays attributable in hook logs. The low-level hook cannot read
/// `LLKHF_INJECTED` through handy-keys, so [`is_injection_active`] remains
/// the actual self-filter.
#[cfg(target_os = "windows")]
const INJECTION_EXTRA_INFO: usize = 0x5443_4D4B;

/// How long [`await_shortcut_modifier_release`] waits for the user to lift
/// the shortcut modifiers before forcing synthetic key-ups (FR-005-01).
#[cfg(target_os = "windows")]
const MODIFIER_RELEASE_TIMEOUT: Duration = Duration::from_millis(500);

/// Poll interval for the physical-release wait.
#[cfg(target_os = "windows")]
const MODIFIER_RELEASE_POLL: Duration = Duration::from_millis(10);

/// Virtual keys whose held state changes the meaning of an injected chord:
/// both sides of Shift, Ctrl, Alt (`VK_MENU`) and Win.
#[cfg(target_os = "windows")]
const CHORD_MODIFIER_VKS: &[u16] = &[
    VK_LSHIFT.0,
    VK_RSHIFT.0,
    VK_LCONTROL.0,
    VK_RCONTROL.0,
    VK_LMENU.0,
    VK_RMENU.0,
    VK_LWIN.0,
    VK_RWIN.0,
];

/// Pure decision loop behind [`await_shortcut_modifier_release`]: polls
/// `is_down` over `vks` until all are released or `timeout` elapses, then
/// returns the VKs still held (in input order). Clock and sleep are
/// injected so tests can drive it with canned state.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn poll_until_released(
    vks: &[u16],
    timeout: Duration,
    poll_interval: Duration,
    mut is_down: impl FnMut(u16) -> bool,
    mut sleep: impl FnMut(Duration),
    mut now: impl FnMut() -> Instant,
) -> Vec<u16> {
    let started = now();
    loop {
        let held: Vec<u16> = vks.iter().copied().filter(|&vk| is_down(vk)).collect();
        if held.is_empty() || now().saturating_duration_since(started) >= timeout {
            return held;
        }
        sleep(poll_interval);
    }
}

/// Whether `vk` is physically held right now. `GetAsyncKeyState` reports a
/// negative value (bit 15 set) while the key is down.
#[cfg(target_os = "windows")]
fn vk_physically_down(vk: u16) -> bool {
    // SAFETY: GetAsyncKeyState accepts any virtual-key value and has no
    // preconditions.
    let state = unsafe { GetAsyncKeyState(i32::from(vk)) };
    state < 0
}

/// Injects a key-up for every VK in `held` so a physical hold stops
/// contributing to the modifiers the focused app sees. The user's eventual
/// real key-up then just repeats an already-released edge — harmless.
/// Events carry [`INJECTION_EXTRA_INFO`] and are sent under an
/// [`InjectionGuard`] so our own hook drops them.
#[cfg(target_os = "windows")]
fn send_modifier_key_ups(held: &[u16]) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY,
    };

    let _guard = InjectionGuard::begin();
    let inputs: Vec<INPUT> = held
        .iter()
        .map(|&vk| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: 0,
                    dwFlags: KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: INJECTION_EXTRA_INFO,
                },
            },
        })
        .collect();
    if inputs.is_empty() {
        return;
    }
    // SAFETY: `inputs` is a valid slice that outlives the call.
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        // Rare (e.g. UIPI filtering): the injection proceeds anyway; worst
        // case is the pre-FR-005-01 behavior of the modifier blending in.
        log::warn!(
            "modifier key-up injection incomplete ({}/{} sent)",
            sent,
            inputs.len()
        );
    }
}

/// FR-005-01: wait for the physical release of every chord-modifier the
/// user may still be holding from the shortcut that fired (up to
/// [`MODIFIER_RELEASE_TIMEOUT`], polling `GetAsyncKeyState`), then inject
/// synthetic key-ups for any still held. Call immediately before
/// synthesizing keystrokes — after it returns, no held modifier can turn
/// `Ctrl+V` into `Win+V`/`Ctrl+Alt+V` (AC-005-08) or typed characters into
/// shortcuts.
///
/// No-op on other platforms: v1 is Windows-first and neither enigo backend
/// there exposes an async key-state query.
pub(crate) fn await_shortcut_modifier_release() {
    #[cfg(target_os = "windows")]
    {
        let still_held = poll_until_released(
            CHORD_MODIFIER_VKS,
            MODIFIER_RELEASE_TIMEOUT,
            MODIFIER_RELEASE_POLL,
            vk_physically_down,
            std::thread::sleep,
            Instant::now,
        );
        if still_held.is_empty() {
            return;
        }
        log::warn!(
            "forcing synthetic key-up for still-held modifier VK(s) {still_held:02X?} before injection"
        );
        send_modifier_key_ups(&still_held);
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::Key;
    use log::{debug, warn};
    use std::ffi::c_void;

    type TisInputSourceRef = *const c_void;
    type CfDataRef = *const c_void;
    type CfStringRef = *const c_void;

    // kVK_ANSI_V. This is the behavior Handy used before layout-aware
    // resolution and remains the safest fallback if macOS cannot expose the
    // active layout.
    const ANSI_V_KEYCODE: u16 = 9;
    const KEYCODE_COUNT: u16 = 128;
    const UC_KEY_ACTION_DISPLAY: u16 = 3;
    const UC_KEY_TRANSLATE_NO_DEAD_KEYS_MASK: u32 = 1;
    // Carbon's cmdKey is bit 8. UCKeyTranslate expects Carbon modifiers shifted
    // right by 8, so Command is represented by bit 0 here.
    const COMMAND_MODIFIER_STATE: u32 = 1;

    #[link(name = "Carbon", kind = "framework")]
    extern "C" {
        fn TISCopyCurrentKeyboardLayoutInputSource() -> TisInputSourceRef;
        fn TISGetInputSourceProperty(
            input_source: TisInputSourceRef,
            property_key: CfStringRef,
        ) -> CfDataRef;
        static kTISPropertyUnicodeKeyLayoutData: CfStringRef;
        fn UCKeyTranslate(
            key_layout: *const u8,
            virtual_key_code: u16,
            key_action: u16,
            modifier_key_state: u32,
            keyboard_type: u32,
            key_translate_options: u32,
            dead_key_state: *mut u32,
            max_string_length: usize,
            actual_string_length: *mut usize,
            unicode_string: *mut u16,
        ) -> i32;
        fn LMGetKbdType() -> u8;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFDataGetBytePtr(data: CfDataRef) -> *const u8;
        fn CFRelease(value: *const c_void);
    }

    struct InputSource(TisInputSourceRef);

    impl Drop for InputSource {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: TISCopyCurrentKeyboardLayoutInputSource returned this
                // retained reference, so this balances that ownership.
                unsafe { CFRelease(self.0) };
            }
        }
    }

    fn find_keycode(mut matches: impl FnMut(u16) -> bool) -> Option<u16> {
        (0..KEYCODE_COUNT).find(|&keycode| matches(keycode))
    }

    /// Resolves the physical key that macOS interprets as `v` while Command is
    /// held. Including Command is important: non-Latin layouts commonly map
    /// Cmd shortcuts to their ANSI equivalents, while standard Dvorak does not.
    ///
    /// TIS APIs must run on the main thread. Handy's paste path already enters
    /// through `AppHandle::run_on_main_thread` before reaching this function.
    fn resolve_command_v_keycode() -> Result<u16, String> {
        // SAFETY: This function is called on the macOS main thread. The returned
        // source follows the Create Rule and is released by InputSource::drop.
        // The property constant is provided by Carbon; layout_data is a CFData
        // owned by the retained input source and stays valid until source is
        // dropped after the scan. LMGetKbdType has no arguments and returns the
        // physical keyboard type used by UCKeyTranslate.
        let (source, layout, keyboard_type) = unsafe {
            let source = InputSource(TISCopyCurrentKeyboardLayoutInputSource());
            if source.0.is_null() {
                return Err("macOS returned no current keyboard layout input source".into());
            }
            let layout_data = TISGetInputSourceProperty(source.0, kTISPropertyUnicodeKeyLayoutData);
            if layout_data.is_null() {
                return Err("current macOS keyboard layout has no Unicode layout data".into());
            }
            let layout = CFDataGetBytePtr(layout_data);
            if layout.is_null() {
                return Err("current macOS keyboard layout data is empty".into());
            }
            (source, layout, LMGetKbdType() as u32)
        };
        let keycode = find_keycode(|keycode| {
            let mut dead_key_state = 0;
            let mut chars = [0_u16; 4];
            let mut length = 0_usize;

            // SAFETY: layout points to valid UCKeyboardLayout bytes while source
            // is retained. All output pointers reference initialized local
            // storage of the declared sizes.
            let status = unsafe {
                UCKeyTranslate(
                    layout,
                    keycode,
                    UC_KEY_ACTION_DISPLAY,
                    COMMAND_MODIFIER_STATE,
                    keyboard_type,
                    UC_KEY_TRANSLATE_NO_DEAD_KEYS_MASK,
                    &mut dead_key_state,
                    chars.len(),
                    &mut length,
                    chars.as_mut_ptr(),
                )
            };

            status == 0 && length == 1 && chars[0] == u16::from(b'v')
        })
        .ok_or_else(|| "could not map Cmd+V in the current macOS keyboard layout".to_string())?;

        Ok(keycode)
    }

    pub(super) fn command_v_key() -> Key {
        match resolve_command_v_keycode() {
            Ok(keycode) => {
                debug!("Resolved Cmd+V for the active macOS layout to keycode {keycode}");
                Key::Other(u32::from(keycode))
            }
            Err(error) => {
                warn!(
                    "Could not resolve Cmd+V for the active macOS layout ({error}); using ANSI V keycode {ANSI_V_KEYCODE}"
                );
                Key::Other(u32::from(ANSI_V_KEYCODE))
            }
        }
    }
}

/// Wrapper for Enigo to store in Tauri's managed state.
/// Enigo is wrapped in a Mutex since it requires mutable access.
pub struct EnigoState(pub Mutex<Enigo>);

impl EnigoState {
    pub fn new() -> Result<Self, String> {
        let enigo = Enigo::new(&Settings::default())
            .map_err(|e| format!("Failed to initialize Enigo: {}", e))?;
        Ok(Self(Mutex::new(enigo)))
    }
}

/// Get the current mouse cursor position using the managed Enigo instance.
/// Returns None if the state is not available or if getting the location fails.
pub fn get_cursor_position(app_handle: &AppHandle) -> Option<(i32, i32)> {
    let enigo_state = app_handle.try_state::<EnigoState>()?;
    let enigo = enigo_state.0.lock().ok()?;
    enigo.location().ok()
}

/// Sends a Ctrl+V or Cmd+V paste command using platform-specific virtual key codes.
/// This ensures the paste works regardless of keyboard layout (e.g., Russian, AZERTY, DVORAK).
/// Note: On Wayland, this may not work - callers should check for Wayland and use alternative methods.
///
/// `hold_ms` is how long the modifier stays held after the V click before being
/// released. Most applications read the modifier from the V event's flags and
/// need no hold at all, but applications that poll global keyboard state when
/// handling the key need the modifier to still be down — the hold insures
/// against those. Callers that can detect a failed chord (e.g. the
/// receipt-sequenced paste path) may use a much shorter hold.
pub fn send_paste_ctrl_v(enigo: &mut Enigo, hold_ms: u64) -> Result<(), String> {
    // FR-005-01: must precede the guard — the wait depends on seeing the
    // user's own key releases.
    await_shortcut_modifier_release();
    let _guard = InjectionGuard::begin();
    // Platform-specific key definitions
    #[cfg(target_os = "macos")]
    let (modifier_key, v_key_code) = (Key::Meta, macos::command_v_key());
    #[cfg(target_os = "windows")]
    let (modifier_key, v_key_code) = (Key::Control, Key::Other(0x56)); // VK_V
    #[cfg(target_os = "linux")]
    let (modifier_key, v_key_code) = (Key::Control, Key::Unicode('v'));

    // Press modifier + V
    enigo
        .key(modifier_key, enigo::Direction::Press)
        .map_err(|e| format!("Failed to press modifier key: {}", e))?;
    enigo
        .key(v_key_code, enigo::Direction::Click)
        .map_err(|e| format!("Failed to click V key: {}", e))?;

    std::thread::sleep(std::time::Duration::from_millis(hold_ms));

    enigo
        .key(modifier_key, enigo::Direction::Release)
        .map_err(|e| format!("Failed to release modifier key: {}", e))?;

    Ok(())
}

/// Sends a Ctrl+Shift+V paste command.
/// This is commonly used in terminal applications on Linux to paste without formatting.
/// Note: On Wayland, this may not work - callers should check for Wayland and use alternative methods.
pub fn send_paste_ctrl_shift_v(enigo: &mut Enigo, hold_ms: u64) -> Result<(), String> {
    await_shortcut_modifier_release();
    let _guard = InjectionGuard::begin();
    // Platform-specific key definitions
    #[cfg(target_os = "macos")]
    let (modifier_key, v_key_code) = (Key::Meta, macos::command_v_key());
    #[cfg(target_os = "windows")]
    let (modifier_key, v_key_code) = (Key::Control, Key::Other(0x56)); // VK_V
    #[cfg(target_os = "linux")]
    let (modifier_key, v_key_code) = (Key::Control, Key::Unicode('v'));

    // Press Ctrl/Cmd + Shift + V
    enigo
        .key(modifier_key, enigo::Direction::Press)
        .map_err(|e| format!("Failed to press modifier key: {}", e))?;
    enigo
        .key(Key::Shift, enigo::Direction::Press)
        .map_err(|e| format!("Failed to press Shift key: {}", e))?;
    enigo
        .key(v_key_code, enigo::Direction::Click)
        .map_err(|e| format!("Failed to click V key: {}", e))?;

    std::thread::sleep(std::time::Duration::from_millis(hold_ms));

    enigo
        .key(Key::Shift, enigo::Direction::Release)
        .map_err(|e| format!("Failed to release Shift key: {}", e))?;
    enigo
        .key(modifier_key, enigo::Direction::Release)
        .map_err(|e| format!("Failed to release modifier key: {}", e))?;

    Ok(())
}

/// Sends a Shift+Insert paste command (Windows and Linux only).
/// This is more universal for terminal applications and legacy software.
/// Note: On Wayland, this may not work - callers should check for Wayland and use alternative methods.
pub fn send_paste_shift_insert(enigo: &mut Enigo, hold_ms: u64) -> Result<(), String> {
    await_shortcut_modifier_release();
    let _guard = InjectionGuard::begin();
    #[cfg(target_os = "windows")]
    let insert_key_code = Key::Other(0x2D); // VK_INSERT
    #[cfg(not(target_os = "windows"))]
    let insert_key_code = Key::Other(0x76); // XK_Insert (keycode 118 / 0x76, also used as fallback)

    // Press Shift + Insert
    enigo
        .key(Key::Shift, enigo::Direction::Press)
        .map_err(|e| format!("Failed to press Shift key: {}", e))?;
    enigo
        .key(insert_key_code, enigo::Direction::Click)
        .map_err(|e| format!("Failed to click Insert key: {}", e))?;

    std::thread::sleep(std::time::Duration::from_millis(hold_ms));

    enigo
        .key(Key::Shift, enigo::Direction::Release)
        .map_err(|e| format!("Failed to release Shift key: {}", e))?;

    Ok(())
}

/// FR-002-04 "menu mask key": inject an inert press+release of the
/// unassigned virtual key `0xE8`. Windows opens the Start menu (or focuses
/// the menu bar, for Alt) when a Win/Alt press→release pair passes with no
/// intervening key; since modifier-only hotkeys must keep their edges
/// flowing to the OS, firing the mask while such a hold is active makes
/// the hold read as a chord instead of a tap.
///
/// The VK maps to nothing in handy-keys (`vk_to_modifier`/`map_key` both
/// return `None`), so the injected events never reach the matcher and this
/// needs no [`InjectionGuard`]. Called from the shortcut manager thread;
/// a failure is benign — worst case is the pre-mask behavior, the shell
/// menu opening on release.
#[cfg(target_os = "windows")]
pub fn send_menu_mask_key() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        VIRTUAL_KEY,
    };

    /// Unassigned VK; the same value AutoHotkey and handy-keys mask with.
    const MENU_MASK_VK: u16 = 0xE8;

    let input = |flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(MENU_MASK_VK),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: INJECTION_EXTRA_INFO,
            },
        },
    };
    let inputs = [input(KEYBD_EVENT_FLAGS(0)), input(KEYEVENTF_KEYUP)];
    // SAFETY: `inputs` is a valid stack array of INPUTs that outlives the call.
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        // Rare (e.g. UIPI filtering); the cost is the pre-mask behavior:
        // the shell may open its menu when the modifier is released.
        log::warn!("menu mask key injection failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::HashSet;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// FR-005-01: with nothing held the wait must be free — one poll, no
    /// sleep — so it never delays the common injection path.
    #[test]
    fn modifier_wait_returns_immediately_when_nothing_held() {
        let polls = Cell::new(0u32);
        let sleeps = Cell::new(0u32);
        let held: HashSet<u16> = HashSet::new();

        let still_held = poll_until_released(
            &[0xA2, 0x5B],
            ms(500),
            ms(10),
            |vk| {
                polls.set(polls.get() + 1);
                held.contains(&vk)
            },
            |_| sleeps.set(sleeps.get() + 1),
            Instant::now,
        );

        assert!(still_held.is_empty());
        assert_eq!(polls.get(), 2);
        assert_eq!(sleeps.get(), 0);
    }

    /// FR-005-01 / AC-005-08: a modifier the user is still holding is
    /// polled until its physical release, then the injection may proceed.
    #[test]
    fn modifier_wait_polls_until_physical_release() {
        const VK_LCONTROL_TEST: u16 = 0xA2;
        let held = RefCell::new(HashSet::from([VK_LCONTROL_TEST]));
        let start = Instant::now();
        let now = Cell::new(start);

        let still_held = poll_until_released(
            &[VK_LCONTROL_TEST],
            ms(500),
            ms(10),
            |vk| held.borrow().contains(&vk),
            |d| {
                now.set(now.get() + d);
                // The user lets go 120 ms in.
                if now.get().duration_since(start) >= ms(120) {
                    held.borrow_mut().clear();
                }
            },
            || now.get(),
        );

        assert!(still_held.is_empty());
        let waited = now.get().duration_since(start);
        assert!(waited >= ms(120) && waited < ms(200), "waited {waited:?}");
    }

    /// FR-005-01: a modifier never released within 500 ms is reported back
    /// so the caller can force a synthetic key-up before injecting.
    #[test]
    fn modifier_wait_times_out_and_reports_still_held_keys() {
        const VK_LWIN_TEST: u16 = 0x5B;
        const VK_RCONTROL_TEST: u16 = 0xA3;
        let held: HashSet<u16> = HashSet::from([VK_LWIN_TEST, VK_RCONTROL_TEST]);
        let start = Instant::now();
        let now = Cell::new(start);

        let still_held = poll_until_released(
            &[0xA0, VK_LWIN_TEST, VK_RCONTROL_TEST],
            ms(500),
            ms(10),
            |vk| held.contains(&vk),
            |d| now.set(now.get() + d),
            || now.get(),
        );

        assert_eq!(
            still_held,
            vec![VK_LWIN_TEST, VK_RCONTROL_TEST],
            "order must follow the input list so callers can report/inject deterministically"
        );
        assert!(now.get().duration_since(start) >= ms(500));
    }

    /// FR-005-01: a partial release reports only the modifiers the user
    /// still holds at the deadline.
    #[test]
    fn modifier_wait_reports_only_modifiers_held_at_timeout() {
        let held = RefCell::new(HashSet::from([0xA2u16, 0x5Bu16]));
        let start = Instant::now();
        let now = Cell::new(start);

        let still_held = poll_until_released(
            &[0xA2, 0x5B],
            ms(500),
            ms(10),
            |vk| held.borrow().contains(&vk),
            |d| {
                now.set(now.get() + d);
                // Ctrl released early; Win stays held forever.
                if now.get().duration_since(start) >= ms(50) {
                    held.borrow_mut().remove(&0xA2);
                }
            },
            || now.get(),
        );

        assert_eq!(still_held, vec![0x5B]);
    }

    /// AC-005-08 (Windows): the wait covers both sides of every modifier
    /// that can turn an injected `Ctrl+V` into a different chord — Ctrl,
    /// Shift, Alt and Win.
    #[cfg(target_os = "windows")]
    #[test]
    fn modifier_wait_covers_every_chord_modifier_vk() {
        assert_eq!(
            CHORD_MODIFIER_VKS,
            &[
                VK_LSHIFT.0,
                VK_RSHIFT.0,
                VK_LCONTROL.0,
                VK_RCONTROL.0,
                VK_LMENU.0,
                VK_RMENU.0,
                VK_LWIN.0,
                VK_RWIN.0,
            ]
        );
    }
}
