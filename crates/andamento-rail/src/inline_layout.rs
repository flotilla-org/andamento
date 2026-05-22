use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineRun {
    items: Vec<InlineItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineItem {
    pub id: String,
    pub text: String,
    pub class: InlineClass,
    pub priority: i64,
    pub min_width: usize,
    pub compact_text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineClass {
    Required,
    Optional,
    Priority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedInlineRun {
    pub text: String,
    pub visible_width: usize,
    pub items: Vec<PlacedInlineItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedInlineItem {
    pub id: String,
    pub cols: std::ops::Range<usize>,
}

impl InlineRun {
    pub fn new(items: Vec<InlineItem>) -> Self {
        Self { items }
    }

    pub fn layout(&self, width: usize) -> PlacedInlineRun {
        let mut text = String::new();
        let mut visible_width = 0usize;
        let mut placed_items = Vec::new();
        for item in &self.items {
            let separator = inline_separator(&text, &item.text);
            let separator_width = separator.width();
            let item_width = item.text.width();
            if visible_width + separator_width + item_width > width {
                break;
            }
            text.push_str(separator);
            visible_width += separator_width;
            let start = visible_width;
            text.push_str(&item.text);
            visible_width += item_width;
            placed_items.push(PlacedInlineItem {
                id: item.id.clone(),
                cols: start..visible_width,
            });
        }
        PlacedInlineRun {
            text,
            visible_width,
            items: placed_items,
        }
    }
}

impl InlineItem {
    pub fn text(id: impl Into<String>, text: impl Into<String>) -> Self {
        let text = text.into();
        let min_width = text.width().min(3);
        Self {
            id: id.into(),
            text,
            class: InlineClass::Required,
            priority: 100,
            min_width,
            compact_text: None,
        }
    }

    pub fn required(mut self) -> Self {
        self.class = InlineClass::Required;
        self.priority = 100;
        self
    }

    pub fn optional(mut self) -> Self {
        self.class = InlineClass::Optional;
        self.priority = 0;
        self
    }
}

fn inline_separator(current_text: &str, next_text: &str) -> &'static str {
    if current_text.is_empty() || next_text.starts_with(':') {
        ""
    } else {
        " "
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_run_keeps_full_text_when_it_fits() {
        let run = InlineRun::new(vec![
            InlineItem::text("toggle", "▼").required(),
            InlineItem::text("label", "project-a").required(),
            InlineItem::text("count", "(2)").optional(),
        ]);

        let placed = run.layout(20);

        assert_eq!(placed.text, "▼ project-a (2)");
        assert_eq!(placed.visible_width, 15);
        assert_eq!(placed.items.len(), 3);
        assert_eq!(placed.items[0].id, "toggle");
        assert_eq!(placed.items[0].cols, 0..1);
    }
}
