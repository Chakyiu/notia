//! The views of community plugins: status bar text, and panels built from
//! the elements a plugin's `render` returns.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::{Rc, Weak},
};

use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    scroll::ScrollableElement as _,
    text::TextView,
    v_flex,
};
use gpui_kit::{prelude::FluentBuilder as _, *};
use serde::Deserialize;
use serde_json::Value;

use crate::{icon, runtime::JsRuntime};

/// A status bar item showing the text its plugin set.
#[derive(Default)]
pub(crate) struct StatusText {
    text: SharedString,
}

impl StatusText {
    pub(crate) fn set_text(&mut self, text: String, cx: &mut Context<Self>) {
        if self.text != text {
            self.text = text.into();
            cx.notify();
        }
    }
}

impl Render for StatusText {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child(self.text.clone())
    }
}

fn wraps_by_default() -> bool {
    true
}

fn default_rows() -> usize {
    14
}

/// An element of a panel or settings tab, as made by the `ui` functions of
/// the plugin API. Handlers are indices into the view's handlers.
#[derive(Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum Node {
    Column {
        #[serde(default)]
        children: Vec<Node>,
    },
    Row {
        #[serde(default)]
        children: Vec<Node>,
        /// When false, children stay on one line and the bar scrolls.
        #[serde(default = "wraps_by_default")]
        wrap: bool,
    },
    Heading {
        text: String,
    },
    Text {
        text: String,
        #[serde(default)]
        muted: bool,
        #[serde(default)]
        bold: bool,
        #[serde(default)]
        small: bool,
        #[serde(default)]
        danger: bool,
    },
    Markdown {
        source: String,
    },
    Button {
        label: String,
        #[serde(default)]
        icon: Option<String>,
        #[serde(default)]
        variant: Option<String>,
        #[serde(default)]
        tooltip: Option<String>,
        #[serde(default, rename = "onClick")]
        on_click: Option<usize>,
    },
    List {
        #[serde(default)]
        items: Vec<ListItem>,
    },
    Separator,
    Empty {
        text: String,
    },
    /// A scrolling body with a footer that stays at the bottom of the panel.
    Dock {
        #[serde(default)]
        body: Option<Box<Node>>,
        #[serde(default)]
        footer: Option<Box<Node>>,
    },
    /// A row of a settings tab, with one of the controls below.
    Setting {
        name: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        control: Option<Box<Node>>,
    },
    Toggle {
        value: bool,
        #[serde(default, rename = "onChange")]
        on_change: Option<usize>,
    },
    TextInput {
        #[serde(default)]
        value: String,
        #[serde(default)]
        placeholder: String,
        #[serde(default, rename = "onChange")]
        on_change: Option<usize>,
    },
    /// A multi-line field. `id` keeps the editor across renders.
    TextArea {
        #[serde(default)]
        id: String,
        #[serde(default)]
        value: String,
        #[serde(default = "default_rows")]
        rows: usize,
        #[serde(default)]
        placeholder: String,
        #[serde(default, rename = "onChange")]
        on_change: Option<usize>,
        /// Enter submits. Shift+Enter still inserts a line.
        #[serde(default, rename = "onSubmit")]
        on_submit: Option<usize>,
    },
    NumberInput {
        #[serde(default)]
        value: f64,
        #[serde(default)]
        min: Option<f64>,
        #[serde(default)]
        max: Option<f64>,
        #[serde(default)]
        step: Option<f64>,
        #[serde(default, rename = "onChange")]
        on_change: Option<usize>,
    },
    Dropdown {
        #[serde(default)]
        value: String,
        #[serde(default)]
        options: Vec<DropdownOption>,
        #[serde(default, rename = "onChange")]
        on_change: Option<usize>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Deserialize)]
pub(crate) struct DropdownOption {
    pub(crate) value: String,
    pub(crate) label: String,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListItem {
    title: String,
    #[serde(default)]
    subtitle: Option<String>,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    on_click: Option<usize>,
}

/// A sidebar panel of a community plugin. It asks the plugin to render
/// when it is first shown and whenever the plugin refreshes it.
pub(crate) struct JsPanel {
    js: Weak<JsRuntime>,
    id: Rc<str>,
    dirty: Rc<Cell<bool>>,
    /// A short bar above the note, instead of a sidebar that fills its column.
    bar: bool,
    content: Result<Node, SharedString>,
    /// Text fields, kept across renders so typing is not thrown away.
    fields: PanelFields,
    /// The message list, so a new reply sits at the bottom.
    scroll: ScrollHandle,
}

impl JsPanel {
    pub(crate) fn new(js: Weak<JsRuntime>, id: String, dirty: Rc<Cell<bool>>, bar: bool) -> Self {
        let id: Rc<str> = id.into();
        Self {
            fields: PanelFields::new(js.clone(), id.clone()),
            js,
            id,
            dirty,
            bar,
            content: Ok(Node::Column {
                children: Vec::new(),
            }),
            scroll: ScrollHandle::new(),
        }
    }
}

impl Render for JsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A fresh render is one the plugin asked for. Text fields take their
        // value from the plugin then, and keep keystrokes in between.
        let fresh = self.dirty.replace(false);
        if fresh {
            self.content = match self.js.upgrade() {
                Some(js) => js.render_view(&self.id, cx).and_then(|json| {
                    serde_json::from_str(&json)
                        .map_err(|error| format!("The panel is invalid: {error}"))
                }),
                None => Err("The plugin is disabled.".to_string()),
            }
            .map_err(Into::into);
        }

        let handlers = Handlers {
            js: self.js.clone(),
            view: self.id.clone(),
            after: None,
        };
        let mut ids = 0;
        let dock = match &self.content {
            Ok(Node::Dock { body, footer }) if !self.bar => Some((present(body), present(footer))),
            _ => None,
        };
        let panel = v_flex()
            .id(SharedString::from(format!("js-panel-{}", self.id)))
            .w_full();
        if let Some((body, footer)) = dock {
            let body = render_node(
                &body,
                &handlers,
                &mut ids,
                Some(&self.fields),
                fresh,
                window,
                cx,
            );
            let footer = render_node(
                &footer,
                &handlers,
                &mut ids,
                Some(&self.fields),
                fresh,
                window,
                cx,
            );
            let border = cx.theme().border;
            if fresh {
                self.scroll.scroll_to_bottom();
            }
            return panel
                .size_full()
                .min_h_0()
                .child(
                    div()
                        .id(SharedString::from(format!("js-panel-scroll-{}", self.id)))
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .child(v_flex().p_3().gap_3().w_full().child(body)),
                )
                .child(
                    v_flex()
                        .flex_none()
                        .w_full()
                        .gap_2()
                        .p_3()
                        .border_t_1()
                        .border_color(border)
                        .child(footer),
                )
                .into_any_element();
        }

        let body = match &self.content {
            Ok(node) => render_node(
                node,
                &handlers,
                &mut ids,
                Some(&self.fields),
                fresh,
                window,
                cx,
            ),
            Err(error) => div()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(error.clone())
                .into_any_element(),
        };
        if self.bar {
            panel
                .flex_none()
                .min_w_0()
                .overflow_x_scrollbar()
                .py_0p5()
                .child(body)
                .into_any_element()
        } else {
            panel
                .size_full()
                .overflow_y_scrollbar()
                .child(v_flex().p_3().gap_2().child(body))
                .into_any_element()
        }
    }
}

type AfterHandler = Rc<dyn Fn(&mut App)>;

/// Runs the handlers of a view's elements.
#[derive(Clone)]
pub(crate) struct Handlers {
    pub(crate) js: Weak<JsRuntime>,
    pub(crate) view: Rc<str>,
    /// Runs after each handler.
    pub(crate) after: Option<AfterHandler>,
}

impl Handlers {
    pub(crate) fn call(&self, handler: usize, args: Vec<Value>, cx: &mut App) {
        if let Some(js) = self.js.upgrade() {
            js.call_handler(&self.view, handler, args, cx);
        }
        if let Some(after) = &self.after {
            after(cx);
        }
    }

    fn on_click(&self, handler: usize) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        let this = self.clone();
        move |_, _, cx| this.call(handler, Vec::new(), cx)
    }
}

pub(crate) fn render_node(
    node: &Node,
    handlers: &Handlers,
    ids: &mut usize,
    fields: Option<&PanelFields>,
    fresh: bool,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let mut next_id = || {
        *ids += 1;
        ElementId::from(("js-element", *ids))
    };
    match node {
        Node::Column { children } => v_flex()
            .gap_2()
            .children(
                children
                    .iter()
                    .map(|child| render_node(child, handlers, ids, fields, fresh, window, cx)),
            )
            .into_any_element(),
        Node::Row { children, wrap } => h_flex()
            .items_center()
            .when(*wrap, |row| row.gap_2().flex_wrap())
            .when(!*wrap, |row| row.gap_2().flex_nowrap())
            .children(
                children
                    .iter()
                    .map(|child| render_node(child, handlers, ids, fields, fresh, window, cx)),
            )
            .into_any_element(),
        Node::Heading { text } => div()
            .pt_1()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(muted)
            .child(text.clone())
            .into_any_element(),
        Node::Text {
            text,
            muted: is_muted,
            bold,
            small,
            danger,
        } => div()
            .map(|label| {
                if *small {
                    label.text_xs()
                } else {
                    label.text_sm()
                }
            })
            .when(*is_muted, |label| label.text_color(muted))
            .when(*danger, |label| label.text_color(theme.danger))
            .when(*bold, |label| label.font_weight(FontWeight::SEMIBOLD))
            .child(text.clone())
            .into_any_element(),
        Node::Markdown { source } => div()
            .text_sm()
            .child(TextView::markdown(next_id(), source.clone()))
            .into_any_element(),
        Node::Button {
            label,
            icon: icon_name,
            variant,
            tooltip,
            on_click,
        } => Button::new(next_id())
            .when(label.is_empty(), |button| button.xsmall())
            .when(!label.is_empty(), |button| button.small())
            .map(|button| match variant.as_deref() {
                Some("primary") => button.primary(),
                Some("ghost") => button.ghost(),
                _ => button.outline(),
            })
            .when(!label.is_empty(), |button| button.label(label.clone()))
            .when_some(tooltip.clone(), |button, tip| button.tooltip(tip))
            .when_some(icon_name.as_deref(), |button, name| {
                button.icon(icon(Some(name)))
            })
            .when_some(*on_click, |button, handler| {
                button.on_click(handlers.on_click(handler))
            })
            .into_any_element(),
        Node::List { items } => {
            let hover = theme.list_hover;
            let radius = theme.radius;
            v_flex()
                .children(items.iter().map(|item| {
                    div()
                        .id(next_id())
                        .px_2()
                        .py_1()
                        .rounded(radius)
                        .child(
                            h_flex()
                                .gap_2()
                                .when_some(item.icon.as_deref(), |row, name| {
                                    row.child(icon(Some(name)).small().text_color(muted))
                                })
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .child(div().text_sm().truncate().child(item.title.clone()))
                                        .when_some(item.subtitle.clone(), |column, subtitle| {
                                            column.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(muted)
                                                    .truncate()
                                                    .child(subtitle),
                                            )
                                        }),
                                ),
                        )
                        .when_some(item.on_click, |row, handler| {
                            row.cursor_pointer()
                                .hover(move |style| style.bg(hover))
                                .on_click(handlers.on_click(handler))
                        })
                }))
                .into_any_element()
        }
        Node::Separator => div().h_px().w_full().bg(theme.border).into_any_element(),
        Node::Empty { text } => div()
            .py_4()
            .w_full()
            .text_center()
            .text_sm()
            .text_color(muted)
            .child(text.clone())
            .into_any_element(),
        Node::Dock { body, footer } => v_flex()
            .gap_2()
            .child(render_node(
                &present(body),
                handlers,
                ids,
                fields,
                fresh,
                window,
                cx,
            ))
            .child(render_node(
                &present(footer),
                handlers,
                ids,
                fields,
                fresh,
                window,
                cx,
            ))
            .into_any_element(),
        Node::TextInput {
            value,
            placeholder,
            on_change,
        } => match fields {
            Some(fields) => {
                let key = next_field_key("", ids);
                fields.line(key, value, placeholder, *on_change, fresh, window, cx)
            }
            None => settings_only(muted),
        },
        Node::TextArea {
            id,
            value,
            rows,
            placeholder,
            on_change,
            on_submit,
        } => match fields {
            Some(fields) => {
                let key = next_field_key(id, ids);
                fields.area(
                    key,
                    value,
                    placeholder,
                    *rows,
                    *on_change,
                    *on_submit,
                    fresh,
                    window,
                    cx,
                )
            }
            None => settings_only(muted),
        },
        Node::Setting { .. }
        | Node::Toggle { .. }
        | Node::NumberInput { .. }
        | Node::Dropdown { .. } => settings_only(muted),
        Node::Unknown => div()
            .text_xs()
            .text_color(theme.danger)
            .child("This element is not supported.")
            .into_any_element(),
    }
}

fn present(node: &Option<Box<Node>>) -> Node {
    match node {
        Some(node) => (**node).clone(),
        None => Node::Column {
            children: Vec::new(),
        },
    }
}

fn next_field_key(id: &str, ids: &mut usize) -> String {
    if !id.is_empty() {
        id.to_string()
    } else {
        *ids += 1;
        format!("field-{ids}")
    }
}

fn settings_only(muted: impl Clone + Into<gpui_kit::Hsla>) -> AnyElement {
    div()
        .text_xs()
        .text_color(muted)
        .child("This control shows only on a settings tab.")
        .into_any_element()
}

enum FieldKind {
    Line(Entity<InputState>),
    Area(Entity<TextareaState>),
}

struct FieldSlot {
    kind: FieldKind,
    on_change: Rc<Cell<Option<usize>>>,
    on_submit: Rc<Cell<Option<usize>>>,
    _subscription: Subscription,
}

/// Text fields of one panel. The same id keeps one editor, so a refresh
/// does not throw away what the user is typing.
pub(crate) struct PanelFields {
    js: Weak<JsRuntime>,
    view: Rc<str>,
    editors: RefCell<HashMap<String, FieldSlot>>,
}

impl PanelFields {
    fn new(js: Weak<JsRuntime>, view: Rc<str>) -> Self {
        Self {
            js,
            view,
            editors: RefCell::new(HashMap::new()),
        }
    }

    fn line(
        &self,
        key: String,
        value: &str,
        placeholder: &str,
        on_change: Option<usize>,
        fresh: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.ensure_line(&key, value, placeholder, on_change, window, cx);
        self.sync_line(&key, value, fresh, window, cx);
        self.editors
            .borrow()
            .get(&key)
            .and_then(|slot| match &slot.kind {
                FieldKind::Line(state) => Some(state.clone()),
                FieldKind::Area(_) => None,
            })
            .map(|state| Input::new(&state).small().w_full().into_any_element())
            .unwrap_or_else(|| div().into_any_element())
    }

    fn area(
        &self,
        key: String,
        value: &str,
        placeholder: &str,
        rows: usize,
        on_change: Option<usize>,
        on_submit: Option<usize>,
        fresh: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.ensure_area(
            &key,
            value,
            placeholder,
            rows,
            on_change,
            on_submit,
            window,
            cx,
        );
        self.sync_area(&key, value, fresh, window, cx);
        let rows = rows.clamp(1, 8) as f32;
        self.editors
            .borrow()
            .get(&key)
            .and_then(|slot| match &slot.kind {
                FieldKind::Area(state) => Some(state.clone()),
                FieldKind::Line(_) => None,
            })
            .map(|state| {
                Textarea::new(&state)
                    .w_full()
                    .h(px(rows * 22. + 12.))
                    .text_sm()
                    .into_any_element()
            })
            .unwrap_or_else(|| div().into_any_element())
    }

    fn ensure_line(
        &self,
        key: &str,
        value: &str,
        placeholder: &str,
        on_change: Option<usize>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let exists = self
            .editors
            .borrow()
            .get(key)
            .is_some_and(|slot| matches!(slot.kind, FieldKind::Line(_)));
        if exists {
            self.set_handlers(key, on_change, None);
            return;
        }
        self.editors.borrow_mut().remove(key);
        let state = cx.new(|cx| {
            let editor = InputState::new(window, cx).placeholder(placeholder.to_string());
            if value.is_empty() {
                editor
            } else {
                editor.default_value(value.to_string())
            }
        });
        let on_change_cell = Rc::new(Cell::new(on_change));
        let on_submit_cell = Rc::new(Cell::new(None));
        let watched = state.clone();
        let subscription = self.bind(
            &state,
            on_change_cell.clone(),
            on_submit_cell.clone(),
            move |cx| watched.read(cx).value().to_string(),
            cx,
        );
        self.editors.borrow_mut().insert(
            key.to_string(),
            FieldSlot {
                kind: FieldKind::Line(state),
                on_change: on_change_cell,
                on_submit: on_submit_cell,
                _subscription: subscription,
            },
        );
    }

    fn ensure_area(
        &self,
        key: &str,
        value: &str,
        placeholder: &str,
        rows: usize,
        on_change: Option<usize>,
        on_submit: Option<usize>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let exists = self
            .editors
            .borrow()
            .get(key)
            .is_some_and(|slot| matches!(slot.kind, FieldKind::Area(_)));
        if exists {
            self.set_handlers(key, on_change, on_submit);
            return;
        }
        self.editors.borrow_mut().remove(key);
        let state = cx.new(|cx| {
            let mut editor = TextareaState::new(window, cx)
                .placeholder(placeholder.to_string())
                .rows(rows.clamp(1, 12));
            if on_submit.is_some() {
                editor = editor.submit_on_enter(true);
            }
            if !value.is_empty() {
                editor = editor.default_value(value.to_string());
            }
            editor
        });
        let on_change_cell = Rc::new(Cell::new(on_change));
        let on_submit_cell = Rc::new(Cell::new(on_submit));
        let watched = state.clone();
        let subscription = self.bind(
            &state,
            on_change_cell.clone(),
            on_submit_cell.clone(),
            move |cx| watched.read(cx).value().to_string(),
            cx,
        );
        self.editors.borrow_mut().insert(
            key.to_string(),
            FieldSlot {
                kind: FieldKind::Area(state),
                on_change: on_change_cell,
                on_submit: on_submit_cell,
                _subscription: subscription,
            },
        );
    }

    /// `on_change` runs as the text changes. Enter runs `on_submit`.
    fn bind<T>(
        &self,
        state: &Entity<T>,
        on_change: Rc<Cell<Option<usize>>>,
        on_submit: Rc<Cell<Option<usize>>>,
        text: impl Fn(&App) -> String + 'static,
        cx: &mut App,
    ) -> Subscription
    where
        T: gpui_kit::EventEmitter<InputEvent> + 'static,
    {
        let js = self.js.clone();
        let view = self.view.clone();
        cx.subscribe(state, move |_, event: &InputEvent, cx| {
            let handler = match event {
                InputEvent::Change => on_change.get(),
                InputEvent::PressEnter { shift, .. } if !*shift => on_submit.get(),
                _ => None,
            };
            let Some(handler) = handler else {
                return;
            };
            Handlers {
                js: js.clone(),
                view: view.clone(),
                after: None,
            }
            .call(handler, vec![Value::String(text(cx))], cx);
        })
    }

    fn set_handlers(&self, key: &str, on_change: Option<usize>, on_submit: Option<usize>) {
        let editors = self.editors.borrow();
        if let Some(slot) = editors.get(key) {
            slot.on_change.set(on_change);
            if on_submit.is_some() || matches!(slot.kind, FieldKind::Area(_)) {
                slot.on_submit.set(on_submit);
            }
        }
    }

    fn sync_line(&self, key: &str, value: &str, fresh: bool, window: &mut Window, cx: &mut App) {
        if !fresh {
            return;
        }
        let Some(state) = self
            .editors
            .borrow()
            .get(key)
            .and_then(|slot| match &slot.kind {
                FieldKind::Line(state) => Some(state.clone()),
                FieldKind::Area(_) => None,
            })
        else {
            return;
        };
        if state.read(cx).value().as_ref() != value {
            state.update(cx, |state, cx| {
                state.set_value(value.to_string(), window, cx)
            });
        }
    }

    fn sync_area(&self, key: &str, value: &str, fresh: bool, window: &mut Window, cx: &mut App) {
        if !fresh {
            return;
        }
        let Some(state) = self
            .editors
            .borrow()
            .get(key)
            .and_then(|slot| match &slot.kind {
                FieldKind::Area(state) => Some(state.clone()),
                FieldKind::Line(_) => None,
            })
        else {
            return;
        };
        if state.read(cx).value().as_ref() != value {
            state.update(cx, |state, cx| {
                state.set_value(value.to_string(), window, cx)
            });
        }
    }
}
