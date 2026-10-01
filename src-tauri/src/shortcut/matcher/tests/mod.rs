//! Split by FR: `matching` covers press/release/register basics,
//! `promotion` FR-002-05 prefix promotion, `arming` FR-002-06 the arming
//! window interrupt, `double_tap` FR-002-07 the hands-free gesture.

use super::*;
use handy_keys::{Key, Modifiers};

mod arming;
mod double_tap;
mod matching;
mod promotion;

fn now() -> Instant {
    Instant::now()
}

fn key_event(modifiers: Modifiers, key: Option<Key>, is_key_down: bool) -> KeyEvent {
    KeyEvent {
        modifiers,
        key,
        is_key_down,
        changed_modifier: None,
    }
}

fn modifier_event(modifiers: Modifiers, is_key_down: bool, changed: Modifiers) -> KeyEvent {
    KeyEvent {
        modifiers,
        key: None,
        is_key_down,
        changed_modifier: Some(changed),
    }
}

fn register(matcher: &mut HotkeyMatcher, id: &str, raw: &str) {
    let hotkey: Hotkey = raw.parse().expect("test hotkey must parse");
    matcher
        .register(id, hotkey, raw.to_string())
        .expect("test registration must succeed");
}
