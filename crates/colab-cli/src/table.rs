//! `colab find --table csv|ndjson`: every hit as a row, keyed by its
//! metavariable bindings, for counting call sites and shapes before a
//! migration.
//!
//! ```text
//! path,line,column,rule,id,$E,$M,text
//! store.go,19,10,1,,err,"""remove %s""",…
//! ```
//!
//! `--group-by $M` (repeatable; also `rule`, `id`, `path`) gives one row
//! per distinct value instead, with a `count`, largest first.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};

use clap::ValueEnum;
use colab_core::find::FindReport;
use serde_json::{Map, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum TableFormat {
    Csv,
    Ndjson,
}

/// Column names and rows, every row as long as the columns.
#[derive(Debug, PartialEq, Eq)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

const FIXED: [&str; 5] = ["path", "line", "column", "rule", "id"];

/// One row per hit in `report` (which should hold every hit).
pub fn hits(report: &FindReport) -> Table {
    let vars: BTreeSet<String> = report
        .hits
        .iter()
        .flat_map(|h| {
            h.located
                .captures
                .iter()
                .map(|(name, _)| format!("${name}"))
        })
        .collect();
    let mut columns: Vec<String> = FIXED.iter().map(|c| c.to_string()).collect();
    columns.extend(vars.iter().cloned());
    columns.push("text".into());
    let rows = report
        .hits
        .iter()
        .map(|hit| {
            let captures: BTreeMap<String, &str> = hit
                .located
                .captures
                .iter()
                .map(|(name, text)| (format!("${name}"), text.as_str()))
                .collect();
            let path = hit.path.to_string_lossy();
            let mut row = vec![
                path.strip_prefix("./").unwrap_or(&path).to_string(),
                hit.located.line.to_string(),
                hit.located.column.to_string(),
                (hit.rule + 1).to_string(),
                report
                    .run
                    .rules
                    .get(hit.rule)
                    .and_then(|r| r.id.clone())
                    .unwrap_or_default(),
            ];
            row.extend(
                vars.iter()
                    .map(|v| captures.get(v).copied().unwrap_or("").to_string()),
            );
            row.push(hit.located.text.clone());
            row
        })
        .collect();
    Table { columns, rows }
}

/// `table` grouped by the columns `by` names: one row per distinct value,
/// with how many rows had it, the most common first (ties in order).
pub fn group(table: &Table, by: &[String]) -> colab_core::Result<Table> {
    let at: Vec<usize> = by
        .iter()
        .map(|name| {
            table.columns.iter().position(|c| c == name).ok_or_else(|| {
                colab_core::Error::Config(format!(
                    "`--group-by {name}`: no such column; the columns are {}",
                    table
                        .columns
                        .iter()
                        .filter(|c| *c != "text" && *c != "line" && *c != "column")
                        .map(|c| format!("`{c}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })
        })
        .collect::<colab_core::Result<_>>()?;
    let mut counts: BTreeMap<Vec<String>, usize> = BTreeMap::new();
    for row in &table.rows {
        *counts
            .entry(at.iter().map(|&i| row[i].clone()).collect())
            .or_default() += 1;
    }
    let mut rows: Vec<(Vec<String>, usize)> = counts.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let mut columns = by.to_vec();
    columns.push("count".into());
    Ok(Table {
        columns,
        rows: rows
            .into_iter()
            .map(|(mut key, n)| {
                key.push(n.to_string());
                key
            })
            .collect(),
    })
}

/// `table` as RFC 4180 CSV.
pub fn write_csv(out: &mut impl Write, table: &Table) -> io::Result<()> {
    let line = |cells: &[String]| {
        cells
            .iter()
            .map(|c| {
                if c.contains([',', '"', '\n', '\r']) {
                    format!("\"{}\"", c.replace('"', "\"\""))
                } else {
                    c.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(",")
    };
    writeln!(out, "{}", line(&table.columns))?;
    for row in &table.rows {
        writeln!(out, "{}", line(row))?;
    }
    Ok(())
}

/// `table` as one JSON object per line. `line`, `column`, `rule` and
/// `count` are numbers; the rest strings.
pub fn write_ndjson(out: &mut impl Write, table: &Table) -> io::Result<()> {
    for row in &table.rows {
        let mut object = Map::new();
        for (column, cell) in table.columns.iter().zip(row) {
            let numeric = matches!(column.as_str(), "line" | "column" | "rule" | "count");
            let value = match cell.parse::<u64>() {
                Ok(n) if numeric => Value::from(n),
                _ => Value::from(cell.as_str()),
            };
            object.insert(column.clone(), value);
        }
        writeln!(out, "{}", Value::Object(object))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Table {
        Table {
            columns: ["path", "line", "column", "rule", "id", "$M", "text"]
                .map(String::from)
                .to_vec(),
            rows: vec![
                ["a.go", "1", "2", "1", "", "\"x\"", "f(\"x\")"]
                    .map(String::from)
                    .to_vec(),
                ["b.go", "3", "4", "1", "", "m", "f(m)"]
                    .map(String::from)
                    .to_vec(),
                ["c.go", "5", "6", "1", "", "\"x\"", "f(\"x\",\n)"]
                    .map(String::from)
                    .to_vec(),
            ],
        }
    }

    #[test]
    fn csv_quotes_what_needs_it_and_ndjson_types_numbers() {
        let mut csv = Vec::new();
        write_csv(&mut csv, &table()).unwrap();
        let csv = String::from_utf8(csv).unwrap();
        assert!(
            csv.starts_with(
                "path,line,column,rule,id,$M,text\na.go,1,2,1,,\"\"\"x\"\"\",\"f(\"\"x\"\")\"\n"
            ),
            "{csv}"
        );
        assert!(csv.contains("\"f(\"\"x\"\",\n)\""), "{csv}");
        let mut nd = Vec::new();
        write_ndjson(&mut nd, &table()).unwrap();
        let first: Value =
            serde_json::from_str(String::from_utf8(nd).unwrap().lines().next().unwrap()).unwrap();
        assert_eq!(first["line"], 1);
        assert_eq!(first["$M"], "\"x\"");
    }

    #[test]
    fn grouping_counts_distinct_values_most_common_first() {
        let grouped = group(&table(), &["$M".into()]).unwrap();
        assert_eq!(grouped.columns, ["$M", "count"]);
        assert_eq!(
            grouped.rows,
            [
                vec!["\"x\"".to_string(), "2".into()],
                vec!["m".into(), "1".into()]
            ]
        );
        let e = group(&table(), &["$Q".into()]).unwrap_err().to_string();
        assert!(e.contains("no such column") && e.contains("`$M`"), "{e}");
    }
}
