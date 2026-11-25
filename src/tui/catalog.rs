//! The catalog pane: a tree of tables, views, and columns.
//!
//! The tree is stored as the flat list of rows currently visible. Opening a
//! row inserts its children below it; closing removes them. Children are
//! only fetched when a row is opened. (PERF-005)

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::{text, theme};
use crate::backend::{CatalogNode, NodeKind};

struct Row {
    /// Names from the root down to this row. Its length is the row's depth.
    path: Vec<String>,
    node: CatalogNode,
    open: bool,
}

#[derive(Default)]
pub struct Catalog {
    rows: Vec<Row>,
    cursor: usize,
    /// Index of the first row drawn.
    top: usize,
    /// Rows that fit on screen, remembered from the last draw for paging.
    page: usize,
    /// False until the root has arrived from the backend.
    loaded: bool,
}

impl Catalog {
    pub fn half_page(&self) -> usize {
        (self.page / 2).max(1)
    }

    pub fn move_by(&mut self, delta: isize) {
        self.move_to(self.cursor.saturating_add_signed(delta));
    }

    pub fn move_to(&mut self, index: usize) {
        self.cursor = index.min(self.rows.len().saturating_sub(1));
    }

    pub fn move_to_end(&mut self) {
        self.move_to(usize::MAX);
    }

    /// Path of the row under the cursor, if it is a table or view.
    pub fn current_relation(&self) -> Option<Vec<String>> {
        let row = self.rows.get(self.cursor)?;
        matches!(row.node.kind, NodeKind::Table | NodeKind::View).then(|| row.path.clone())
    }

    /// Open the row under the cursor. Returns the path whose children need
    /// fetching, if they are not on screen yet.
    pub fn open(&mut self) -> Option<Vec<String>> {
        let row = self.rows.get(self.cursor)?;
        if !row.node.has_children {
            return None;
        }
        if row.open {
            // Already open: step down onto the first child.
            self.move_by(1);
            return None;
        }
        Some(row.path.clone())
    }

    /// Close the row under the cursor, or jump to its parent if it is
    /// already closed.
    pub fn close(&mut self) {
        let Some(row) = self.rows.get(self.cursor) else { return };
        let depth = row.path.len();

        if row.open {
            let first_child = self.cursor + 1;
            let after_children = self.rows[first_child..]
                .iter()
                .position(|r| r.path.len() <= depth)
                .map_or(self.rows.len(), |n| first_child + n);
            self.rows.drain(first_child..after_children);
            self.rows[self.cursor].open = false;
        } else if depth > 1 {
            let parent = &row.path[..depth - 1];
            if let Some(index) = self.rows[..self.cursor].iter().rposition(|r| r.path == parent) {
                self.cursor = index;
            }
        }
    }

    /// Put freshly fetched children under the row at `parent`.
    /// An empty `parent` means these are the top-level rows.
    pub fn insert_children(&mut self, parent: &[String], nodes: Vec<CatalogNode>) {
        let new_rows = nodes.into_iter().map(|node| {
            let mut path = parent.to_vec();
            path.push(node.name.clone());
            Row {
                path,
                node,
                open: false,
            }
        });

        if parent.is_empty() {
            self.rows = new_rows.collect();
            self.cursor = 0;
            self.loaded = true;
            return;
        }
        // The row may have been closed or removed while the fetch was in
        // flight; if so the children are simply dropped.
        let Some(index) = self.rows.iter().position(|r| r.path == parent) else {
            return;
        };
        if self.rows[index].open {
            return;
        }
        self.rows[index].open = true;
        self.rows.splice(index + 1..index + 1, new_rows);
    }

    pub fn draw(&mut self, area: Rect, buf: &mut Buffer, focused: bool) {
        self.page = area.height as usize;
        if self.page == 0 {
            return;
        }
        if self.rows.is_empty() {
            let note = if self.loaded { "no tables" } else { "loading…" };
            buf.set_stringn(
                area.x,
                area.y,
                note,
                area.width as usize,
                Style::new().fg(theme::OVERLAY),
            );
            return;
        }

        // Scroll just enough to keep the cursor on screen.
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if self.cursor >= self.top + self.page {
            self.top = self.cursor + 1 - self.page;
        }

        let visible = self.rows.iter().enumerate().skip(self.top).take(self.page);
        for (line, (index, row)) in visible.enumerate() {
            let y = area.y + line as u16;
            let line_area = Rect::new(area.x, y, area.width, 1);

            if index == self.cursor {
                let highlight = if focused { theme::SURFACE2 } else { theme::SURFACE0 };
                buf.set_style(line_area, Style::new().bg(highlight));
            }

            let marker = match (row.node.has_children, row.open) {
                (true, true) => "▾ ",
                (true, false) => "▸ ",
                (false, _) => "  ",
            };
            let indent = "  ".repeat(row.path.len() - 1);
            let name_style = match row.node.kind {
                NodeKind::Catalog => Style::new().fg(theme::MAUVE).add_modifier(Modifier::BOLD),
                NodeKind::Schema => Style::new().fg(theme::PEACH),
                NodeKind::Table => Style::new().fg(theme::BLUE).add_modifier(Modifier::BOLD),
                NodeKind::View => Style::new().fg(theme::TEAL).add_modifier(Modifier::BOLD),
                NodeKind::Column => Style::new().fg(theme::TEXT),
            };

            // Each piece is drawn where the previous one ended, clipped to
            // the pane's right edge.
            let name = text::clean(&row.node.name, area.width as usize);
            let detail = text::clean(&row.node.detail, area.width as usize);
            let pieces = [
                (format!("{indent}{marker}"), Style::new().fg(theme::OVERLAY)),
                (name, name_style),
                (format!(" {detail}"), Style::new().fg(theme::OVERLAY)),
            ];
            let mut x = area.x;
            for (piece, style) in pieces {
                let room = area.right().saturating_sub(x) as usize;
                (x, _) = buf.set_stringn(x, y, piece, room, style);
            }
        }
    }
}
