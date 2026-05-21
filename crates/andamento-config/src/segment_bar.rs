use unicode_width::UnicodeWidthStr;

use crate::{pad_to_width, truncate_to_width};

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
}
