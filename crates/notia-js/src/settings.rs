//! The settings tabs of community plugins: the elements a plugin's `render`
//! returns, as sections of settings in the settings dialog.

use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
    time::{Duration, Instant},
};

use gpui_kit::{
    AnyElement, App, IntoElement as _, ParentElement as _, SharedString, Styled as _, Window,
    component::ActiveTheme as _, div,
};
use notia_plugin::{Setting, SettingsSection};
use serde::Serialize;
use serde_json::Value;

use crate::{
    runtime::JsRuntime,
    view::{Handlers, Node, render_node},
};

/// How long a tab's elements are reused while the dialog keeps drawing. A
/// change made on the tab renders it again at once; this catches changes
/// made elsewhere, such as by a command.
const FRESH_FOR: Duration = Duration::from_millis(500);

/// Reads a control's value and `onChange` handler, or `None` for another
/// kind of element.
type Read<T> = fn(&Node) -> Option<(T, Option<usize>)>;

/// A community plugin's settings tab.
pub(crate) struct JsSettings {
    js: Weak<JsRuntime>,
    id: Rc<str>,
    dirty: Cell<bool>,
    rendered_at: Cell<Option<Instant>>,
    /// The top-level elements of the latest render.
    content: RefCell<Result<Vec<Node>, SharedString>>,
}

impl JsSettings {
    pub(crate) fn new(js: Weak<JsRuntime>, id: String) -> Self {
        Self {
            js,
            id: id.into(),
            dirty: Cell::new(true),
            rendered_at: Cell::new(None),
            content: RefCell::new(Ok(Vec::new())),
        }
    }

    /// Renders the tab again the next time the dialog draws it.
    pub(crate) fn invalidate(&self) {
        self.dirty.set(true);
    }

    pub(crate) fn sections(self: &Rc<Self>, cx: &mut App) -> Vec<SettingsSection> {
        let stale = self.dirty.get()
            || self
                .rendered_at
                .get()
                .is_none_or(|at| at.elapsed() >= FRESH_FOR);
        if stale {
            self.render(cx);
        }
        let content = self.content.borrow();
        match &*content {
            Ok(nodes) => self.to_sections(nodes),
            Err(error) => {
                let error = error.clone();
                vec![SettingsSection::new().setting(Setting::row(move |_, cx| {
                    div()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(error.clone())
                        .into_any_element()
                }))]
            }
        }
    }

    fn render(&self, cx: &mut App) {
        self.dirty.set(false);
        self.rendered_at.set(Some(Instant::now()));
        let content = match self.js.upgrade() {
            Some(js) => js.render_view(&self.id, cx).and_then(|json| {
                serde_json::from_str::<Node>(&json)
                    .map_err(|error| format!("The settings are invalid: {error}"))
            }),
            None => Err("The plugin is disabled.".to_string()),
        };
        *self.content.borrow_mut() = content.map(top_level).map_err(Into::into);
    }

    /// Settings in order, with a new section at each heading.
    fn to_sections(self: &Rc<Self>, nodes: &[Node]) -> Vec<SettingsSection> {
        let mut sections: Vec<(Option<String>, Vec<Setting>)> = vec![(None, Vec::new())];
        for (ix, node) in nodes.iter().enumerate() {
            let setting = match node {
                Node::Heading { text } => {
                    sections.push((Some(text.clone()), Vec::new()));
                    continue;
                }
                Node::Setting {
                    name,
                    description,
                    control,
                } => {
                    let setting = self.setting(ix, name, control.as_deref());
                    match description {
                        Some(description) => setting.with_description(description.clone()),
                        None => setting,
                    }
                }
                node => Setting::row(self.element(node.clone())),
            };
            if let Some((_, settings)) = sections.last_mut() {
                settings.push(setting);
            }
        }
        sections
            .into_iter()
            .map(|(title, settings)| {
                let section = SettingsSection::new().settings(settings);
                match title {
                    Some(title) => section.with_title(title),
                    None => section,
                }
            })
            .collect()
    }

    fn setting(self: &Rc<Self>, ix: usize, name: &str, control: Option<&Node>) -> Setting {
        let name = name.to_string();
        match control {
            Some(Node::Toggle { .. }) => {
                Setting::toggle(name, self.get(ix, toggle), self.set(ix, toggle))
            }
            Some(Node::TextInput { .. }) => {
                Setting::text(name, self.get(ix, text), self.set(ix, text))
            }
            Some(Node::NumberInput { min, max, step, .. }) => Setting::number(
                name,
                min.unwrap_or(f64::MIN)..=max.unwrap_or(f64::MAX),
                step.unwrap_or(1.),
                self.get(ix, number),
                self.set(ix, number),
            ),
            Some(Node::Dropdown { options, .. }) => Setting::choice(
                name,
                options
                    .iter()
                    .map(|option| (option.value.clone(), option.label.clone())),
                self.get(ix, choice),
                self.set(ix, choice),
            ),
            Some(control) => Setting::custom(name, self.element(control.clone())),
            None => Setting::custom(name, |_, _| div().into_any_element()),
        }
    }

    /// Reads the value of the control at `ix` in the latest render, which
    /// may be newer than the render this was made from.
    fn get<T: Default + 'static>(
        self: &Rc<Self>,
        ix: usize,
        read: Read<T>,
    ) -> impl Fn(&App) -> T + 'static {
        let page = Rc::downgrade(self);
        move |_| {
            page.upgrade()
                .and_then(|page| page.control(ix, |node| read(node).map(|(value, _)| value)))
                .unwrap_or_default()
        }
    }

    /// Calls the `onChange` of the control at `ix` in the latest render with
    /// the new value, as long as that control is still of the same kind.
    fn set<T: Serialize + 'static>(
        self: &Rc<Self>,
        ix: usize,
        read: Read<T>,
    ) -> impl Fn(T, &mut App) + 'static {
        let page = Rc::downgrade(self);
        move |value, cx| {
            let Some(page) = page.upgrade() else {
                return;
            };
            let handler = page.control(ix, |node| read(node).and_then(|(_, handler)| handler));
            if let Some(handler) = handler {
                let value = serde_json::to_value(value).unwrap_or(Value::Null);
                page.handlers().call(handler, vec![value], cx);
            }
        }
    }

    fn control<R>(&self, ix: usize, f: impl FnOnce(&Node) -> Option<R>) -> Option<R> {
        match self.content.borrow().as_ref().ok()?.get(ix)? {
            Node::Setting {
                control: Some(control),
                ..
            } => f(control),
            _ => None,
        }
    }

    fn element(
        self: &Rc<Self>,
        node: Node,
    ) -> impl Fn(&mut Window, &mut App) -> AnyElement + 'static {
        let handlers = self.handlers();
        move |_, cx| render_node(&node, &handlers, &mut 0, cx)
    }

    /// Handlers that render the tab again after they run, since they may
    /// have changed what it shows.
    fn handlers(self: &Rc<Self>) -> Handlers {
        let page = Rc::downgrade(self);
        Handlers {
            js: self.js.clone(),
            view: self.id.clone(),
            after: Some(Rc::new(move |cx: &mut App| {
                if let Some(page) = page.upgrade() {
                    page.invalidate();
                }
                cx.refresh_windows();
            })),
        }
    }
}

/// What `render` returned, without the column the prelude puts an array in.
fn top_level(node: Node) -> Vec<Node> {
    match node {
        Node::Column { children } => children,
        node => vec![node],
    }
}

fn toggle(node: &Node) -> Option<(bool, Option<usize>)> {
    match node {
        Node::Toggle { value, on_change } => Some((*value, *on_change)),
        _ => None,
    }
}

fn text(node: &Node) -> Option<(SharedString, Option<usize>)> {
    match node {
        Node::TextInput { value, on_change } => Some((value.clone().into(), *on_change)),
        _ => None,
    }
}

fn number(node: &Node) -> Option<(f64, Option<usize>)> {
    match node {
        Node::NumberInput {
            value, on_change, ..
        } => Some((*value, *on_change)),
        _ => None,
    }
}

fn choice(node: &Node) -> Option<(SharedString, Option<usize>)> {
    match node {
        Node::Dropdown {
            value, on_change, ..
        } => Some((value.clone().into(), *on_change)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use notia_plugin::SettingControl;

    use super::*;

    fn sections(json: &str) -> Vec<SettingsSection> {
        let page = Rc::new(JsSettings::new(Weak::new(), "settings:1".into()));
        let nodes = top_level(serde_json::from_str(json).unwrap());
        page.to_sections(&nodes)
    }

    #[test]
    fn headings_start_sections() {
        let sections = sections(
            r#"{ "type": "column", "children": [
                { "type": "text", "text": "Intro" },
                { "type": "heading", "text": "Bar" },
                { "type": "setting", "name": "Show", "description": "In the bar.",
                  "control": { "type": "toggle", "value": true, "onChange": 0 } },
                { "type": "setting", "name": "Size",
                  "control": { "type": "number_input", "value": 3, "min": 1, "max": 9, "onChange": 1 } },
                { "type": "heading", "text": "Links" },
                { "type": "setting", "name": "Style",
                  "control": { "type": "dropdown", "value": "wiki", "onChange": 2,
                    "options": [{ "value": "wiki", "label": "Wikilinks" }, { "value": "md", "label": "Markdown" }] } },
                { "type": "setting", "name": "Prefix", "control": { "type": "text_input", "value": "> ", "onChange": 3 } },
                { "type": "setting", "name": "Reset", "control": { "type": "button", "label": "Reset", "onClick": 4 } }
            ] }"#,
        );
        let titles: Vec<_> = sections
            .iter()
            .map(|section| section.title().cloned())
            .collect();
        assert_eq!(titles, [None, Some("Bar".into()), Some("Links".into())]);

        let intro = &sections[0].items()[0];
        assert!(intro.name().is_empty());
        assert!(matches!(intro.control(), SettingControl::Custom(_)));

        let bar = sections[1].items();
        assert_eq!(bar[0].name().as_ref(), "Show");
        assert_eq!(bar[0].description().map(AsRef::as_ref), Some("In the bar."));
        assert!(matches!(bar[0].control(), SettingControl::Toggle(_)));
        match bar[1].control() {
            SettingControl::Number { range, step, .. } => {
                assert_eq!((range.clone(), *step), (1.0..=9.0, 1.0));
            }
            _ => panic!("Size is a number"),
        }

        let links = sections[2].items();
        match links[0].control() {
            SettingControl::Choice { options, .. } => {
                assert_eq!(options[1], ("md".into(), "Markdown".into()));
            }
            _ => panic!("Style is a choice"),
        }
        assert!(matches!(links[1].control(), SettingControl::Text(_)));
        assert!(matches!(links[2].control(), SettingControl::Custom(_)));
    }

    #[test]
    fn a_single_element_is_one_row() {
        let sections = sections(r#"{ "type": "text", "text": "Nothing to set." }"#);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].items().len(), 1);
    }
}
