//! Prompt suggestions: bookmarks matching what is typed, with a selection
//! that arrow keys move and Enter opens.

use crate::truncate;
use browser_runtime::{Bookmark, Bookmarks};
use gpui::{div, prelude::*, px, Div, Hsla};

/// Rows shown under the prompt.
const MAX_SUGGESTIONS: usize = 5;

/// Bookmarks to suggest for the prompt text. `:command` input suggests nothing.
pub fn suggestions<'a>(bookmarks: &'a Bookmarks, typed: &str) -> Vec<&'a Bookmark> {
    if typed.starts_with(':') {
        return Vec::new();
    }
    bookmarks.search(typed).into_iter().take(MAX_SUGGESTIONS).collect()
}

/// Move the selection by `delta` over `count` rows. Nothing is selected by
/// default, so Enter submits exactly what was typed; up from the first row
/// returns to the typed text.
pub fn move_selection(selected: Option<usize>, delta: i32, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    match (selected, delta.signum()) {
        (None, 1) => Some(0),
        (Some(i), 1) => Some((i + 1).min(count - 1)),
        (Some(0), -1) => None,
        (Some(i), -1) => Some(i.min(count) - 1),
        (current, _) => current,
    }
}

/// The suggestion list under the prompt box, or nothing when empty.
pub fn render(
    items: &[&Bookmark],
    selected: Option<usize>,
    bar_bg: Hsla,
    bar_text: Hsla,
    accent: Hsla,
) -> Option<Div> {
    if items.is_empty() {
        return None;
    }
    let mut list = div()
        .mt_1()
        .rounded_md()
        .bg(bar_bg)
        .border_1()
        .border_color(accent)
        .occlude()
        .overflow_hidden()
        .flex()
        .flex_col();
    for (i, b) in items.iter().enumerate() {
        let title = if b.title.is_empty() { &b.url } else { &b.title };
        list = list.child(
            div()
                .flex()
                .flex_row()
                .justify_between()
                .gap_3()
                .px_3()
                .py_1()
                .text_size(px(12.0))
                .text_color(if selected == Some(i) { accent } else { bar_text })
                .child(truncate(title, 50))
                .child(truncate(&b.url, 50)),
        );
    }
    Some(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_starts_empty_and_clamps() {
        assert_eq!(move_selection(None, 1, 3), Some(0));
        assert_eq!(move_selection(Some(0), 1, 3), Some(1));
        assert_eq!(move_selection(Some(2), 1, 3), Some(2), "clamps at the last row");
        assert_eq!(move_selection(Some(1), -1, 3), Some(0));
        assert_eq!(move_selection(Some(0), -1, 3), None, "up from the first row deselects");
        assert_eq!(move_selection(None, -1, 3), None);
        assert_eq!(move_selection(Some(1), 1, 0), None, "no rows, no selection");
        assert_eq!(move_selection(Some(4), -1, 2), Some(1), "stale selection is pulled in range");
    }

    #[test]
    fn suggestions_match_bookmarks_and_skip_commands() {
        let mut b = Bookmarks::default();
        b.toggle("https://gpui.rs/", "GPUI");
        assert_eq!(suggestions(&b, "gpu").len(), 1);
        assert!(suggestions(&b, ":gpu").is_empty());
        assert!(suggestions(&b, "").is_empty());
        for i in 0..9 {
            b.toggle(&format!("https://gpui.test/{i}"), "gpui mirror");
        }
        assert_eq!(suggestions(&b, "gpui").len(), MAX_SUGGESTIONS);
    }
}
