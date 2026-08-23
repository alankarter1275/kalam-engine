//! Table structure extraction: styled DOM → rows of cells, driven by
//! computed `display` types (so CSS-retargeted elements work, not just
//! `<table>` markup).
//!
//! v1 scope, chosen for book content: colspan and rowspan honored (grid
//! positions assigned with the HTML occupancy algorithm; `rowspan="0"`
//! spans to the last row; spans ignore row-group boundaries since rows are
//! flattened); `vertical-align` supports top/middle/text-bottom; captions
//! lay out as a block above the table; header rows (thead or all-`<th>`)
//! repeat at the top of continuation pages; nested tables flatten into
//! their cell's text. Cell content is flattened to one inline formatting
//! context — block children separate with hard line breaks — which matches
//! how data cells in books are actually written.

use style::properties::ComputedValues;
use style::servo_arc::Arc as ServoArc;
use style::values::specified::box_::DisplayInside;

use chapbook_dom::{NodeData, NodeId};

use crate::boxtree::{append_collapsed_pub, display_of, BoxTreeInput, DisplayClass, InlineContent};

pub struct TableBox {
    /// `<caption>`, laid out as an ordinary block above the grid.
    pub caption: Option<InlineContent>,
    pub caption_style: Option<ServoArc<ComputedValues>>,
    pub rows: Vec<TableRow>,
    /// Grid column count (after colspan expansion).
    pub columns: usize,
}

pub struct TableRow {
    pub cells: Vec<TableCell>,
    /// From a `table-header-group` (or every cell is a `<th>`): repeated at
    /// the top of continuation pages when the table breaks.
    pub is_header: bool,
}

pub struct TableCell {
    pub node: NodeId,
    pub style: ServoArc<ComputedValues>,
    pub colspan: usize,
    /// Rows this cell spans, already clamped to the rows that exist below
    /// it (`rowspan="0"` resolves to "all remaining").
    pub rowspan: usize,
    /// Resolved grid column of the cell's left edge — cells do not simply
    /// pack left to right once rowspans occupy slots from earlier rows.
    pub col: usize,
    pub content: InlineContent,
}

fn display_inside(style: &ComputedValues) -> DisplayInside {
    style.get_box().display.inside()
}

/// Build a table from an element whose computed display-inside is `Table`.
/// Returns `None` for tables with no cells (degrade to nothing).
pub fn build_table(input: &BoxTreeInput, node: NodeId) -> Option<TableBox> {
    let doc = input.doc;
    let mut table = TableBox {
        caption: None,
        caption_style: None,
        rows: Vec::new(),
        columns: 0,
    };

    // Walk children: caption, rows directly, or rows inside row groups.
    // Document order is kept (browsers reorder thead/tfoot; books rarely
    // rely on it and source order reads correctly).
    let walk_rows = |group: NodeId, table: &mut TableBox, header_group: bool| {
        for child in &doc.node(group).children {
            let NodeData::Element(_) = &doc.node(*child).data else {
                continue;
            };
            let Some(style) = doc.primary_styles(*child) else {
                continue;
            };
            if display_inside(&style) == DisplayInside::TableRow {
                if let Some(mut row) = build_row(input, *child) {
                    row.is_header |= header_group;
                    table.rows.push(row);
                }
            }
        }
    };

    for child in &doc.node(node).children {
        let NodeData::Element(el) = &doc.node(*child).data else {
            continue;
        };
        let Some(style) = doc.primary_styles(*child) else {
            continue;
        };
        match display_inside(&style) {
            DisplayInside::TableRow => {
                if let Some(row) = build_row(input, *child) {
                    table.rows.push(row);
                }
            }
            DisplayInside::TableHeaderGroup => walk_rows(*child, &mut table, true),
            DisplayInside::TableRowGroup | DisplayInside::TableFooterGroup => {
                walk_rows(*child, &mut table, false)
            }
            _ if *el.local_name() == markup5ever::local_name!("caption") => {
                let mut content = InlineContent::default();
                collect_flattened(input, *child, &style, &mut content);
                if input.frag.get(child).is_some_and(|f| f.hyphens_auto) {
                    crate::hyphenate::apply(&mut content);
                }
                if !content.runs.is_empty() {
                    table.caption = Some(content);
                    table.caption_style = Some(style);
                }
            }
            _ => {}
        }
    }

    assign_grid_positions(&mut table);
    (table.columns > 0).then_some(table)
}

/// Resolve every cell's grid column with the HTML occupancy algorithm:
/// a cell slides right past columns still covered by rowspans from earlier
/// rows. Also clamps rowspans to the rows that exist (`0` = to the end).
fn assign_grid_positions(table: &mut TableBox) {
    let row_count = table.rows.len();
    // Per column: how many rows (including the current one) a spanning
    // cell still covers.
    let mut occupied: Vec<usize> = Vec::new();
    for row_idx in 0..row_count {
        let mut col = 0usize;
        for cell in &mut table.rows[row_idx].cells {
            while col < occupied.len() && occupied[col] > 0 {
                col += 1;
            }
            cell.col = col;
            cell.rowspan = if cell.rowspan == 0 {
                row_count - row_idx
            } else {
                cell.rowspan.min(row_count - row_idx)
            };
            let end = col + cell.colspan;
            if occupied.len() < end {
                occupied.resize(end, 0);
            }
            for slot in &mut occupied[col..end] {
                *slot = cell.rowspan;
            }
            col = end;
        }
        for slot in &mut occupied {
            *slot = slot.saturating_sub(1);
        }
    }
    table.columns = table
        .rows
        .iter()
        .flat_map(|r| r.cells.iter())
        .map(|c| c.col + c.colspan)
        .max()
        .unwrap_or(0);
}

fn build_row(input: &BoxTreeInput, row: NodeId) -> Option<TableRow> {
    let doc = input.doc;
    let mut cells = Vec::new();
    let mut all_th = true;
    for child in &doc.node(row).children {
        let NodeData::Element(el) = &doc.node(*child).data else {
            continue;
        };
        let Some(style) = doc.primary_styles(*child) else {
            continue;
        };
        if display_inside(&style) != DisplayInside::TableCell {
            continue;
        }
        let colspan = el
            .attr(&markup5ever::local_name!("colspan"))
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(1)
            .clamp(1, 100);
        // rowspan=0 is HTML for "to the end of the table"; resolved (and
        // clamped to the rows that exist) once all rows are collected.
        let rowspan = el
            .attr(&markup5ever::local_name!("rowspan"))
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(1)
            .min(100);
        all_th &= *el.local_name() == markup5ever::local_name!("th");
        let mut content = InlineContent::default();
        collect_flattened(input, *child, &style, &mut content);
        if input.frag.get(child).is_some_and(|f| f.hyphens_auto) {
            crate::hyphenate::apply(&mut content);
        }
        cells.push(TableCell {
            node: *child,
            style,
            colspan,
            rowspan,
            col: 0, // resolved by the grid-assignment pass
            content,
        });
    }
    (!cells.is_empty()).then_some(TableRow {
        is_header: all_th,
        cells,
    })
}

/// Flatten a cell's subtree into one IFC: inline content concatenates,
/// block-level children separate with hard line breaks.
fn collect_flattened(
    input: &BoxTreeInput,
    node: NodeId,
    style: &ServoArc<ComputedValues>,
    out: &mut InlineContent,
) {
    let doc = input.doc;
    for child in &doc.node(node).children {
        match &doc.node(*child).data {
            NodeData::Text(text) => {
                append_collapsed_pub(
                    out,
                    text,
                    input.locator.get(child).copied().unwrap_or(0),
                    style.clone(),
                );
            }
            NodeData::Element(_) => {
                if doc.is_html_element(*child, &markup5ever::local_name!("br")) {
                    let offset = input.locator.get(child).copied().unwrap_or(0);
                    out.runs.push(crate::boxtree::InlineRun {
                        text: "\n".to_string(),
                        offsets: vec![offset],
                        style: style.clone(),
                    });
                    continue;
                }
                let Some(child_style) = doc.primary_styles(*child) else {
                    continue;
                };
                match display_of(&child_style) {
                    DisplayClass::None => {}
                    DisplayClass::Inline => {
                        collect_flattened(input, *child, &child_style, out);
                    }
                    _ => {
                        // Block-level inside a cell: hard break, then its
                        // content, then a break before whatever follows.
                        push_break(out, input, *child);
                        collect_flattened(input, *child, &child_style, out);
                        push_break(out, input, *child);
                    }
                }
            }
            _ => {}
        }
    }
}

fn push_break(out: &mut InlineContent, input: &BoxTreeInput, node: NodeId) {
    // Avoid leading/doubled breaks.
    let ends_with_break = out
        .runs
        .last()
        .and_then(|r| r.text.chars().next_back())
        .is_none_or(|c| c == '\n');
    if ends_with_break {
        return;
    }
    let offset = input.locator.get(&node).copied().unwrap_or(0);
    out.runs.push(crate::boxtree::InlineRun {
        text: "\n".to_string(),
        offsets: vec![offset],
        style: input.doc.primary_styles(node).expect("styled element"),
    });
}
