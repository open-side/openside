//! Aligned tables: a dim header over a hairline rule, two-space gutters, no vertical lines.
//! Flexible columns shrink (with an ellipsis) until the table fits the terminal.

use std::borrow::Cow;

use super::{
    text::{truncate_spans, width, wrap_spans},
    Glyph, Role, Span, Ui,
};

pub(crate) struct Column {
    header: Cow<'static, str>,
    /// Narrowest width the column may shrink to; `None` keeps it whole.
    min: Option<usize>,
}

impl Column {
    pub(crate) fn new(header: impl Into<Cow<'static, str>>) -> Self {
        Self {
            header: header.into(),
            min: None,
        }
    }

    /// May be truncated down to `min` columns when the terminal is narrow.
    pub(crate) fn flex(mut self, min: usize) -> Self {
        self.min = Some(min);
        self
    }
}

/// One table cell: styled spans on a single line.
#[derive(Clone, Debug, Default)]
pub(crate) struct Cell(pub(crate) Vec<Span>);

impl Cell {
    pub(crate) fn text(text: impl Into<String>) -> Self {
        Self(vec![Span::plain(text)])
    }

    pub(crate) fn role(ui: &Ui, role: Role, text: impl Into<String>) -> Self {
        Self(vec![Span::role(ui, role, text)])
    }

    pub(crate) fn spans(spans: Vec<Span>) -> Self {
        Self(spans)
    }

    pub(crate) fn empty() -> Self {
        Self(Vec::new())
    }

    fn width(&self) -> usize {
        self.0.iter().map(|span| width(&span.text)).sum()
    }
}

pub(crate) struct Table {
    columns: Vec<Column>,
    rows: Vec<Vec<Cell>>,
}

const INDENT: usize = 2;
const GUTTER: usize = 2;

impl Table {
    pub(crate) fn new(columns: Vec<Column>) -> Self {
        Self {
            columns,
            rows: Vec::new(),
        }
    }

    pub(crate) fn row(&mut self, cells: Vec<Cell>) {
        debug_assert_eq!(cells.len(), self.columns.len());
        self.rows.push(cells);
    }

    fn widths(&self, available: usize) -> Vec<usize> {
        let mut widths: Vec<usize> = self
            .columns
            .iter()
            .enumerate()
            .map(|(index, column)| {
                self.rows
                    .iter()
                    .map(|row| row[index].width())
                    .chain(std::iter::once(width(&column.header)))
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let gutters = GUTTER * self.columns.len().saturating_sub(1);
        let mut total = INDENT + widths.iter().sum::<usize>() + gutters;
        while total > available {
            // Shrink the widest column that still has room to give.
            let candidate = self
                .columns
                .iter()
                .enumerate()
                .filter(|(index, column)| column.min.is_some_and(|min| widths[*index] > min))
                .max_by_key(|(index, _)| widths[*index]);
            let Some((index, _)) = candidate else { break };
            widths[index] -= 1;
            total -= 1;
        }
        widths
    }

    /// The table as lines of text, each ending in a newline.
    pub(crate) fn render(&self, ui: &Ui) -> String {
        let widths = self.widths(ui.width);
        let ellipsis = ui.glyph(Glyph::Ellipsis);
        let last = self.columns.len().saturating_sub(1);
        let mut out = String::new();
        let line = |cells: Vec<(Vec<Span>, usize)>| -> String {
            let mut line = " ".repeat(INDENT);
            for (index, (spans, column_width)) in cells.into_iter().enumerate() {
                let used: usize = spans.iter().map(|span| width(&span.text)).sum();
                line.push_str(&ui.render(&spans));
                // The last column never pads: no trailing spaces to copy.
                if index != last {
                    line.push_str(&" ".repeat(column_width.saturating_sub(used) + GUTTER));
                }
            }
            // An empty trailing cell leaves only padding behind.
            line.truncate(line.trim_end_matches(' ').len());
            line.push('\n');
            line
        };
        let header: Vec<_> = self
            .columns
            .iter()
            .zip(&widths)
            .map(|(column, column_width)| {
                (
                    vec![Span::role(ui, Role::Dim, column.header.as_ref())],
                    *column_width,
                )
            })
            .collect();
        out.push_str(&line(header));
        let rule_width = widths.iter().sum::<usize>() + GUTTER * last;
        out.push_str(&" ".repeat(INDENT));
        out.push_str(&ui.rule(rule_width));
        out.push('\n');
        for row in &self.rows {
            let cells = row
                .iter()
                .zip(&widths)
                .map(|(cell, column_width)| {
                    (
                        truncate_spans(&cell.0, *column_width, ellipsis),
                        *column_width,
                    )
                })
                .collect();
            out.push_str(&line(cells));
        }
        out
    }

    pub(crate) fn render_wrapped(&self, ui: &Ui) -> String {
        if self.columns.is_empty() {
            return String::new();
        }
        let widths = self.widths(ui.width);
        let mut out = String::new();
        let mut append = |cells: Vec<Vec<Vec<Span>>>| {
            let height = cells.iter().map(Vec::len).max().unwrap_or(0);
            for line in 0..height {
                out.push_str(&" ".repeat(INDENT));
                for (index, (cell, columns)) in cells.iter().zip(&widths).enumerate() {
                    let used = if let Some(spans) = cell.get(line) {
                        out.push_str(&ui.render(spans));
                        spans.iter().map(|span| width(&span.text)).sum()
                    } else {
                        0
                    };
                    if index + 1 != cells.len() {
                        out.push_str(&" ".repeat(columns.saturating_sub(used) + GUTTER));
                    }
                }
                out.truncate(out.trim_end_matches(' ').len());
                out.push('\n');
            }
        };
        append(
            self.columns
                .iter()
                .zip(&widths)
                .map(|(column, columns)| {
                    wrap_spans(
                        &[Span::role(ui, Role::Dim, column.header.as_ref())],
                        *columns,
                        *columns,
                    )
                })
                .collect(),
        );
        for row in &self.rows {
            append(
                row.iter()
                    .zip(&widths)
                    .map(|(cell, columns)| wrap_spans(&cell.0, *columns, *columns))
                    .collect(),
            );
        }
        out
    }
}
