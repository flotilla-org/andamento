use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentItem {
    pub label: String,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedSegment {
    parts: Vec<String>,
    width: usize,
}

impl RenderedSegment {
    pub fn from_parts(parts: Vec<String>) -> Self {
        let width = parts.iter().map(|part| part.width()).sum();
        Self { parts, width }
    }

    fn text(&self) -> String {
        self.parts.concat()
    }

    fn width(&self) -> usize {
        self.width
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentHit {
    pub index: usize,
    pub col_start: usize,
    pub col_end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentHitBox {
    pub index: usize,
    pub row: usize,
    pub col_start: usize,
    pub col_end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedSegmentRun {
    pub lines: Vec<String>,
    pub hits: Vec<SegmentHitBox>,
    pub overflowed: bool,
    pub final_line_remaining_width: usize,
}

pub trait SegmentBarStyle {
    fn render_item(&self, item: &SegmentItem) -> RenderedSegment;
    fn separator(&self) -> RenderedSegment;
}

pub fn render(
    items: &[SegmentItem],
    style: &dyn SegmentBarStyle,
    width: usize,
) -> (String, Vec<SegmentHit>) {
    let mut line = String::new();
    let mut hits = vec![];
    let mut col = 0usize;

    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            append_segment(&mut line, &mut col, &style.separator(), width);
            if col >= width {
                break;
            }
        }

        let rendered = style.render_item(item);
        let item_start = col;
        let (rendered_any, item_end) = append_segment(&mut line, &mut col, &rendered, width);
        if rendered_any && item_end > item_start {
            hits.push(SegmentHit {
                index,
                col_start: item_start,
                col_end: item_end.saturating_sub(1),
            });
        }
        if col >= width {
            break;
        }
    }

    (pad_to_width(&line, width), hits)
}

pub fn render_wrapped(
    items: &[SegmentItem],
    style: &dyn SegmentBarStyle,
    width: usize,
) -> RenderedSegmentRun {
    if width == 0 || items.is_empty() {
        return RenderedSegmentRun {
            lines: vec![],
            hits: vec![],
            overflowed: false,
            final_line_remaining_width: width,
        };
    }

    let mut lines = vec![];
    let mut hits = vec![];
    let mut line = String::new();
    let mut col = 0usize;
    let mut row = 0usize;
    let mut overflowed = false;

    for (index, item) in items.iter().enumerate() {
        let rendered = style.render_item(item);
        let separator = if line.is_empty() {
            None
        } else {
            Some(style.separator())
        };
        let needed_width =
            separator.as_ref().map(RenderedSegment::width).unwrap_or(0) + rendered.width();

        if !line.is_empty() && needed_width > width.saturating_sub(col) {
            lines.push(pad_to_width(&line, width));
            line.clear();
            col = 0;
            row = row.saturating_add(1);
        }

        if let Some(separator) = separator.filter(|_| !line.is_empty()) {
            let _ = append_segment(&mut line, &mut col, &separator, width);
        }

        let item_start = col;
        let (rendered_any, item_end) = append_segment(&mut line, &mut col, &rendered, width);
        if rendered_any && item_end > item_start {
            hits.push(SegmentHitBox {
                index,
                row,
                col_start: item_start,
                col_end: item_end.saturating_sub(1),
            });
        }
        overflowed |= rendered.width() > item_end.saturating_sub(item_start);
    }

    if !line.is_empty() {
        let line_width = line.width();
        lines.push(pad_to_width(&line, width));
        RenderedSegmentRun {
            lines,
            hits,
            overflowed,
            final_line_remaining_width: width.saturating_sub(line_width),
        }
    } else {
        RenderedSegmentRun {
            lines,
            hits,
            overflowed,
            final_line_remaining_width: width,
        }
    }
}

fn append_segment(
    line: &mut String,
    col: &mut usize,
    segment: &RenderedSegment,
    width: usize,
) -> (bool, usize) {
    if *col >= width || segment.width() == 0 {
        return (false, *col);
    }
    let visible = truncate_to_width(&segment.text(), width.saturating_sub(*col));
    let visible_width = visible.width();
    if visible_width == 0 {
        return (false, *col);
    }
    line.push_str(&visible);
    *col = col.saturating_add(visible_width);
    (true, *col)
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ZellijRibbonStyle;

impl SegmentBarStyle for ZellijRibbonStyle {
    fn render_item(&self, item: &SegmentItem) -> RenderedSegment {
        let _active = item.active;
        RenderedSegment::from_parts(vec![
            "".to_owned(),
            format!(" {} ", item.label),
            "".to_owned(),
        ])
    }

    fn separator(&self) -> RenderedSegment {
        RenderedSegment::from_parts(vec![])
    }
}

fn truncate_to_width(text: &str, max_width: usize) -> String {
    if text.width() <= max_width {
        return text.to_owned();
    }
    text.chars().take(max_width).collect()
}

fn pad_to_width(text: &str, width: usize) -> String {
    let text_width = text.width();
    if text_width >= width {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len() + (width - text_width));
    out.push_str(text);
    out.push_str(&" ".repeat(width - text_width));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_returns_hits_from_rendered_segments() {
        let items = vec![
            SegmentItem {
                label: "settings".to_owned(),
                active: false,
            },
            SegmentItem {
                label: "inspect".to_owned(),
                active: true,
            },
        ];

        let (line, hits) = render(&items, &ZellijRibbonStyle, 32);

        assert_eq!(line.trim_end(), " settings  inspect ");
        assert_eq!(
            hits,
            vec![
                SegmentHit {
                    index: 0,
                    col_start: 0,
                    col_end: 11
                },
                SegmentHit {
                    index: 1,
                    col_start: 12,
                    col_end: 22
                }
            ]
        );
    }

    #[test]
    fn render_truncates_last_segment_and_keeps_hit_to_visible_columns() {
        let items = vec![SegmentItem {
            label: "templates".to_owned(),
            active: true,
        }];

        let (line, hits) = render(&items, &ZellijRibbonStyle, 6);

        assert_eq!(line, " temp");
        assert_eq!(
            hits,
            vec![SegmentHit {
                index: 0,
                col_start: 0,
                col_end: 5
            }]
        );
    }

    #[test]
    fn render_wrapped_keeps_segments_inside_bounded_width() {
        let items = vec![
            SegmentItem {
                label: "main".to_owned(),
                active: false,
            },
            SegmentItem {
                label: "feat/rail".to_owned(),
                active: true,
            },
            SegmentItem {
                label: "docs".to_owned(),
                active: false,
            },
        ];

        let rendered = render_wrapped(&items, &ZellijRibbonStyle, 20);

        assert_eq!(rendered.lines[0].trim_end(), " main ");
        assert_eq!(rendered.lines[1].trim_end(), " feat/rail ");
        assert_eq!(rendered.lines[2].trim_end(), " docs ");
        assert_eq!(
            rendered.hits,
            vec![
                SegmentHitBox {
                    index: 0,
                    row: 0,
                    col_start: 0,
                    col_end: 7
                },
                SegmentHitBox {
                    index: 1,
                    row: 1,
                    col_start: 0,
                    col_end: 12
                },
                SegmentHitBox {
                    index: 2,
                    row: 2,
                    col_start: 0,
                    col_end: 7
                }
            ]
        );
        assert_eq!(rendered.final_line_remaining_width, 12);
    }
}
