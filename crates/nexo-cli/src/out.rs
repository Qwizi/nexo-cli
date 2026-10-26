//! Output: aligned tables for people, JSON with `--json`.

use comfy_table::presets::NOTHING;
use comfy_table::{Cell, ContentArrangement, Table};
use serde_json::Value;

use crate::ctx::text;

/// How to show a row's value in a column.
pub type Getter = Box<dyn Fn(&Value) -> String>;

/// A column: header and how to show a row's value.
pub type Column<'a> = (&'a str, &'a dyn Fn(&Value) -> String);

/// Prints `rows` as a table of `columns` (header, JSON field or function), or
/// the raw values with `--json`.
pub fn list(json: bool, rows: &[Value], columns: &[Column]) {
    if json {
        println!("{}", serde_json::to_string_pretty(rows).unwrap_or_default());
        return;
    }
    if rows.is_empty() {
        eprintln!("(brak)");
        return;
    }
    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(columns.iter().map(|(name, _)| Cell::new(name)));
    for row in rows {
        table.add_row(columns.iter().map(|(_, get)| Cell::new(get(row))));
    }
    println!("{table}");
}

/// A field of the row as text.
pub fn field(name: &'static str) -> impl Fn(&Value) -> String {
    move |row: &Value| text(&row[name])
}

/// A single object: pretty JSON (people read it too).
pub fn value(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_default()
    );
}

/// `2026-10-26T21:40:22Z` → `2026-10-26 21:40`.
pub fn when(value: &Value) -> String {
    let raw = text(value);
    if raw.len() >= 16 {
        raw[..16].replace('T', " ")
    } else {
        raw
    }
}

/// Grosze → `1 234,56 zł`.
pub fn money(minor: &Value, currency: &str) -> String {
    let Some(amount) = minor.as_i64() else {
        return String::new();
    };
    let sign = if amount < 0 { "-" } else { "" };
    let amount = amount.abs();
    let whole = (amount / 100).to_string();
    let mut grouped = String::new();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            grouped.push(' ');
        }
        grouped.push(c);
    }
    let symbol = if currency == "PLN" { "zł" } else { currency };
    format!("{sign}{grouped},{:02} {symbol}", amount % 100)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn formats_money_and_dates() {
        assert_eq!(money(&json!(545190), "PLN"), "5 451,90 zł");
        assert_eq!(money(&json!(99), "PLN"), "0,99 zł");
        assert_eq!(money(&json!(123456789), "EUR"), "1 234 567,89 EUR");
        assert_eq!(when(&json!("2026-10-26T21:40:22.123Z")), "2026-10-26 21:40");
    }
}
