//! Transactional registrations. Native callbacks only enqueue bounded events.
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError, mpsc};

use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
    hotkey::{Code, HotKey, Modifiers},
};
use gpui::Keystroke;
use huterm_protocol::{CommandArgument, CommandInvocation, CommandValue, ids};

use crate::{config::KeybindingEntry, keymap::CompiledKeymap};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Binding {
    pub key: HotKey,
    /// The keystroke as configured, for messages.
    pub keystroke: String,
    pub invocation: CommandInvocation,
}

pub(crate) fn compile(
    entries: &[KeybindingEntry],
    profiles: &super::Config,
    local: &CompiledKeymap,
) -> Result<Vec<Binding>, String> {
    let mut result = Vec::new();
    let mut keys = HashSet::new();
    for (index, entry) in entries.iter().enumerate() {
        let parsed = (|| {
            if entry.when.is_some() {
                return Err(
                    "global shortcuts cannot have a when predicate".into()
                );
            }
            if entry.disabled.is_some() {
                return Err("global shortcuts cannot be disabled; remove the \
                     entry instead"
                    .into());
            }
            let id = match entry.command.as_str() {
                "show_quake" => ids::SHOW_QUAKE,
                "hide_quake" => ids::HIDE_QUAKE,
                "toggle_quake" => ids::TOGGLE_QUAKE,
                _ => {
                    return Err("global shortcuts support show_quake, hide_quake and toggle_quake only".into());
                }
            };
            let args = entry.args.clone().unwrap_or_default();
            if args.keys().any(|name| name != "profile") {
                return Err("only the profile argument is supported".into());
            }
            let profile = match args.get("profile") {
                Some(toml::Value::String(value)) => value.as_str(),
                None => "default",
                _ => return Err("profile must be text".into()),
            };
            if !profiles.profiles.contains_key(profile) {
                return Err(format!("unknown quake profile {profile:?}"));
            }
            let stroke = parse_stroke(&entry.key)?;
            for binding in &local.bindings {
                if binding.keystrokes().first().is_some_and(|other| {
                    *other.modifiers() == stroke.modifiers
                        && other.key() == stroke.key
                }) {
                    return Err(
                        "conflicts with the first stroke of an in-app binding"
                            .into(),
                    );
                }
            }
            let key = native_key(&stroke)?;
            if !keys.insert(key.id()) {
                return Err("duplicate global shortcut".into());
            }
            Ok(Binding {
                key,
                keystroke: entry.key.clone(),
                invocation: CommandInvocation::new(
                    id,
                    vec![CommandArgument::new(
                        "profile",
                        CommandValue::Text(profile.into()),
                    )],
                ),
            })
        })();
        result.push(parsed.map_err(|error: String| {
            format!(
                "global_keybinding {} ({:?}): {error}",
                index + 1,
                entry.key
            )
        })?);
    }
    Ok(result)
}

fn parse_stroke(key: &str) -> Result<Keystroke, String> {
    if key.split_whitespace().count() != 1 || key.contains('+') {
        return Err("expected one stroke with modifiers joined by '-'".into());
    }
    Keystroke::parse(key).map_err(|error| error.to_string())
}
fn native_key(stroke: &Keystroke) -> Result<HotKey, String> {
    let maximum_function_key = if cfg!(target_os = "macos") { 20 } else { 24 };
    let code = match stroke.key.as_str() {
        "space" => Code::Space,
        "enter" => Code::Enter,
        "escape" => Code::Escape,
        "tab" => Code::Tab,
        "backspace" => Code::Backspace,
        "up" => Code::ArrowUp,
        "down" => Code::ArrowDown,
        "left" => Code::ArrowLeft,
        "right" => Code::ArrowRight,
        key if key.len() == 1 && key.as_bytes()[0].is_ascii_lowercase() => {
            format!("Key{}", key.to_ascii_uppercase())
                .parse()
                .map_err(|_| "unsupported letter")?
        }
        key if key.len() == 1 && key.as_bytes()[0].is_ascii_digit() => {
            format!("Digit{key}")
                .parse()
                .map_err(|_| "unsupported digit")?
        }
        key if key
            .strip_prefix('f')
            .and_then(|number| number.parse::<u8>().ok())
            .is_some_and(|number| {
                (1..=maximum_function_key).contains(&number)
            }) =>
        {
            key.to_ascii_uppercase()
                .parse()
                .map_err(|_| "unsupported function key")?
        }
        _ => {
            return Err("unsupported global key; use letters, digits, F1-F20 (F24 on X11), space, enter, escape, tab, backspace or arrows".into());
        }
    };
    if stroke.modifiers.function {
        return Err("the fn modifier is not supported globally".into());
    }
    let mut modifiers = Modifiers::empty();
    if stroke.modifiers.control {
        modifiers |= Modifiers::CONTROL;
    }
    if stroke.modifiers.alt {
        modifiers |= Modifiers::ALT;
    }
    if stroke.modifiers.shift {
        modifiers |= Modifiers::SHIFT;
    }
    if stroke.modifiers.platform {
        modifiers |= Modifiers::META;
    }
    Ok(HotKey::new(Some(modifiers), code))
}

#[derive(Clone, Copy)]
struct CallbackSlot {
    generation: u64,
    pressed: bool,
}
#[derive(Default)]
struct CallbackState(HashMap<u32, CallbackSlot>);
impl CallbackState {
    fn event(&mut self, id: u32, state: HotKeyState) -> Option<(u64, u32)> {
        let slot = self.0.get_mut(&id)?;
        if state == HotKeyState::Released {
            slot.pressed = false;
            return None;
        }
        if slot.pressed {
            return None;
        }
        slot.pressed = true;
        Some((slot.generation, id))
    }
    fn replace(&mut self, generation: u64, keys: impl Iterator<Item = u32>) {
        self.0 = keys
            .map(|id| {
                (
                    id,
                    CallbackSlot {
                        generation,
                        pressed: self
                            .0
                            .get(&id)
                            .is_some_and(|slot| slot.pressed),
                    },
                )
            })
            .collect();
    }
}
trait RegistrationBackend {
    fn register(&self, key: HotKey) -> Result<(), String>;
    fn unregister(&self, key: HotKey) -> Result<(), String>;
}
impl RegistrationBackend for GlobalHotKeyManager {
    fn register(&self, key: HotKey) -> Result<(), String> {
        // Registrations never repeat an owned key, so the platform reports
        // this only for a grab another application holds.
        self.register(key).map_err(|error| match error {
            global_hotkey::Error::AlreadyRegistered(_) => {
                "already in use by another application".into()
            }
            error => error.to_string(),
        })
    }
    fn unregister(&self, key: HotKey) -> Result<(), String> {
        self.unregister(key).map_err(|error| error.to_string())
    }
}
#[derive(Default)]
struct OwnedKeys(HashMap<u32, HotKey>);
impl OwnedKeys {
    fn has_bindings(&self, bindings: &[Binding]) -> bool {
        bindings
            .iter()
            .any(|binding| self.0.contains_key(&binding.key.id()))
    }

    /// Grabs each binding's key and releases every other owned key, one key
    /// at a time, and returns one message per key the backend refused. A
    /// refusal skips only that key, and the next reconcile tries it again.
    /// `self` holds exactly the backend's grabs, including one whose release
    /// failed, so `Drop` can still release it.
    fn reconcile(
        &mut self,
        backend: &impl RegistrationBackend,
        bindings: &[Binding],
    ) -> Vec<String> {
        let mut refused = Vec::new();
        // An unchanged grab is kept, never released and grabbed again.
        for binding in bindings {
            let key = binding.key;
            if self.0.contains_key(&key.id()) {
                continue;
            }
            match backend.register(key) {
                Ok(()) => {
                    self.0.insert(key.id(), key);
                }
                Err(error) => refused.push(format!(
                    "cannot register {}: {error}",
                    binding.keystroke
                )),
            }
        }
        let wanted: HashSet<u32> =
            bindings.iter().map(|binding| binding.key.id()).collect();
        let removed: Vec<HotKey> = self
            .0
            .values()
            .filter(|key| !wanted.contains(&key.id()))
            .copied()
            .collect();
        for key in removed {
            match backend.unregister(key) {
                Ok(()) => {
                    self.0.remove(&key.id());
                }
                Err(error) => {
                    refused.push(format!("cannot release {key}: {error}"));
                }
            }
        }
        refused
    }
}

pub(crate) struct Registrations {
    manager: GlobalHotKeyManager,
    owned: OwnedKeys,
    bindings: Vec<Binding>,
    generation: u64,
    live: Arc<Mutex<CallbackState>>,
    events: mpsc::Receiver<(u64, u32)>,
    pub wakeups: async_channel::Receiver<()>,
}
impl Registrations {
    pub fn new() -> Result<Self, String> {
        #[cfg(target_os = "linux")]
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            return Err("global shortcuts require a native X11 session; Wayland and XWayland are unsupported".into());
        }
        let manager =
            GlobalHotKeyManager::new().map_err(|error| error.to_string())?;
        let (sender, events) = mpsc::sync_channel(64);
        let (wake, wakeups) = async_channel::bounded(1);
        let live = Arc::new(Mutex::new(CallbackState::default()));
        let callback_live = Arc::clone(&live);
        GlobalHotKeyEvent::set_event_handler(Some(
            move |event: GlobalHotKeyEvent| {
                // Release state is updated before admission to the bounded queue.
                // A full queue can drop a press, but never strand a held shortcut.
                if let Ok(mut live) = callback_live.lock()
                    && let Some(event) = live.event(event.id, event.state)
                    && sender.try_send(event).is_ok()
                {
                    let _ = wake.try_send(());
                }
            },
        ));
        Ok(Self {
            manager,
            owned: OwnedKeys::default(),
            bindings: Vec::new(),
            generation: 0,
            live,
            events,
            wakeups,
        })
    }
    /// Registers `next` as far as the platform allows and returns one
    /// message per key it refused. Only registered keys dispatch.
    pub fn replace(&mut self, next: Vec<Binding>) -> Vec<String> {
        let refused = self.owned.reconcile(&self.manager, &next);
        self.bindings = next;
        self.generation += 1;
        // The callback cannot panic while holding the lock, and the state
        // is replaced whole, so a poisoned lock still holds usable state.
        self.live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(
                self.generation,
                self.bindings
                    .iter()
                    .map(|binding| binding.key.id())
                    .filter(|id| self.owned.0.contains_key(id)),
            );
        refused
    }
    pub fn has_bindings(&self) -> bool {
        self.owned.has_bindings(&self.bindings)
    }
    pub fn drain(&self) -> Vec<CommandInvocation> {
        self.events
            .try_iter()
            .take(64)
            .filter(|(generation, _)| *generation == self.generation)
            .filter_map(|(_, id)| {
                self.bindings
                    .iter()
                    .find(|binding| binding.key.id() == id)
                    .map(|binding| binding.invocation.clone())
            })
            .collect()
    }
}
impl Drop for Registrations {
    fn drop(&mut self) {
        self.wakeups.close();
        if let Ok(mut live) = self.live.lock() {
            live.0.clear();
        }
        for key in self.owned.0.values() {
            if let Err(error) = self.manager.unregister(*key) {
                eprintln!("Global shortcut cleanup {key}: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    #[test]
    fn release_survives_full_queue_and_reload_does_not_repeat_a_held_key() {
        let mut state = CallbackState::default();
        state.replace(1, [7].into_iter());
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .try_send(state.event(7, HotKeyState::Pressed).unwrap())
            .unwrap();
        assert!(state.event(7, HotKeyState::Released).is_none());
        assert!(
            sender
                .try_send(state.event(7, HotKeyState::Pressed).unwrap())
                .is_err()
        );
        assert!(state.event(7, HotKeyState::Released).is_none());
        receiver.try_recv().unwrap();
        let event = state.event(7, HotKeyState::Pressed).unwrap();
        assert_eq!(event, (1, 7));
        state.replace(2, [7].into_iter());
        assert!(state.event(7, HotKeyState::Pressed).is_none());
        state.event(7, HotKeyState::Released);
        assert_eq!(state.event(7, HotKeyState::Pressed), Some((2, 7)));
    }
    #[derive(Default)]
    struct Fake {
        keys: RefCell<HashMap<u32, HotKey>>,
        fail_register: RefCell<HashSet<u32>>,
        fail_unregister: RefCell<HashSet<u32>>,
    }
    impl RegistrationBackend for Fake {
        fn register(&self, key: HotKey) -> Result<(), String> {
            if self.fail_register.borrow().contains(&key.id()) {
                return Err("registration rejected".into());
            }
            self.keys.borrow_mut().insert(key.id(), key);
            Ok(())
        }
        fn unregister(&self, key: HotKey) -> Result<(), String> {
            if self.fail_unregister.borrow().contains(&key.id()) {
                return Err("unregistration rejected".into());
            }
            self.keys.borrow_mut().remove(&key.id());
            Ok(())
        }
    }
    fn key(code: Code) -> HotKey {
        HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), code)
    }
    fn binding(keystroke: &str, code: Code) -> Binding {
        Binding {
            key: key(code),
            keystroke: keystroke.into(),
            invocation: CommandInvocation::new(ids::TOGGLE_QUAKE, Vec::new()),
        }
    }
    const NONE: [String; 0] = [];
    fn owned_keys(bindings: &[&Binding]) -> HashMap<u32, HotKey> {
        bindings
            .iter()
            .map(|binding| (binding.key.id(), binding.key))
            .collect()
    }
    #[test]
    fn refused_grab_is_reported_while_the_rest_register_and_retried_later() {
        let first = binding("ctrl-alt-a", Code::KeyA);
        let taken = binding("ctrl-alt-b", Code::KeyB);
        let last = binding("ctrl-alt-c", Code::KeyC);
        let all = [first.clone(), taken.clone(), last.clone()];
        let backend = Fake::default();
        backend.fail_register.borrow_mut().insert(taken.key.id());
        let mut owned = OwnedKeys::default();
        assert_eq!(
            owned.reconcile(&backend, &all),
            ["cannot register ctrl-alt-b: registration rejected"]
        );
        assert_eq!(owned.0, owned_keys(&[&first, &last]));
        assert_eq!(owned.0, *backend.keys.borrow());
        assert!(!owned.has_bindings(std::slice::from_ref(&taken)));
        backend.fail_register.borrow_mut().clear();
        assert_eq!(owned.reconcile(&backend, &all), NONE);
        assert_eq!(owned.0, owned_keys(&[&first, &taken, &last]));
        assert_eq!(owned.0, *backend.keys.borrow());
    }
    #[test]
    fn refused_replacement_still_releases_the_old_shortcut() {
        let old = binding("ctrl-alt-a", Code::KeyA);
        let new = binding("ctrl-alt-b", Code::KeyB);
        let backend = Fake::default();
        let mut owned = OwnedKeys::default();
        assert_eq!(owned.reconcile(&backend, std::slice::from_ref(&old)), NONE);
        backend.fail_register.borrow_mut().insert(new.key.id());
        assert_eq!(
            owned.reconcile(&backend, std::slice::from_ref(&new)),
            ["cannot register ctrl-alt-b: registration rejected"]
        );
        assert_eq!(owned.0, HashMap::new());
        assert_eq!(*backend.keys.borrow(), HashMap::new());
    }
    #[test]
    fn failed_release_stays_owned_for_cleanup_without_keeping_alive() {
        let stray = binding("ctrl-alt-a", Code::KeyA);
        let backend = Fake::default();
        let mut owned = OwnedKeys::default();
        assert_eq!(
            owned.reconcile(&backend, std::slice::from_ref(&stray)),
            NONE
        );
        backend.fail_unregister.borrow_mut().insert(stray.key.id());
        let refused = owned.reconcile(&backend, &[]);
        assert_eq!(refused.len(), 1);
        assert!(
            refused[0].starts_with("cannot release ")
                && refused[0].ends_with(": unregistration rejected"),
            "{refused:?}"
        );
        assert_eq!(owned.0, owned_keys(&[&stray]));
        assert_eq!(owned.0, *backend.keys.borrow());
        assert!(
            !owned.has_bindings(&[]),
            "stray grabs cannot dispatch commands"
        );
        backend.fail_unregister.borrow_mut().clear();
        assert_eq!(owned.reconcile(&backend, &[]), NONE);
        assert_eq!(owned.0, HashMap::new());
        assert_eq!(*backend.keys.borrow(), HashMap::new());
    }
    #[test]
    fn compile_validates_profile_arguments_and_global_local_conflicts() {
        let profiles = super::super::Config::default().validated().unwrap();
        let local =
            crate::keymap::compile_defaults(crate::keymap::Platform::current());
        let entry = KeybindingEntry {
            key: "ctrl-alt-t".into(),
            command: "toggle_quake".into(),
            args: None,
            when: None,
            description: None,
            disabled: None,
        };
        let compiled =
            compile(std::slice::from_ref(&entry), &profiles, &local).unwrap();
        assert!(
            compile(
                &[KeybindingEntry {
                    key: "f12".into(),
                    ..entry.clone()
                }],
                &profiles,
                &local
            )
            .is_ok()
        );
        assert_eq!(compiled[0].invocation.text("profile"), Some("default"));
        for invalid in [
            KeybindingEntry {
                key: "ctrl-k ctrl-t".into(),
                ..entry.clone()
            },
            KeybindingEntry {
                when: Some("Terminal".into()),
                ..entry.clone()
            },
            KeybindingEntry {
                command: "quit".into(),
                ..entry.clone()
            },
            KeybindingEntry {
                args: Some(toml::Table::from_iter([(
                    "profile".into(),
                    toml::Value::String("missing".into()),
                )])),
                ..entry.clone()
            },
        ] {
            assert!(compile(&[invalid], &profiles, &local).is_err());
        }
        // Global shortcuts cannot be disabled, whatever the value.
        for value in [true, false] {
            let error = compile(
                &[KeybindingEntry {
                    disabled: Some(value),
                    ..entry.clone()
                }],
                &profiles,
                &local,
            )
            .unwrap_err();
            assert!(error.contains("cannot be disabled"), "{error}");
        }
        let mut local = local;
        local.bindings.push(gpui::KeyBinding::new(
            "ctrl-alt-t x",
            crate::commands::InvokeApp(CommandInvocation::new(
                ids::SHOW_QUAKE,
                Vec::new(),
            )),
            None,
        ));
        assert!(
            compile(&[entry], &profiles, &local)
                .unwrap_err()
                .contains("conflicts")
        );
    }
}
