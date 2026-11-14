//! The views of community plugins: status bar text, and panels built from
//! the elements a plugin's `render` returns.

use std::{
    cell::Cell,
    rc::{Rc, Weak},
};

use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
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
        #[serde(default, rename = "onChange")]
        on_change: Option<usize>,
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
}

impl JsPanel {
    pub(crate) fn new(js: Weak<JsRuntime>, id: String, dirty: Rc<Cell<bool>>, bar: bool) -> Self {
        Self {
            js,
            id: id.into(),
            dirty,
            bar,
            content: Ok(Node::Column {
                children: Vec::new(),
            }),
        }
    }
}

impl Render for JsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.dirty.replace(false) {
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
        let body = match &self.content {
            Ok(node) => render_node(node, &handlers, &mut ids, cx),
            Err(error) => div()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(error.clone())
                .into_any_element(),
        };
        let panel = v_flex()
            .id(SharedString::from(format!("js-panel-{}", self.id)))
            .w_full();
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
    cx: &App,
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
                    .map(|child| render_node(child, handlers, ids, cx)),
            )
            .into_any_element(),
        Node::Row { children, wrap } => h_flex()
            .items_center()
            .when(*wrap, |row| row.gap_2().flex_wrap())
            .when(!*wrap, |row| row.gap_2().flex_nowrap())
            .children(
                children
                    .iter()
                    .map(|child| render_node(child, handlers, ids, cx)),
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
        } => div()
            .map(|label| {
                if *small {
                    label.text_xs()
                } else {
                    label.text_sm()
                }
            })
            .when(*is_muted, |label| label.text_color(muted))
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
        Node::Setting { .. }
        | Node::Toggle { .. }
        | Node::TextInput { .. }
        | Node::NumberInput { .. }
        | Node::Dropdown { .. } => div()
            .text_xs()
            .text_color(muted)
            .child("Settings show only on a settings tab, at its top level.")
            .into_any_element(),
        Node::Unknown => div()
            .text_xs()
            .text_color(theme.danger)
            .child("This element is not supported.")
            .into_any_element(),
    }
}
