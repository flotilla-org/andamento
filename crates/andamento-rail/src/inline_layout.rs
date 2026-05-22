use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineRun {
    items: Vec<InlineItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineItem {
    pub id: String,
    pub text: String,
    pub measure_text: String,
    pub class: InlineClass,
    pub priority: i64,
    pub min_width: usize,
    pub compact_text: Option<String>,
    pub hit: Option<InlineHit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineClass {
    Required,
    Optional,
    Priority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineHit {
    GroupToggle,
    InspectNode,
    SwitchTab { index: usize },
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
    pub hit: Option<InlineHit>,
}

impl InlineRun {
    pub fn new(items: Vec<InlineItem>) -> Self {
        Self { items }
    }

    pub fn layout(&self, width: usize) -> PlacedInlineRun {
        let selected = select_items_for_width(&self.items, width);
        place_items(&selected, width)
    }
}

impl InlineItem {
    pub fn text(id: impl Into<String>, text: impl Into<String>) -> Self {
        let text = text.into();
        let min_width = text.width().min(3);
        Self {
            id: id.into(),
            measure_text: text.clone(),
            text,
            class: InlineClass::Required,
            priority: 100,
            min_width,
            compact_text: None,
            hit: None,
        }
    }

    pub fn styled_text(
        id: impl Into<String>,
        text: impl Into<String>,
        measure_text: impl Into<String>,
    ) -> Self {
        let measure_text = measure_text.into();
        Self {
            id: id.into(),
            text: text.into(),
            min_width: measure_text.width().min(3),
            measure_text,
            class: InlineClass::Required,
            priority: 100,
            compact_text: None,
            hit: None,
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

    pub fn priority(mut self, priority: i64) -> Self {
        self.class = InlineClass::Priority;
        self.priority = priority;
        self
    }

    pub fn hit(mut self, hit: InlineHit) -> Self {
        self.hit = Some(hit);
        self
    }
}

fn select_items_for_width(items: &[InlineItem], width: usize) -> Vec<InlineItem> {
    if inline_items_width(items) <= width {
        return items.to_vec();
    }

    let mut selected = items.to_vec();
    selected.retain(|item| item.class != InlineClass::Optional);
    if inline_items_width(&selected) <= width {
        return selected;
    }

    truncate_lowest_priority_item_to_fit(&mut selected, width);
    if inline_items_width(&selected) <= width {
        return selected;
    }

    for priority in droppable_priorities(&selected) {
        selected.retain(|item| item.class != InlineClass::Priority || item.priority > priority);
        if inline_items_width(&selected) <= width {
            return selected;
        }
        truncate_lowest_priority_item_to_fit(&mut selected, width);
        if inline_items_width(&selected) <= width {
            return selected;
        }
    }

    truncate_lowest_priority_item_to_fit(&mut selected, width);
    selected
}

fn place_items(items: &[InlineItem], width: usize) -> PlacedInlineRun {
    let mut text = String::new();
    let mut visible_width = 0usize;
    let mut placed_items = Vec::new();
    for item in items {
        let separator = inline_separator(&text, &item.measure_text);
        let separator_width = separator.width();
        let item_width = item.measure_text.width();
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
            hit: item.hit,
        });
    }
    PlacedInlineRun {
        text,
        visible_width: visible_width.min(width),
        items: placed_items,
    }
}

fn inline_items_width(items: &[InlineItem]) -> usize {
    let mut text = String::new();
    let mut width = 0usize;
    for item in items {
        let separator = inline_separator(&text, &item.measure_text);
        width += separator.width() + item.measure_text.width();
        text.push_str(separator);
        text.push_str(&item.measure_text);
    }
    width
}

fn truncate_lowest_priority_item_to_fit(items: &mut [InlineItem], width: usize) {
    let Some((index, _)) = items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.measure_text.width() > item.min_width)
        .min_by_key(|(_, item)| item.priority)
    else {
        return;
    };
    let current_width = inline_items_width(items);
    if current_width <= width {
        return;
    }
    let item_width = items[index].measure_text.width();
    let overflow = current_width - width;
    let max_reduction = item_width.saturating_sub(items[index].min_width);
    let new_width = item_width.saturating_sub(overflow.min(max_reduction));
    let truncated = truncate_to_width(&items[index].measure_text, new_width);
    items[index].measure_text = truncated.clone();
    items[index].text = truncated;
}

fn droppable_priorities(items: &[InlineItem]) -> Vec<i64> {
    let mut priorities = items
        .iter()
        .filter(|item| item.class == InlineClass::Priority)
        .map(|item| item.priority)
        .collect::<Vec<_>>();
    priorities.sort_unstable();
    priorities.dedup();
    priorities
}

fn truncate_to_width(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_owned();
    }
    let mut output = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let char_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + char_width >= width {
            break;
        }
        output.push(ch);
        used += char_width;
    }
    output.push('…');
    output
}

fn inline_separator(current_text: &str, next_text: &str) -> &'static str {
    if current_text.is_empty() || next_text.starts_with(':') {
        ""
    } else {
        " "
    }
}

#[cfg(test)]
fn visible_width_without_ansi(line: &str) -> usize {
    let mut visible = String::new();
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if code == 'm' {
                    break;
                }
            }
            continue;
        }
        visible.push(ch);
    }
    visible.width()
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

    #[test]
    fn inline_run_drops_optional_items_before_priority_items() {
        let run = InlineRun::new(vec![
            InlineItem::text("toggle", "▼").required(),
            InlineItem::text("label", "project-a").required(),
            InlineItem::text("count", "(2)").optional(),
            InlineItem::text("active", ": agent-1").priority(80),
        ]);

        let placed = run.layout(20);

        assert_eq!(placed.text, "▼ project-a: agent-1");
        assert!(!placed.items.iter().any(|item| item.id == "count"));
        assert!(placed.items.iter().any(|item| item.id == "active"));
    }

    #[test]
    fn inline_run_truncates_lowest_priority_item_before_dropping_it() {
        let run = InlineRun::new(vec![
            InlineItem::text("toggle", "▼").required(),
            InlineItem::text("repo", "flotilla-org/flotilla").priority(100),
            InlineItem::text("branch", "feat/very-long-branch-name").priority(60),
        ]);

        let placed = run.layout(28);

        assert!(placed.text.starts_with("▼ flotilla-org/flotilla "));
        assert!(placed.text.ends_with("…"));
        assert!(placed.items.iter().any(|item| item.id == "branch"));
        assert!(placed.visible_width <= 28);
    }

    #[test]
    fn inline_run_keeps_hit_payloads_for_visible_items() {
        let run = InlineRun::new(vec![
            InlineItem::text("toggle", "▼")
                .required()
                .hit(InlineHit::GroupToggle),
            InlineItem::text("inspect", "◐")
                .required()
                .hit(InlineHit::InspectNode),
        ]);

        let placed = run.layout(8);

        assert_eq!(placed.items[0].hit, Some(InlineHit::GroupToggle));
        assert_eq!(placed.items[0].cols, 0..1);
        assert_eq!(placed.items[1].hit, Some(InlineHit::InspectNode));
    }

    #[test]
    fn inline_run_counts_visible_width_without_ansi() {
        let styled = "\u{1b}[1;38;5;2mproject\u{1b}[0m";
        let run = InlineRun::new(vec![
            InlineItem::styled_text("label", styled, "project").required()
        ]);

        let placed = run.layout(7);

        assert_eq!(placed.visible_width, 7);
        assert_eq!(visible_width_without_ansi(&placed.text), 7);
    }
}
