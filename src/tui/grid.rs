//! The result pane: rows and columns with a cell cursor.
//!
//! Only the cells on screen are formatted each frame, so drawing costs the
//! same for ten rows or ten thousand.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::{text, theme};
use crate::backend::{ResultSet, Value};

/// Widest a column is ever drawn. Longer values are cut with `…`.
const MAX_COLUMN_WIDTH: usize = 40;
/// Blank columns between two grid columns.
const GAP: usize = 2;

#[derive(Default)]
pub struct Grid {
    result: Option<ResultSet>,
    elapsed: Duration,
    /// Display width of each column, worked out once when a result arrives.
    widths: Vec<usize>,
    row: usize,
    col: usize,
    /// First row and first column drawn.
    top: usize,
    left: usize,
    /// Rows that fit on screen, remembered from the last draw for paging.
    page: usize,
}

impl Grid {
    pub fn set_result(&mut self, result: ResultSet, elapsed: Duration) {
        let widths = result
            .columns
            .iter()
            .enumerate()
            .map(|(col, name)| {
                let widest_cell = result.rows.iter().map(|row| text::width(&cell(&row[col]).0)).max();
                text::width(name)
                    .max(widest_cell.unwrap_or(0))
                    .clamp(1, MAX_COLUMN_WIDTH)
            })
            .collect();
        // A new result starts with the cursor and scroll back at the top left.
        *self = Grid {
            result: Some(result),
            elapsed,
            widths,
            page: self.page,
            ..Grid::default()
        };
    }

    fn row_count(&self) -> usize {
        self.result.as_ref().map_or(0, |r| r.rows.len())
    }

    pub fn half_page(&self) -> usize {
        (self.page / 2).max(1)
    }

    pub fn move_rows(&mut self, delta: isize) {
        self.move_to_row(self.row.saturating_add_signed(delta));
    }

    pub fn move_to_row(&mut self, index: usize) {
        self.row = index.min(self.row_count().saturating_sub(1));
    }

    pub fn move_cols(&mut self, delta: isize) {
        self.move_to_col(self.col.saturating_add_signed(delta));
    }

    pub fn move_to_col(&mut self, index: usize) {
        self.col = index.min(self.widths.len().saturating_sub(1));
    }

    /// One-line summary for the status bar, and whether the result was cut
    /// off by the row cap. Truncation must never go unnoticed. (GRID-004)
    pub fn summary(&self) -> Option<(String, bool)> {
        let result = self.result.as_ref()?;
        let position = if result.rows.is_empty() {
            "0 rows".to_string()
        } else {
            format!("row {}/{}", self.row + 1, result.rows.len())
        };
        let summary = format!(
            "{position} · col {}/{} · {} ms",
            self.col + 1,
            result.columns.len(),
            self.elapsed.as_millis()
        );
        Some((summary, result.truncated))
    }

    pub fn draw(&mut self, area: Rect, buf: &mut Buffer, focused: bool) {
        // The header takes the first line.
        self.page = (area.height as usize).saturating_sub(1);
        let Some(result) = &self.result else {
            let hint = "Enter on a table to preview it, or : to run a query";
            buf.set_stringn(
                area.x,
                area.y,
                hint,
                area.width as usize,
                Style::new().fg(theme::OVERLAY),
            );
            return;
        };
        if area.height == 0 || area.width == 0 {
            return;
        }

        // Scroll just enough to keep the cursor cell on screen.
        if self.row < self.top {
            self.top = self.row;
        } else if self.page > 0 && self.row >= self.top + self.page {
            self.top = self.row + 1 - self.page;
        }
        self.left = self.left.min(self.col);
        while self.left < self.col && span(&self.widths[self.left..=self.col]) > area.width as usize {
            self.left += 1;
        }

        let header_style = Style::new().fg(theme::MAUVE).add_modifier(Modifier::BOLD);
        let cursor_line = area.y + 1 + (self.row - self.top) as u16;
        if !result.rows.is_empty() && cursor_line < area.bottom() {
            let line = Rect::new(area.x, cursor_line, area.width, 1);
            buf.set_style(line, Style::new().bg(theme::SURFACE0));
        }

        let mut x = area.x;
        for col in self.left..self.widths.len() {
            let room = area.right().saturating_sub(x) as usize;
            if room == 0 {
                break;
            }
            let width = self.widths[col].min(room);

            let name = text::fit(&text::clean(&result.columns[col], MAX_COLUMN_WIDTH + 1), width);
            buf.set_stringn(x, area.y, name, width, header_style);

            let visible = result.rows.iter().enumerate().skip(self.top).take(self.page);
            for (line, (row_index, row)) in visible.enumerate() {
                let y = area.y + 1 + line as u16;
                let (content, align_right, mut style) = cell(&row[col]);
                let content = text::fit(&content, width);

                if row_index == self.row && col == self.col {
                    let highlight = if focused { theme::SURFACE2 } else { theme::SURFACE0 };
                    style = style.bg(highlight).add_modifier(Modifier::BOLD);
                    buf.set_style(Rect::new(x, y, width as u16, 1), style);
                }
                let pad = if align_right {
                    width.saturating_sub(text::width(&content))
                } else {
                    0
                };
                buf.set_stringn(x + pad as u16, y, content, width - pad, style);
            }
            x += (width + GAP) as u16;
        }
    }
}

/// Total screen width of a run of columns, including the gaps between them.
fn span(widths: &[usize]) -> usize {
    widths.iter().sum::<usize>() + GAP * widths.len().saturating_sub(1)
}

/// How one value is shown: its text, whether it is right-aligned, its style.
fn cell(value: &Value) -> (String, bool, Style) {
    let dim = Style::new().fg(theme::OVERLAY).add_modifier(Modifier::ITALIC);
    let number = Style::new().fg(theme::PEACH);
    match value {
        Value::Null => ("NULL".into(), false, dim),
        Value::Integer(n) => (n.to_string(), true, number),
        Value::Real(x) => (format!("{x:?}"), true, number),
        Value::Decimal(digits) => (text::clean(digits, MAX_COLUMN_WIDTH + 1), true, number),
        Value::Text(s) => (
            text::clean(s, MAX_COLUMN_WIDTH + 1),
            false,
            Style::new().fg(theme::TEXT),
        ),
        Value::Blob(bytes) => (format!("<blob {bytes} bytes>"), false, dim),
    }
}
