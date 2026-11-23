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

/// Largest `gap` a plugin can set, in px. Anything past this clamps.
const MAX_GAP: f32 = 256.;
/// Largest `minHeight` or `rowHeight`, in px.
const MAX_LENGTH: f32 = 1024.;
/// Equal tracks in one row. Capped so a bad value cannot build a huge track list.
const MAX_COLUMNS: u16 = 31;
/// A marker dot: 4px, drawn 3px under the label, inside the button.
const MARKER_DOT: f32 = 4.;
const MARKER_GAP: f32 = 3.;
/// Tall enough for the label and the dot. The stock button is 24px and would clip it.
const MARKER_BUTTON: f32 = 36.;

/// `None` keeps the 8px gap rows and columns have always used.
#[derive(Clone, Copy, Debug, PartialEq)]
enum StackGap {
    Legacy,
    Px(f32),
}

fn explicit_px(value: Option<f32>, max: f32) -> Option<f32> {
    value.and_then(|value| value.is_finite().then_some(value.clamp(0., max)))
}

fn stack_gap(value: Option<f32>) -> StackGap {
    match explicit_px(value, MAX_GAP) {
        Some(value) => StackGap::Px(value),
        None => StackGap::Legacy,
    }
}

fn with_stack_gap<E: Styled>(element: E, gap: Option<f32>) -> E {
    match stack_gap(gap) {
        StackGap::Legacy => element.gap_2(),
        StackGap::Px(value) => element.gap(px(value)),
    }
}

fn grid_track_count(columns: u16) -> u16 {
    columns.clamp(1, MAX_COLUMNS)
}

/// `"start"` left-aligns a cell. Anything else centers it, so stacked rows
/// of the same width share one vertical axis per column.
fn cell_centers(align: Option<&str>) -> bool {
    align != Some("start")
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum MarkerTone {
    Accent,
    PrimaryForeground,
}

fn marker_tone(tone: Option<&str>) -> MarkerTone {
    match tone {
        Some("primaryForeground") => MarkerTone::PrimaryForeground,
        _ => MarkerTone::Accent,
    }
}

/// An element of a panel or settings tab, as made by the `ui` functions of
/// the plugin API. Handlers are indices into the view's handlers.
#[derive(Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum Node {
    Column {
        #[serde(default)]
        children: Vec<Node>,
        /// Px. Omitted, the column keeps its 8px gap.
        #[serde(default)]
        gap: Option<f32>,
        /// Px. Keeps a block the same height when its contents change.
        #[serde(default, rename = "minHeight")]
        min_height: Option<f32>,
    },
    Row {
        #[serde(default)]
        children: Vec<Node>,
        /// When false, children stay on one line and the bar scrolls.
        #[serde(default = "wraps_by_default")]
        wrap: bool,
        /// Puts the first child at the start and the last at the end.
        #[serde(default)]
        spread: bool,
        /// Px. Omitted, the row keeps its 8px gap.
        #[serde(default)]
        gap: Option<f32>,
        /// When set, children sit in this many equal tracks
        /// (`repeat(n, minmax(0, 1fr))`). Omitted, the row stays a flex row:
        /// a host from before this field ignores it and draws that same flex row.
        #[serde(default)]
        columns: Option<u16>,
        /// Px. Every cell is this tall, so stacked weeks share one pitch.
        /// Read only when `columns` is set.
        #[serde(default, rename = "rowHeight")]
        row_height: Option<f32>,
        /// `"start"` left-aligns a cell. Anything else centers it.
        /// Read only when `columns` is set.
        #[serde(default)]
        align: Option<String>,
    },
    /// A bordered field: a question, and the controls under it.
    Composer {
        #[serde(default)]
        children: Vec<Node>,
    },
    Heading {
        text: String,
    },
    /// A Lucide icon, with no button around it.
    Icon {
        #[serde(default)]
        name: String,
        #[serde(default)]
        muted: bool,
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
        /// Label uses the theme's muted foreground. The button stays clickable.
        #[serde(default)]
        muted: bool,
        /// A 4px dot under the label, drawn inside the button.
        #[serde(default)]
        dot: bool,
        /// `"accent"` (default) or `"primaryForeground"` (ink on a filled primary button).
        #[serde(default, rename = "dotTone")]
        dot_tone: Option<String>,
    },
    List {
        #[serde(default)]
        items: Vec<ListItem>,
        /// Tighter rows, for a log of tool calls.
        #[serde(default)]
        compact: bool,
    },
    /// A short label. `on_close` draws an × that removes it.
    Chip {
        #[serde(default)]
        label: String,
        #[serde(default, rename = "onClose")]
        on_close: Option<usize>,
    },
    Separator,
    Empty {
        text: String,
    },
    /// Chats across the top of a panel, with actions at the end.
    TabBar {
        #[serde(default)]
        tabs: Vec<ChatTab>,
        #[serde(default)]
        actions: Vec<Node>,
    },
    /// A scrolling body with a footer that stays at the bottom of the panel.
    Dock {
        #[serde(default)]
        header: Option<Box<Node>>,
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
        /// Hidden text, with a button that shows the value.
        #[serde(default)]
        secret: bool,
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
        /// Grow with the text from `rows` up to this many, then scroll.
        #[serde(default, rename = "maxRows")]
        max_rows: Option<usize>,
        #[serde(default)]
        placeholder: String,
        /// No border of its own, so it can sit in a [`Node::Composer`].
        #[serde(default)]
        plain: bool,
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
pub(crate) struct ChatTab {
    label: String,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    active: bool,
    #[serde(default)]
    on_select: Option<usize>,
    #[serde(default)]
    on_close: Option<usize>,
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
                gap: None,
                min_height: None,
            }),
            scroll: ScrollHandle::new(),
        }
    }
}

/// A panel scrolled this close to its bottom still follows new content.
const FOLLOW_MARGIN: f32 = 32.0;

/// Whether a panel is at, or within `margin` of, its bottom. GPUI's scroll
/// offset is negative once scrolled down, and `max_offset` is the positive
/// distance from the top to the bottom of the content.
fn is_pinned_to_bottom(offset: Pixels, max_offset: Pixels, margin: Pixels) -> bool {
    max_offset + offset <= margin
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
            Ok(Node::Dock {
                header,
                body,
                footer,
            }) if !self.bar => Some((header.clone(), present(body), present(footer))),
            _ => None,
        };
        let panel = v_flex()
            .id(SharedString::from(format!("js-panel-{}", self.id)))
            .w_full();
        if let Some((header, body, footer)) = dock {
            let framed = frames_composer(&footer);
            let header = header.map(|header| {
                render_node(
                    &header,
                    &handlers,
                    &mut ids,
                    Some(&self.fields),
                    fresh,
                    window,
                    cx,
                )
            });
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
            // Follow new content only while the reader is at the bottom, so a
            // streaming refresh does not yank a reader who scrolled up.
            if fresh
                && is_pinned_to_bottom(
                    self.scroll.offset().y,
                    self.scroll.max_offset().y,
                    px(FOLLOW_MARGIN),
                )
            {
                self.scroll.scroll_to_bottom();
            }
            let mut column = panel.size_full().min_h_0();
            if let Some(header) = header {
                column = column.child(div().flex_none().w_full().child(header));
            }
            return column
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
                        .when(!framed, |bar| bar.border_t_1().border_color(border))
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
        Node::Column {
            children,
            gap,
            min_height,
        } => {
            let mut column = with_stack_gap(v_flex(), *gap);
            if let Some(height) = explicit_px(*min_height, MAX_LENGTH) {
                column = column.min_h(px(height));
            }
            column
                .children(
                    children
                        .iter()
                        .map(|child| render_node(child, handlers, ids, fields, fresh, window, cx)),
                )
                .into_any_element()
        }
        Node::Row {
            children,
            wrap,
            spread,
            gap,
            columns,
            row_height,
            align,
        } => {
            if let Some(columns) = *columns {
                // Equal tracks. `spread` and `wrap` stay on the node so a host
                // that ignores `columns` still lays the same children out as a flex row.
                let center = cell_centers(align.as_deref());
                let height = explicit_px(*row_height, MAX_LENGTH);
                with_stack_gap(
                    div().w_full().grid().grid_cols(grid_track_count(columns)),
                    *gap,
                )
                .children(children.iter().map(|child| {
                    let child = render_node(child, handlers, ids, fields, fresh, window, cx);
                    let mut cell = div().min_w_0().flex().items_center();
                    cell = if center {
                        cell.justify_center()
                    } else {
                        cell.justify_start()
                    };
                    if let Some(height) = height {
                        cell = cell.h(px(height)).min_h(px(height));
                    }
                    cell.child(child)
                }))
                .into_any_element()
            } else {
                with_stack_gap(
                    h_flex()
                        .items_center()
                        .when(*spread, |row| row.w_full().justify_between())
                        .when(*wrap, |row| row.flex_wrap())
                        .when(!*wrap, |row| row.flex_nowrap()),
                    *gap,
                )
                .children(
                    children
                        .iter()
                        .map(|child| render_node(child, handlers, ids, fields, fresh, window, cx)),
                )
                .into_any_element()
            }
        }
        Node::TabBar { tabs, actions } => {
            h_flex()
                .w_full()
                .h(px(36.))
                .items_center()
                .border_b_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .items_center()
                        .overflow_x_scrollbar()
                        .children(tabs.iter().map(|tab| {
                            let active = tab.active;
                            let label_color = if active { theme.foreground } else { muted };
                            h_flex()
                                .id(next_id())
                                .flex_none()
                                .h_full()
                                .items_center()
                                .gap_1()
                                .px_2()
                                .when_some(tab.on_select, |item, handler| {
                                    item.cursor_pointer().on_click(handlers.on_click(handler))
                                })
                                .when_some(tab.icon.as_deref(), |item, name| {
                                    item.child(icon(Some(name)).small().text_color(label_color))
                                })
                                .child(
                                    div()
                                        .max_w(px(180.))
                                        .text_sm()
                                        .truncate()
                                        .text_color(label_color)
                                        .child(tab.label.clone()),
                                )
                                .when_some(tab.on_close, |item, handler| {
                                    let close = handlers.clone();
                                    item.child(
                                        div()
                                            .id(next_id())
                                            .cursor_pointer()
                                            .px_1()
                                            .text_sm()
                                            .text_color(muted)
                                            .child("×")
                                            .on_click(move |_, _, cx| {
                                                cx.stop_propagation();
                                                close.call(handler, Vec::new(), cx);
                                            }),
                                    )
                                })
                        })),
                )
                .child(h_flex().flex_none().items_center().gap_1().px_1().children(
                    actions.iter().map(|action| {
                        render_node(action, handlers, ids, fields, fresh, window, cx)
                    }),
                ))
                .into_any_element()
        }
        Node::Composer { children } => v_flex()
            .w_full()
            .gap_1()
            .px_2()
            .py_2()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .children(
                children
                    .iter()
                    .map(|child| render_node(child, handlers, ids, fields, fresh, window, cx)),
            )
            .into_any_element(),
        Node::Icon {
            name,
            muted: is_muted,
        } => div()
            .flex_none()
            .child(icon(Some(name)).xsmall().text_color(if *is_muted {
                muted
            } else {
                theme.foreground
            }))
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
            muted: is_muted,
            dot,
            dot_tone,
        } => {
            // A label element carries the muted color, so the variant still
            // draws the fill, ring and hover. The dot is a child of the
            // button, which is what keeps it on top of a filled primary button.
            let custom = *dot || (*is_muted && !label.is_empty());
            let mut button = Button::new(next_id())
                .when(
                    label.is_empty() && variant.as_deref() != Some("primary"),
                    |button| button.xsmall(),
                )
                .when(
                    !label.is_empty() || variant.as_deref() == Some("primary"),
                    |button| button.small(),
                )
                .when(
                    label.is_empty() && variant.as_deref() == Some("primary"),
                    |button| button.rounded(px(999.)),
                )
                .map(|button| match variant.as_deref() {
                    Some("primary") => button.primary(),
                    Some("ghost") => button.ghost(),
                    _ => button.outline(),
                })
                .when(!custom && !label.is_empty(), |button| {
                    button.label(label.clone())
                });
            if *dot {
                let accent = theme.accent;
                let on_primary = theme.button_primary_foreground;
                // Reapplied after the variant's fixed 24px height, which would clip the dot.
                button = button.h(px(MARKER_BUTTON)).child(button_marker(
                    label,
                    *is_muted,
                    dot_tone.as_deref(),
                    muted,
                    accent,
                    on_primary,
                ));
            } else if *is_muted && !label.is_empty() {
                button = button.child(muted_button_label(label, muted));
            }
            button
                .when_some(tooltip.clone(), |button, tip| button.tooltip(tip))
                .when_some(icon_name.as_deref(), |button, name| {
                    button.icon(icon(Some(name)))
                })
                .when_some(*on_click, |button, handler| {
                    button.on_click(handlers.on_click(handler))
                })
                .into_any_element()
        }
        Node::List { items, compact } => {
            let hover = theme.list_hover;
            let radius = theme.radius;
            v_flex()
                .children(items.iter().map(|item| {
                    div()
                        .id(next_id())
                        .px_2()
                        .when(*compact, |row| row.py(px(1.)).text_xs())
                        .when(!*compact, |row| row.py_1().text_sm())
                        .rounded(radius)
                        .child(
                            h_flex()
                                .when(*compact, |row| row.gap_1())
                                .when(!*compact, |row| row.gap_2())
                                .when_some(item.icon.as_deref(), |row, name| {
                                    let mark = icon(Some(name)).text_color(muted);
                                    row.child(if *compact {
                                        mark.xsmall()
                                    } else {
                                        mark.small()
                                    })
                                })
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .child(div().truncate().child(item.title.clone()))
                                        .when_some(item.subtitle.clone(), |column, subtitle| {
                                            column.child(
                                                div().text_color(muted).truncate().child(subtitle),
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
        Node::Chip { label, on_close } => h_flex()
            .id(next_id())
            .self_start()
            .flex_none()
            .max_w_full()
            .items_center()
            .gap_1()
            .px_2()
            .py(px(2.))
            .rounded(theme.radius)
            .bg(theme.secondary)
            .text_xs()
            .text_color(theme.secondary_foreground)
            .when_some(*on_close, |chip, handler| {
                chip.child(
                    div()
                        .id(next_id())
                        .cursor_pointer()
                        .text_color(muted)
                        .child("×")
                        .on_click(handlers.on_click(handler)),
                )
            })
            .child(div().min_w_0().truncate().child(label.clone()))
            .into_any_element(),
        Node::Separator => div().h_px().w_full().bg(theme.border).into_any_element(),
        Node::Empty { text } => div()
            .py_4()
            .w_full()
            .text_center()
            .text_sm()
            .text_color(muted)
            .child(text.clone())
            .into_any_element(),
        Node::Dock {
            header,
            body,
            footer,
        } => v_flex()
            .gap_2()
            .when_some(header.as_ref(), |column, header| {
                column.child(render_node(
                    header, handlers, ids, fields, fresh, window, cx,
                ))
            })
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
            secret,
            on_change,
        } => match fields {
            Some(fields) => {
                let key = next_field_key("", ids);
                fields.line(
                    key,
                    value,
                    placeholder,
                    *secret,
                    *on_change,
                    fresh,
                    window,
                    cx,
                )
            }
            None => settings_only(muted),
        },
        Node::TextArea {
            id,
            value,
            rows,
            max_rows,
            placeholder,
            plain,
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
                    *max_rows,
                    *plain,
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

/// A composer draws its own border, so the footer should not add another.
fn frames_composer(node: &Node) -> bool {
    match node {
        Node::Composer { .. } => true,
        Node::Column { children, .. } => children.iter().any(frames_composer),
        // Equal tracks are still a `row`. Only that mode is a new container;
        // a flex row keeps the old result and does not frame the dock.
        Node::Row {
            children, columns, ..
        } if columns.is_some() => children.iter().any(frames_composer),
        _ => false,
    }
}

fn muted_button_label(text: &str, color: Hsla) -> Div {
    div()
        .text_color(color)
        .font_weight(FontWeight::NORMAL)
        .child(text.to_string())
}

fn button_marker(
    text: &str,
    muted_label: bool,
    tone: Option<&str>,
    muted: Hsla,
    accent: Hsla,
    on_primary: Hsla,
) -> Div {
    let dot = match marker_tone(tone) {
        MarkerTone::Accent => accent,
        MarkerTone::PrimaryForeground => on_primary,
    };
    v_flex()
        .items_center()
        .gap(px(MARKER_GAP))
        .child(if muted_label {
            muted_button_label(text, muted)
        } else {
            div().child(text.to_string())
        })
        .child(
            div()
                .size(px(MARKER_DOT))
                .flex_none()
                .rounded_full()
                .bg(dot),
        )
}

fn present(node: &Option<Box<Node>>) -> Node {
    match node {
        Some(node) => (**node).clone(),
        None => Node::Column {
            children: Vec::new(),
            gap: None,
            min_height: None,
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
    /// Whether the field was created as hidden text.
    secret: bool,
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
        secret: bool,
        on_change: Option<usize>,
        fresh: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.ensure_line(&key, value, placeholder, secret, on_change, window, cx);
        self.sync_line(&key, value, fresh, window, cx);
        self.editors
            .borrow()
            .get(&key)
            .and_then(|slot| match &slot.kind {
                FieldKind::Line(state) => Some(state.clone()),
                FieldKind::Area(_) => None,
            })
            .map(|state| {
                let input = Input::new(&state).small().w_full();
                if secret { input.mask_toggle() } else { input }.into_any_element()
            })
            .unwrap_or_else(|| div().into_any_element())
    }

    fn area(
        &self,
        key: String,
        value: &str,
        placeholder: &str,
        rows: usize,
        max_rows: Option<usize>,
        plain: bool,
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
            max_rows,
            on_change,
            on_submit,
            window,
            cx,
        );
        self.sync_area(&key, value, fresh, window, cx);
        let grow = max_rows.is_some_and(|max| max > rows);
        let rows = rows.clamp(1, 8) as f32;
        self.editors
            .borrow()
            .get(&key)
            .and_then(|slot| match &slot.kind {
                FieldKind::Area(state) => Some(state.clone()),
                FieldKind::Line(_) => None,
            })
            .map(|state| {
                let mut area = Textarea::new(&state).w_full().text_sm();
                if !grow {
                    area = area.h(px(rows * 22. + if plain { 4. } else { 12. }));
                }
                if plain {
                    area.appearance(false).bordered(false)
                } else {
                    area
                }
                .into_any_element()
            })
            .unwrap_or_else(|| div().into_any_element())
    }

    fn ensure_line(
        &self,
        key: &str,
        value: &str,
        placeholder: &str,
        secret: bool,
        on_change: Option<usize>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let exists =
            self.editors.borrow().get(key).is_some_and(|slot| {
                matches!(slot.kind, FieldKind::Line(_)) && slot.secret == secret
            });
        if exists {
            self.set_handlers(key, on_change, None);
            return;
        }
        self.editors.borrow_mut().remove(key);
        let state = cx.new(|cx| {
            let mut editor = InputState::new(window, cx).placeholder(placeholder.to_string());
            if secret {
                editor = editor.masked(true);
            }
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
                secret,
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
        max_rows: Option<usize>,
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
            let min_rows = rows.clamp(1, 12);
            let mut editor = TextareaState::new(window, cx).placeholder(placeholder.to_string());
            editor = match max_rows.filter(|max| *max > min_rows) {
                Some(max) => editor.auto_grow(min_rows, max.clamp(min_rows, 12)),
                None => editor.rows(min_rows),
            };
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
                secret: false,
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

#[cfg(test)]
mod tests {
    // No `use super::*` here: view.rs glob-imports gpui, whose `test`
    // attribute would shadow the built-in one.
    use gpui_kit::px;
    use rquickjs::CatchResultExt as _;

    use super::is_pinned_to_bottom;

    #[test]
    fn a_view_at_the_bottom_follows_new_content() {
        assert!(is_pinned_to_bottom(px(-500.), px(500.), px(32.)));
        assert!(is_pinned_to_bottom(px(-480.), px(500.), px(32.)));
        // Nothing to scroll is at the bottom by definition.
        assert!(is_pinned_to_bottom(px(0.), px(0.), px(32.)));
    }

    #[test]
    fn a_view_scrolled_up_keeps_its_place() {
        assert!(!is_pinned_to_bottom(px(-200.), px(500.), px(32.)));
        assert!(!is_pinned_to_bottom(px(0.), px(500.), px(32.)));
    }

    #[test]
    fn layout_decisions_keep_the_old_gap_and_clamp_the_new_one() {
        assert_eq!(super::stack_gap(None), super::StackGap::Legacy);
        assert_eq!(super::stack_gap(Some(f32::NAN)), super::StackGap::Legacy);
        assert_eq!(super::stack_gap(Some(0.)), super::StackGap::Px(0.));
        assert_eq!(super::stack_gap(Some(-4.)), super::StackGap::Px(0.));
        assert_eq!(
            super::stack_gap(Some(9_999.)),
            super::StackGap::Px(super::MAX_GAP)
        );

        assert_eq!(super::grid_track_count(0), 1);
        assert_eq!(super::grid_track_count(7), 7);
        assert_eq!(super::grid_track_count(100), super::MAX_COLUMNS);

        assert_eq!(super::explicit_px(Some(96.), super::MAX_LENGTH), Some(96.));
        assert_eq!(
            super::explicit_px(Some(5_000.), super::MAX_LENGTH),
            Some(super::MAX_LENGTH)
        );
        assert_eq!(super::explicit_px(None, super::MAX_LENGTH), None);

        assert!(super::cell_centers(None));
        assert!(super::cell_centers(Some("center")));
        assert!(!super::cell_centers(Some("start")));

        assert_eq!(super::marker_tone(None), super::MarkerTone::Accent);
        assert_eq!(
            super::marker_tone(Some("accent")),
            super::MarkerTone::Accent
        );
        assert_eq!(super::marker_tone(Some("nope")), super::MarkerTone::Accent);
        assert_eq!(
            super::marker_tone(Some("primaryForeground")),
            super::MarkerTone::PrimaryForeground
        );
    }

    #[test]
    fn a_legacy_tree_still_decodes_without_the_new_fields() {
        let node: super::Node = serde_json::from_str(
            r#"{ "type": "column", "children": [
                { "type": "row", "spread": true, "children": [
                    { "type": "button", "label": "OK", "variant": "outline", "onClick": 0 }
                ] },
                { "type": "text", "text": "Hello" }
            ] }"#,
        )
        .unwrap();
        let super::Node::Column {
            gap,
            min_height,
            children,
        } = node
        else {
            panic!("root is a column");
        };
        assert_eq!((gap, min_height), (None, None));
        let super::Node::Row {
            wrap,
            spread,
            gap,
            columns,
            row_height,
            align,
            children: row,
        } = &children[0]
        else {
            panic!("first child is a row");
        };
        assert_eq!((*wrap, *spread, *gap), (true, true, None));
        assert!(columns.is_none() && row_height.is_none() && align.is_none());
        let super::Node::Button {
            label,
            variant,
            muted,
            dot,
            dot_tone,
            on_click,
            ..
        } = &row[0]
        else {
            panic!("the row holds a button");
        };
        assert_eq!(label, "OK");
        assert_eq!(variant.as_deref(), Some("outline"));
        assert!(!*muted && !*dot && dot_tone.is_none());
        assert_eq!(*on_click, Some(0));
        assert!(matches!(children[1], super::Node::Text { .. }));
        assert!(!super::frames_composer(&super::Node::Column {
            children: Vec::new(),
            gap: None,
            min_height: None,
        }));
    }

    #[test]
    fn a_grid_node_type_stays_unsupported() {
        let node: super::Node =
            serde_json::from_str(r#"{ "type": "grid", "columns": 7, "children": [] }"#).unwrap();
        assert!(matches!(node, super::Node::Unknown));
    }

    /// GALA-24: a missing `type` is not `Unknown`. Serde rejects the tree, which
    /// is the whole-panel "invalid" path, and a bad field on a known node does too.
    #[test]
    fn a_node_without_a_type_or_with_a_bad_field_rejects_the_tree() {
        let missing = serde_json::from_str::<super::Node>(r#"{ "children": [] }"#)
            .err()
            .unwrap()
            .to_string();
        assert!(missing.contains("type"), "{missing}");
        let bad = serde_json::from_str::<super::Node>(
            r#"{ "type": "row", "columns": "seven", "children": [] }"#,
        )
        .err()
        .unwrap()
        .to_string();
        assert!(bad.contains("invalid type"), "{bad}");
    }

    #[test]
    fn a_composer_inside_an_equal_track_row_still_frames_the_dock() {
        let node: super::Node = serde_json::from_str(
            r#"{ "type": "row", "columns": 7, "children": [
                { "type": "composer", "children": [] }
            ] }"#,
        )
        .unwrap();
        assert!(super::frames_composer(&node));
        let flex = super::Node::Row {
            children: vec![super::Node::Composer {
                children: Vec::new(),
            }],
            wrap: true,
            spread: false,
            gap: None,
            columns: None,
            row_height: None,
            align: None,
        };
        assert!(!super::frames_composer(&flex));
        let super::Node::Row {
            columns,
            gap,
            row_height,
            align,
            ..
        } = node
        else {
            panic!("decoded a row");
        };
        assert_eq!(columns, Some(7));
        assert!(gap.is_none() && row_height.is_none() && align.is_none());
    }

    /// Runs the real prelude and renders two panels. This is the contract
    /// `render_node` decodes; it does not paint the panel.
    #[test]
    fn the_plugin_api_emits_the_calendar_contract() {
        let legacy = render_panel("legacy");
        assert!(
            !legacy.contains("minHeight")
                && !legacy.contains("dot")
                && !legacy.contains("muted")
                && !legacy.contains("rowHeight")
                && !legacy.contains("columns")
                && !legacy.contains("grid"),
            "{legacy}"
        );
        let legacy: super::Node = serde_json::from_str(&legacy).unwrap();
        let super::Node::Column {
            gap,
            min_height,
            children,
        } = &legacy
        else {
            panic!("legacy root");
        };
        assert!(gap.is_none() && min_height.is_none());
        let super::Node::Row {
            spread,
            gap,
            columns,
            row_height,
            align,
            ..
        } = &children[0]
        else {
            panic!("legacy row");
        };
        assert!(
            *spread
                && gap.is_none()
                && columns.is_none()
                && row_height.is_none()
                && align.is_none()
        );

        let calendar = render_panel("cal");
        assert!(!calendar.contains("\"type\":\"grid\""), "{calendar}");
        let calendar: super::Node = serde_json::from_str(&calendar).unwrap();
        let super::Node::Column {
            gap,
            min_height,
            children,
        } = calendar
        else {
            panic!("calendar root");
        };
        assert_eq!(gap, Some(12.));
        assert!(min_height.is_none());

        let super::Node::Row {
            spread,
            columns,
            gap,
            row_height,
            align,
            children: weekdays,
            ..
        } = &children[0]
        else {
            panic!("weekday row");
        };
        assert_eq!(
            (*spread, *columns, *gap, *row_height),
            (true, Some(7), Some(0.), None)
        );
        assert!(align.is_none());
        assert_eq!(weekdays.len(), 7);
        assert!(matches!(&weekdays[0], super::Node::Text { text, .. } if text == "Su"));
        assert!(matches!(&weekdays[6], super::Node::Text { text, .. } if text == "Sa"));

        let super::Node::Row {
            spread,
            columns,
            gap,
            row_height,
            children: days,
            ..
        } = &children[1]
        else {
            panic!("date row");
        };
        assert!(*spread);
        assert_eq!(
            (*columns, *gap, *row_height),
            (Some(7), Some(0.), Some(36.))
        );
        assert_eq!(days.len(), 4);
        assert!(matches!(
            &days[0],
            super::Node::Button { label, variant, muted: true, dot: false, .. }
                if label == "1" && variant.as_deref() == Some("ghost")
        ));
        assert!(matches!(
            &days[1],
            super::Node::Button {
                label,
                variant,
                muted: false,
                dot: true,
                dot_tone: Some(tone),
                ..
            } if label == "23" && variant.as_deref() == Some("primary") && tone == "primaryForeground"
        ));
        assert!(matches!(
            &days[2],
            super::Node::Button { label, dot: true, dot_tone: None, .. } if label == "25"
        ));
        assert!(matches!(
            &days[3],
            super::Node::Button { label, muted: false, dot: false, .. } if label == "30"
        ));
        assert!(matches!(children[2], super::Node::Separator));
        let super::Node::Column {
            gap,
            min_height,
            children: day,
        } = &children[3]
        else {
            panic!("selected day");
        };
        assert_eq!((*gap, *min_height), (Some(8.), Some(96.)));
        assert!(matches!(
            &day[1],
            super::Node::Text {
                muted: true,
                small: true,
                ..
            }
        ));

        let super::Node::Row {
            columns,
            gap,
            row_height,
            align,
            ..
        } = &children[4]
        else {
            panic!("clamped row");
        };
        assert_eq!(*columns, Some(1));
        assert_eq!(*gap, Some(0.));
        assert_eq!(*row_height, Some(super::MAX_LENGTH));
        assert_eq!(align.as_deref(), Some("start"));
    }

    fn render_panel(id: &str) -> String {
        let runtime = rquickjs::Runtime::new().unwrap();
        let context = rquickjs::Context::full(&runtime).unwrap();
        context.with(|ctx| {
            let host = rquickjs::Function::new(
                ctx.clone(),
                |_ctx: rquickjs::Ctx<'_>, _method: String, _args: String| "null".to_string(),
            )
            .unwrap();
            ctx.globals().set("__notiaHost", host).unwrap();
            eval(&ctx, include_str!("prelude.js"));
            eval(&ctx, PLUGIN);
            let manifest = r#"{"id":"cal","name":"Cal","main":"main.js"}"#;
            let boot = format!(
                "__notiaStart(undefined, {})",
                serde_json::to_string(manifest).unwrap()
            );
            let promise: rquickjs::Promise = ctx
                .eval(boot)
                .catch(&ctx)
                .unwrap_or_else(|error| panic!("{error}"));
            while ctx.execute_pending_job() {}
            match promise.result::<rquickjs::Value>() {
                Some(Ok(_)) => {}
                Some(Err(error)) => panic!("onload failed: {error}"),
                None => panic!("onload did not finish"),
            }
            let dispatch: rquickjs::Function = ctx.globals().get("__notiaDispatch").unwrap();
            dispatch
                .call(("render", id, ""))
                .catch(&ctx)
                .unwrap_or_else(|error| panic!("{error}"))
        })
    }

    fn eval(ctx: &rquickjs::Ctx, source: &str) {
        ctx.eval::<(), _>(source)
            .catch(ctx)
            .unwrap_or_else(|error| panic!("{error}"));
    }

    const PLUGIN: &str = r#"
const { Plugin, ui } = globalThis[Symbol.for("notia")];
module.exports = class extends Plugin {
  onload() {
    this.addPanel({
      id: "legacy",
      title: "Legacy",
      render: () => ui.column([
        ui.row([ui.button("OK", () => {}, { variant: "outline" })], { spread: true }),
        ui.button("Go", () => {}, { variant: "ghost", icon: "plus" }),
      ]),
    });
    this.addPanel({
      id: "cal",
      title: "Calendar",
      render: () => ui.column([
        ui.row(["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"], { spread: true, columns: 7, gap: 0 }),
        ui.row([
          ui.button("1", () => {}, { variant: "ghost", muted: true }),
          ui.button("23", () => {}, { variant: "primary", dot: true, dotTone: "primaryForeground" }),
          ui.button("25", () => {}, { variant: "ghost", dot: "accent" }),
          ui.button("30", () => {}, { variant: "ghost" }),
        ], { spread: true, columns: 7, gap: 0, rowHeight: 36 }),
        ui.separator(),
        ui.column([
          ui.text("Monday · 2026-11-30", { bold: true }),
          ui.text("No notes on this day", { muted: true, small: true }),
        ], { gap: 8, minHeight: 96 }),
        ui.row([ui.text("edge")], { columns: 0, gap: -8, rowHeight: 5000, align: "start" }),
      ], { gap: 12 }),
    });
  }
};
"#;
}
