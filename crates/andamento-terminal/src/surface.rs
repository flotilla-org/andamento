//! A terminal consumer of the semantic snapshot, also usable without a plugin.
use andamento_core::{
    presentation::{PlacementNode, SurfaceSnapshot},
    sidebar::Action,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug)]
pub struct Frame {
    pub lines: Vec<String>,
    pub hits: Vec<Hit>,
}

#[derive(Debug)]
pub struct Hit {
    pub row: usize,
    pub action: Action,
}

pub fn render(snapshot: &SurfaceSnapshot, columns: usize) -> Frame {
    fn line(frame: &mut Frame, text: String, columns: usize) {
        let mut width = 0;
        let text: String = text
            .chars()
            .take_while(|c| {
                width += c.width().unwrap_or(0);
                width <= columns
            })
            .collect();
        frame.lines.push(format!(
            "{text}{}",
            " ".repeat(columns.saturating_sub(text.width()))
        ));
    }
    fn nodes(frame: &mut Frame, items: &[PlacementNode], depth: usize, columns: usize) {
        for node in items {
            let text = node.content.text();
            let text = if text.is_empty() { &node.label } else { &text };
            let prefix = if node.children.is_empty() {
                " "
            } else if node.collapsed {
                ">"
            } else {
                "v"
            };
            frame.hits.push(Hit {
                row: frame.lines.len(),
                action: if node.children.is_empty() {
                    Action::Activate {
                        entity: node.entity.clone(),
                    }
                } else {
                    Action::TogglePlacement {
                        key: node.key.clone(),
                    }
                },
            });
            line(
                frame,
                format!("{}{prefix} {text}", "  ".repeat(depth)),
                columns,
            );
            if !node.collapsed {
                nodes(frame, &node.children, depth + 1, columns);
            }
        }
    }
    let mut frame = Frame {
        lines: vec![],
        hits: vec![],
    };
    for section in &snapshot.sections {
        let text = section.content.text();
        line(
            &mut frame,
            if text.is_empty() {
                section.name.clone()
            } else {
                text
            },
            columns,
        );
        nodes(&mut frame, &section.nodes, 0, columns);
        for control in &section.content.controls {
            if let Some(name) = &control.variable {
                frame.hits.push(Hit {
                    row: frame.lines.len(),
                    action: Action::ToggleDisplayVariable { name: name.clone() },
                });
                line(
                    &mut frame,
                    format!("[{name}: {:?}]", snapshot.display_values.get(name)),
                    columns,
                );
            }
        }
    }
    frame
}
