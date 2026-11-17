use gpui_kit::*;
use notia_plugin::{Notia, Plugin, PluginContext, PluginManifest, StatusItem, core::parse_note};

pub struct WordCountPlugin;

impl Plugin for WordCountPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new("word-count", "Word count").with_description(
            "Shows the word and character count of the active note in the status bar.",
        )
    }

    fn activate(&self, ctx: &mut PluginContext, _: &mut App) {
        ctx.add_status_item(
            StatusItem::new("count", |_, cx| cx.new(WordCount::new)).with_order(10),
        );
    }
}

pub struct WordCount {
    counts: Option<(usize, usize)>,
    _subscription: Subscription,
}

impl WordCount {
    fn new(cx: &mut Context<Self>) -> Self {
        let workspace = Notia::workspace(cx);
        let subscription = cx.observe(&workspace, |this, workspace, cx| {
            let counts = Self::count(workspace.read(cx));
            if counts != this.counts {
                this.counts = counts;
                cx.notify();
            }
        });
        Self {
            counts: Self::count(workspace.read(cx)),
            _subscription: subscription,
        }
    }

    fn count(workspace: &notia_plugin::WorkspaceModel) -> Option<(usize, usize)> {
        let note = workspace.active_note()?;
        // A canvas or kanban is a board. Counting its JSON would fill the
        // status bar with the words of the file format.
        if !note.path().is_markdown() {
            return None;
        }
        let meta = parse_note(note.text());
        Some((meta.word_count, meta.char_count))
    }
}

impl Render for WordCount {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().children(self.counts.map(|(words, chars)| {
            format!(
                "{words} {}   {chars} {}",
                if words == 1 { "word" } else { "words" },
                if chars == 1 {
                    "character"
                } else {
                    "characters"
                }
            )
        }))
    }
}
