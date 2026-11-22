//! `org.a11y.atspi.Table` on a table and `org.a11y.atspi.TableCell` on its
//! cells and column headers: the table's size (all its rows, those a
//! virtualized table hasn't built too), the cell at a row and column, its
//! position, its column header, and which rows are selected. A cell's index
//! is `row * columns + column`; a row a virtualized table hasn't built has
//! no cells to give (a null reference).

use super::super::marshal::{Args, Body, Value};
use super::super::{NULL_PATH, Reply, node_path};
use super::{Node, ret};
use crate::platform::access::{A11yNode, A11yRole};

pub(super) const TABLE: &str = "org.a11y.atspi.Table";
pub(super) const TABLE_CELL: &str = "org.a11y.atspi.TableCell";

/// Whether the node has `Table`.
pub(super) fn is_table(n: &A11yNode) -> bool {
    n.role == A11yRole::Table && n.table.is_some()
}

/// Whether the node has `TableCell`.
pub(super) fn is_cell(n: &A11yNode) -> bool {
    matches!(n.role, A11yRole::Cell | A11yRole::ColumnHeader) && n.cell.is_some()
}

/// The table's `(rows, columns)`.
fn size(n: &A11yNode) -> (i32, i32) {
    n.table
        .map_or((0, 0), |t| (t.rows as i32, t.columns as i32))
}

/// The chain of the item at `row` (and `column`) under the table `n`.
fn item(n: &Node, row: i32, column: Option<i32>) -> Option<Vec<usize>> {
    let row = usize::try_from(row).ok()?;
    let column = match column {
        Some(c) => Some(usize::try_from(c).ok()?),
        None => None,
    };
    let (path, _) = n.node.table_item(row, column)?;
    let mut chain = n.chain.clone();
    chain.extend(path);
    Some(chain)
}

/// The rows of the table `n` that are selected (built ones only).
fn selected_rows(n: &A11yNode) -> Vec<i32> {
    fn go(n: &A11yNode, out: &mut Vec<i32>) {
        for c in &n.children {
            match c.role {
                A11yRole::Row => {
                    if c.selected
                        && let Some(p) = c.cell
                    {
                        out.push(p.row as i32);
                    }
                }
                A11yRole::Table => {}
                _ => go(c, out),
            }
        }
    }
    let mut out = Vec::new();
    go(n, &mut out);
    out
}

/// A `Table` property of the table `n`.
pub(super) fn table_property(bus: &str, n: &Node, prop: &str) -> Option<Value> {
    let (rows, columns) = size(n.node);
    Some(match prop {
        "NRows" => Value::I32(rows),
        "NColumns" => Value::I32(columns),
        "Caption" | "Summary" => Value::Obj(bus.to_string(), NULL_PATH.to_string()),
        "NSelectedRows" => Value::I32(selected_rows(n.node).len() as i32),
        "NSelectedColumns" => Value::I32(0),
        _ => return None,
    })
}

/// A `TableCell` property of the cell `n`.
pub(super) fn cell_property(bus: &str, n: &Node, prop: &str) -> Option<Value> {
    let p = n.node.cell?;
    Some(match prop {
        "ColumnSpan" | "RowSpan" => Value::I32(1),
        "Position" => Value::Pair(p.row as i32, p.column as i32),
        "Table" => Value::Obj(bus.to_string(), table_path(n)),
        _ => return None,
    })
}

/// The path of the table a cell is in (null when none).
fn table_path(n: &Node) -> String {
    n.win
        .root
        .as_deref()
        .and_then(|root| root.table_above(&n.chain))
        .map_or_else(|| NULL_PATH.to_string(), |c| node_path(n.win.id, &c))
}

/// Write an object reference to `chain` (or null).
fn obj(r: &mut Body, bus: &str, n: &Node, chain: Option<Vec<usize>>) {
    match chain {
        Some(c) => r.obj(bus, &node_path(n.win.id, &c)),
        None => r.obj(bus, NULL_PATH),
    };
}

/// A `Table` method on the table `n`.
pub(super) fn table_call(bus: &str, n: &Node, member: &str, args: &mut Args) -> Reply {
    let (rows, columns) = size(n.node);
    let selected = selected_rows(n.node);
    match member {
        "GetAccessibleAt" => {
            let (row, column) = (args.i32().unwrap_or(-1), args.i32().unwrap_or(-1));
            let chain = item(n, row, Some(column));
            ret("(so)", |r| obj(r, bus, n, chain))
        }
        "GetIndexAt" => {
            let (row, column) = (args.i32().unwrap_or(-1), args.i32().unwrap_or(-1));
            let ok = (0..rows).contains(&row) && (0..columns).contains(&column);
            ret("i", |r| {
                r.i32(if ok { row * columns + column } else { -1 });
            })
        }
        "GetRowAtIndex" | "GetColumnAtIndex" => {
            let i = args.i32().unwrap_or(-1);
            let ok = columns > 0 && (0..rows * columns).contains(&i);
            let v = match (ok, member) {
                (false, _) => -1,
                (true, "GetRowAtIndex") => i / columns,
                (true, _) => i % columns,
            };
            ret("i", |r| {
                r.i32(v);
            })
        }
        "GetRowColumnExtentsAtIndex" => {
            let i = args.i32().unwrap_or(-1);
            let ok = columns > 0 && (0..rows * columns).contains(&i);
            let (row, column) = if ok {
                (i / columns, i % columns)
            } else {
                (-1, -1)
            };
            ret("biiiib", |r| {
                r.bool(ok)
                    .i32(row)
                    .i32(column)
                    .i32(i32::from(ok))
                    .i32(i32::from(ok))
                    .bool(ok && selected.contains(&row));
            })
        }
        "GetRowExtentAt" | "GetColumnExtentAt" => ret("i", |r| {
            r.i32(1);
        }),
        "GetRowDescription" => ret("s", |r| {
            r.str("");
        }),
        "GetColumnDescription" => {
            let column = args.i32().unwrap_or(-1);
            let name = header(n, column).map(|(_, h)| h.name.clone());
            ret("s", |r| {
                r.str(name.as_deref().unwrap_or(""));
            })
        }
        "GetRowHeader" => ret("(so)", |r| obj(r, bus, n, None)),
        "GetColumnHeader" => {
            let column = args.i32().unwrap_or(-1);
            let chain = header(n, column).map(|(c, _)| c);
            ret("(so)", |r| obj(r, bus, n, chain))
        }
        "GetSelectedRows" => ret("ai", |r| {
            r.array(4, |r| {
                for &row in &selected {
                    r.i32(row);
                }
            });
        }),
        "GetSelectedColumns" => ret("ai", |r| {
            r.array(4, |_| {});
        }),
        "IsRowSelected" | "IsSelected" => {
            let row = args.i32().unwrap_or(-1);
            ret("b", |r| {
                r.bool(selected.contains(&row));
            })
        }
        "IsColumnSelected" => ret("b", |r| {
            r.bool(false);
        }),
        // Selection goes through the rows' own clicks (their Action); the
        // table doesn't change it here.
        "AddRowSelection"
        | "AddColumnSelection"
        | "RemoveRowSelection"
        | "RemoveColumnSelection" => ret("b", |r| {
            r.bool(false);
        }),
        _ => Reply::Error(super::UNKNOWN_METHOD),
    }
}

/// The header of `column` under the table `n`: its chain and node.
fn header<'a>(n: &Node<'a>, column: i32) -> Option<(Vec<usize>, &'a A11yNode)> {
    let column = usize::try_from(column).ok()?;
    let (_, path) = n
        .node
        .column_headers()
        .into_iter()
        .find(|(c, _)| *c == column)?;
    let node = n.node.at_path(&path)?;
    let mut chain = n.chain.clone();
    chain.extend(path);
    Some((chain, node))
}

/// A `TableCell` method on the cell `n`.
pub(super) fn cell_call(bus: &str, n: &Node, member: &str) -> Reply {
    let p = n
        .node
        .cell
        .unwrap_or(crate::platform::access::A11yCell { row: 0, column: 0 });
    match member {
        "GetRowColumnSpan" => ret("iiii", |r| {
            r.i32(p.row as i32).i32(p.column as i32).i32(1).i32(1);
        }),
        "GetRowHeaderCells" => ret("a(so)", |r| {
            r.array(8, |_| {});
        }),
        "GetColumnHeaderCells" => {
            // A header has none of its own.
            let chain = (n.node.role == A11yRole::Cell)
                .then(|| {
                    let root = n.win.root.as_deref()?;
                    let table = root.table_above(&n.chain)?;
                    let t = root.at_path(&table)?;
                    let (_, path) = t
                        .column_headers()
                        .into_iter()
                        .find(|(c, _)| *c == p.column)?;
                    let mut chain = table;
                    chain.extend(path);
                    Some(chain)
                })
                .flatten();
            ret("a(so)", |r| {
                r.array(8, |r| {
                    if let Some(c) = &chain {
                        r.obj(bus, &node_path(n.win.id, c));
                    }
                });
            })
        }
        _ => Reply::Error(super::UNKNOWN_METHOD),
    }
}
