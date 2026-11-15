//! Shortcuts for palette commands, including the ones saved in settings.
//!
//! GPUI only adds key bindings, so a changed shortcut is a new binding and
//! an [`Unbind`] of the one it replaces. Later bindings win.

use std::{collections::HashMap, rc::Rc};

use gpui_kit::component::{Icon, WindowExt as _};
use gpui_kit::{
    Action, App, Global, KeyBinding, KeyBindingContextPredicate, Keystroke, SharedString,
    Subscription, Unbind,
};

use crate::{
    AppSettings, Command, PluginHost,
    actions::{RunCommand, WORKSPACE_CONTEXT},
};

/// One command that can be listed in the palette and bound to a shortcut.
pub struct PaletteCommand {
    pub id: SharedString,
    pub name: SharedString,
    pub group: SharedString,
    pub default_hotkey: Option<SharedString>,
    pub icon: Option<Icon>,
    action_name: SharedString,
    bind: Rc<dyn Fn() -> Box<dyn Action>>,
}

impl Clone for PaletteCommand {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            name: self.name.clone(),
            group: self.group.clone(),
            default_hotkey: self.default_hotkey.clone(),
            icon: self.icon.clone(),
            action_name: self.action_name.clone(),
            bind: self.bind.clone(),
        }
    }
}

impl PaletteCommand {
    /// A built-in command. `A`'s action name is the id settings store.
    pub fn core<A>(
        name: impl Into<SharedString>,
        default_hotkey: Option<&str>,
        icon: Option<Icon>,
    ) -> Self
    where
        A: Action + Default + 'static,
    {
        let id = A::name_for_type();
        Self {
            id: id.into(),
            name: name.into(),
            group: "Notia".into(),
            default_hotkey: default_hotkey.map(Into::into),
            icon,
            action_name: id.into(),
            bind: Rc::new(|| Box::new(A::default())),
        }
    }

    fn plugin(group: SharedString, command: &Command) -> Self {
        let id = command.id().clone();
        let action_name: SharedString = RunCommand { id: id.clone() }.name().into();
        let command_id = id.clone();
        Self {
            id,
            name: command.name().clone(),
            group,
            default_hotkey: command.keybinding().cloned(),
            icon: command.icon().cloned(),
            action_name,
            bind: Rc::new(move || {
                Box::new(RunCommand {
                    id: command_id.clone(),
                })
            }),
        }
    }

    pub fn action(&self) -> Box<dyn Action> {
        (self.bind)()
    }
}

/// Built-in commands. Plugin commands are read from the host when asked.
struct PaletteCore {
    commands: Vec<PaletteCommand>,
}

impl Global for PaletteCore {}

/// The shortcut currently installed for each command this module has changed.
struct EnforcedHotkeys {
    keys: HashMap<String, Option<String>>,
}

impl Global for EnforcedHotkeys {}

struct HotkeyCapture {
    command_id: SharedString,
}

impl Global for HotkeyCapture {}

struct CaptureInstalled {
    _subscription: Subscription,
}

impl Global for CaptureInstalled {}

/// The built-in commands, then every command of the enabled plugins.
pub fn commands(cx: &App) -> Vec<PaletteCommand> {
    let mut commands = cx
        .try_global::<PaletteCore>()
        .map(|core| core.commands.clone())
        .unwrap_or_default();
    if cx.has_global::<PluginHost>() {
        for (group, command) in PluginHost::global(cx).commands() {
            commands.push(PaletteCommand::plugin(group, &command));
        }
    }
    commands
}

/// Remembers the built-in commands. Call before plugins bind their keys.
pub fn set_core(commands: Vec<PaletteCommand>, cx: &mut App) {
    cx.set_global(PaletteCore { commands });
}

/// Binds each built-in command's default shortcut.
pub fn bind_core_defaults(cx: &mut App) {
    let Some(core) = cx.try_global::<PaletteCore>() else {
        return;
    };
    let commands = core.commands.clone();
    let mut bindings = Vec::new();
    for command in commands {
        let Some(keys) = command.default_hotkey.clone() else {
            continue;
        };
        if let Some(binding) = load_binding(&keys, command.action(), cx) {
            bindings.push(binding);
        }
    }
    cx.bind_keys(bindings);
}

/// Listens for the next shortcut while one is being recorded.
pub fn install_capture(cx: &mut App) {
    if cx.has_global::<CaptureInstalled>() {
        return;
    }
    let subscription = cx.intercept_keystrokes(|event, window, cx| {
        let Some(capture) = cx.try_global::<HotkeyCapture>() else {
            return;
        };
        let command_id = capture.command_id.clone();
        let keystroke = event.keystroke.clone();
        if keystroke.is_modifier_key() {
            return;
        }
        cx.stop_propagation();
        cx.remove_global::<HotkeyCapture>();
        match keystroke.key.as_str() {
            "escape" => {}
            "backspace" | "delete" => {
                clear_hotkey(&command_id, cx);
            }
            _ => {
                let stolen = set_hotkey(&command_id, &binding_source(&keystroke, cx), cx);
                if !stolen.is_empty() {
                    window.push_notification(
                        format!("Removed that shortcut from {}.", stolen.join(", ")),
                        cx,
                    );
                }
            }
        }
        window.refresh();
    });
    cx.set_global(CaptureInstalled {
        _subscription: subscription,
    });
}

pub fn is_enabled(cx: &App) -> bool {
    AppSettings::global(cx).palette.enabled
}

pub fn set_enabled(enabled: bool, cx: &mut App) {
    AppSettings::update(cx, |settings| settings.palette.enabled = enabled);
}

pub fn is_listed(id: &str, cx: &App) -> bool {
    !AppSettings::global(cx).palette.hidden.contains(id)
}

pub fn set_listed(id: &str, listed: bool, cx: &mut App) {
    AppSettings::update(cx, |settings| {
        if listed {
            settings.palette.hidden.remove(id);
        } else {
            settings.palette.hidden.insert(id.to_string());
        }
    });
}

pub fn is_customized(id: &str, cx: &App) -> bool {
    let palette = &AppSettings::global(cx).palette;
    palette.hidden.contains(id) || palette.hotkeys.contains_key(id)
}

pub fn hotkey_is_custom(id: &str, cx: &App) -> bool {
    AppSettings::global(cx).palette.hotkeys.contains_key(id)
}

/// The shortcut in effect, or none when it was cleared.
pub fn hotkey(id: &str, cx: &App) -> Option<String> {
    let mac = cx.platform_info().is_mac();
    let default = commands(cx)
        .into_iter()
        .find(|command| command.id.as_ref() == id)
        .and_then(|command| command.default_hotkey.map(|keys| keys.to_string()));
    let stored = AppSettings::global(cx)
        .palette
        .hotkeys
        .get(id)
        .map(String::as_str);
    desired_key(default.as_deref(), stored, mac).0
}

pub fn is_capturing(id: &str, cx: &App) -> bool {
    cx.try_global::<HotkeyCapture>()
        .is_some_and(|capture| capture.command_id.as_ref() == id)
}

pub fn start_capture(id: SharedString, cx: &mut App) {
    cx.set_global(HotkeyCapture { command_id: id });
}

pub fn cancel_capture(cx: &mut App) {
    cx.remove_global::<HotkeyCapture>();
}

/// Saves `keys` as the command's shortcut. Returns the names of commands that
/// had to give the shortcut up.
pub fn set_hotkey(id: &str, keys: &str, cx: &mut App) -> Vec<SharedString> {
    let mac = cx.platform_info().is_mac();
    let canonical = canonicalize(keys, mac);
    let default = default_hotkey(id, cx);
    AppSettings::update(cx, |settings| {
        if default
            .as_deref()
            .is_some_and(|default| canonicalize(default, mac) == canonical)
        {
            settings.palette.hotkeys.remove(id);
        } else {
            settings.palette.hotkeys.insert(id.to_string(), canonical);
        }
    });
    sync(cx)
}

pub fn clear_hotkey(id: &str, cx: &mut App) -> Vec<SharedString> {
    let default = default_hotkey(id, cx);
    AppSettings::update(cx, |settings| {
        if default.is_none() {
            settings.palette.hotkeys.remove(id);
        } else {
            settings
                .palette
                .hotkeys
                .insert(id.to_string(), String::new());
        }
    });
    sync(cx)
}

/// Forgets a saved shortcut so the default is used again.
pub fn restore_hotkey(id: &str, cx: &mut App) -> Vec<SharedString> {
    AppSettings::update(cx, |settings| {
        settings.palette.hotkeys.remove(id);
    });
    sync(cx)
}

/// Restores the command's place in the palette and its default shortcut.
pub fn reset_command(id: &str, cx: &mut App) {
    AppSettings::update(cx, |settings| {
        settings.palette.hidden.remove(id);
        settings.palette.hotkeys.remove(id);
    });
    sync(cx);
}

/// Applies saved shortcuts on top of the defaults currently bound.
pub fn sync(cx: &mut App) -> Vec<SharedString> {
    let mac = cx.platform_info().is_mac();
    let entries = commands(cx);
    let mut palette = AppSettings::global(cx).palette.clone();
    let mut choices: Vec<Choice> = entries
        .iter()
        .map(|command| {
            let stored = palette.hotkeys.get(command.id.as_ref()).cloned();
            let (desired, redundant) =
                desired_key(command.default_hotkey.as_deref(), stored.as_deref(), mac);
            if redundant {
                palette.hotkeys.remove(command.id.as_ref());
            }
            Choice {
                id: command.id.to_string(),
                desired,
                explicit: palette.hotkeys.contains_key(command.id.as_ref()),
            }
        })
        .collect();
    let stolen = resolve_conflicts(&mut choices);
    for id in &stolen {
        palette.hotkeys.insert(id.clone(), String::new());
    }
    if palette != AppSettings::global(cx).palette {
        AppSettings::update(cx, |settings| settings.palette = palette);
    }

    let mut enforced = cx
        .try_global::<EnforcedHotkeys>()
        .map(|state| state.keys.clone())
        .unwrap_or_default();
    let mut planned = Vec::new();
    for command in &entries {
        let want = choices
            .iter()
            .find(|choice| choice.id == command.id.as_ref())
            .and_then(|choice| choice.desired.clone());
        let current = enforced
            .get(command.id.as_ref())
            .cloned()
            .unwrap_or_else(|| command.default_hotkey.as_ref().map(ToString::to_string));
        planned.push((command, current, want));
    }
    // Unbind first. Two commands can share an action name, and unbinding the
    // loser after the winner was bound would remove the winner as well.
    for (command, current, want) in &planned {
        if keys_equal(current.as_ref(), want.as_ref(), mac) {
            continue;
        }
        if let Some(current) = current.as_ref().filter(|keys| !keys.is_empty()) {
            unbind(&command.action_name, current, cx);
        }
    }
    for (command, current, want) in &planned {
        if keys_equal(current.as_ref(), want.as_ref(), mac) {
            continue;
        }
        if let Some(want) = want.as_ref()
            && let Some(binding) = load_binding(want, command.action(), cx)
        {
            cx.bind_keys([binding]);
        }
        enforced.insert(command.id.to_string(), want.clone());
    }
    cx.set_global(EnforcedHotkeys { keys: enforced });

    stolen
        .into_iter()
        .filter_map(|id| {
            entries
                .iter()
                .find(|command| command.id.as_ref() == id)
                .map(|command| command.name.clone())
        })
        .collect()
}

/// A label such as `⌘P` for a stored shortcut string.
pub fn display_hotkey(source: &str, cx: &App) -> String {
    let os = cx.platform_info().os;
    if source.is_empty() {
        return "None".to_string();
    }
    source
        .split_whitespace()
        .map(|part| {
            Keystroke::parse_with_os(part, os)
                .map(|keystroke| keystroke.display(os).to_string())
                .unwrap_or_else(|_| part.to_string())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn default_hotkey(id: &str, cx: &App) -> Option<String> {
    commands(cx)
        .into_iter()
        .find(|command| command.id.as_ref() == id)
        .and_then(|command| command.default_hotkey.map(|keys| keys.to_string()))
}

fn binding_source(keystroke: &Keystroke, cx: &App) -> String {
    let mac = cx.platform_info().is_mac();
    let mut parts = Vec::new();
    if keystroke.modifiers.function {
        parts.push("fn");
    }
    if mac && keystroke.modifiers.platform {
        parts.push("secondary");
    }
    if keystroke.modifiers.control {
        parts.push(if mac { "ctrl" } else { "secondary" });
    } else if !mac && keystroke.modifiers.platform {
        parts.push("win");
    }
    if keystroke.modifiers.alt {
        parts.push("alt");
    }
    if keystroke.modifiers.shift {
        parts.push("shift");
    }
    parts.push(keystroke.key.as_str());
    parts.join("-")
}

struct Choice {
    id: String,
    desired: Option<String>,
    /// A saved shortcut, including one that was cleared.
    explicit: bool,
}

/// When several commands want one shortcut, an explicit choice beats a
/// default, and otherwise the later command keeps it. Losers are cleared.
fn resolve_conflicts(choices: &mut [Choice]) -> Vec<String> {
    let mut owners: HashMap<String, usize> = HashMap::new();
    let mut stolen = Vec::new();
    for index in 0..choices.len() {
        let Some(key) = choices[index].desired.clone() else {
            continue;
        };
        let Some(previous) = owners.insert(key.clone(), index) else {
            continue;
        };
        let winner = if choices[index].explicit && !choices[previous].explicit {
            index
        } else if choices[previous].explicit && !choices[index].explicit {
            previous
        } else {
            index
        };
        let loser = if winner == index { previous } else { index };
        owners.insert(key, winner);
        if choices[loser].desired.take().is_some() {
            stolen.push(choices[loser].id.clone());
        }
    }
    stolen
}

/// The shortcut to install, and whether a saved value only repeats the default.
fn desired_key(default: Option<&str>, stored: Option<&str>, mac: bool) -> (Option<String>, bool) {
    match stored {
        Some("") => (None, false),
        Some(value) => {
            let key = canonicalize(value, mac);
            let redundant = default.is_some_and(|default| canonicalize(default, mac) == key);
            (Some(key), redundant)
        }
        None => (default.map(|default| canonicalize(default, mac)), false),
    }
}

fn keys_equal(current: Option<&String>, want: Option<&String>, mac: bool) -> bool {
    match (current, want) {
        (None, None) => true,
        (Some(current), Some(want)) => canonicalize(current, mac) == canonicalize(want, mac),
        _ => false,
    }
}

/// Spells `cmd` on macOS and `ctrl` elsewhere as `secondary`, in a stable order.
pub(crate) fn canonicalize(source: &str, mac: bool) -> String {
    source
        .split_whitespace()
        .map(|stroke| canonicalize_stroke(stroke, mac))
        .filter(|stroke| !stroke.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn canonicalize_stroke(stroke: &str, mac: bool) -> String {
    let mut parts: Vec<&str> = stroke.split('-').filter(|part| !part.is_empty()).collect();
    let Some(key) = parts.pop() else {
        return String::new();
    };
    let mut modifiers = Vec::new();
    for part in parts {
        let part = match part {
            "cmd" | "super" if mac => "secondary",
            "ctrl" if !mac => "secondary",
            "super" | "win" if !mac => "win",
            other => other,
        };
        if !modifiers.contains(&part) {
            modifiers.push(part);
        }
    }
    const ORDER: [&str; 5] = ["secondary", "ctrl", "alt", "shift", "fn"];
    modifiers.sort_by_key(|modifier| ORDER.iter().position(|item| item == modifier).unwrap_or(9));
    modifiers.push(key);
    modifiers.join("-")
}

fn load_binding(keys: &str, action: Box<dyn Action>, cx: &App) -> Option<KeyBinding> {
    let context = KeyBindingContextPredicate::parse(WORKSPACE_CONTEXT)
        .ok()
        .map(Rc::new);
    match KeyBinding::load(keys, action, context, false, None, cx) {
        Ok(binding) => Some(binding),
        Err(error) => {
            log::warn!("ignoring the shortcut “{keys}”: {error}");
            None
        }
    }
}

fn unbind(action_name: &str, keys: &str, cx: &mut App) {
    if let Some(binding) = load_binding(keys, Box::new(Unbind(action_name.into())), cx) {
        cx.bind_keys([binding]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(id: &str, desired: Option<&str>, explicit: bool) -> Choice {
        Choice {
            id: id.into(),
            desired: desired.map(str::to_string),
            explicit,
        }
    }

    #[test]
    fn shortcuts_use_secondary_for_the_platform_command_key() {
        assert_eq!(canonicalize("cmd-p", true), "secondary-p");
        assert_eq!(canonicalize("ctrl-p", false), "secondary-p");
        assert_eq!(canonicalize("ctrl-tab", true), "ctrl-tab");
        assert_eq!(canonicalize("ctrl-shift-tab", false), "secondary-shift-tab");
        assert_eq!(
            canonicalize("shift-secondary-f", true),
            canonicalize("secondary-shift-f", true)
        );
        assert_eq!(
            canonicalize("secondary-k secondary-t", true),
            "secondary-k secondary-t"
        );
    }

    #[test]
    fn a_cleared_shortcut_beats_the_default_and_a_repeat_does_not() {
        assert_eq!(
            desired_key(Some("secondary-p"), None, true).0.as_deref(),
            Some("secondary-p")
        );
        assert_eq!(desired_key(Some("secondary-p"), Some(""), true).0, None);
        let (key, redundant) = desired_key(Some("secondary-p"), Some("cmd-p"), true);
        assert_eq!(key.as_deref(), Some("secondary-p"));
        assert!(redundant);
        assert_eq!(
            desired_key(Some("secondary-p"), Some("secondary-shift-p"), true)
                .0
                .as_deref(),
            Some("secondary-shift-p")
        );
    }

    #[test]
    fn an_explicit_shortcut_takes_the_key_from_a_default() {
        let mut choices = vec![
            choice("palette", Some("secondary-p"), false),
            choice("back", Some("secondary-p"), true),
        ];
        let stolen = resolve_conflicts(&mut choices);
        assert_eq!(stolen, ["palette"]);
        assert!(choices[0].desired.is_none());
        assert_eq!(choices[1].desired.as_deref(), Some("secondary-p"));
    }
}
