//! Products in bulk: CSV export and import of prices, sales, stock and names,
//! and photos (added to one product, or a folder matched by product code).
//!
//! CSV follows Polish Excel: `;` between columns, decimal commas, UTF-8 with a
//! byte-order mark. Import reads `;` or `,` and both decimal marks.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use nexo_core::Client;
use serde_json::{Value, json};

use crate::cmd::content::{publish, upload};
use crate::cmd::shop::find_product;
use crate::ctx::{Ctx, query, text};

const COLUMNS: [&str; 13] = [
    "document_id",
    "title",
    "status",
    "path",
    "sku",
    "variant_id",
    "options",
    "price",
    "sale_price",
    "sale_from",
    "sale_to",
    "stock",
    "active",
];

/// Product documents of the site (summaries).
async fn products(client: &Client, site: &str) -> Result<Vec<Value>> {
    Ok(client
        .list_all(
            "/documents",
            &query(&[("site", Some(site)), ("kind", Some("product"))]),
            100_000,
        )
        .await?)
}

/// Shop data (variants) of many products, eight requests at a time.
async fn shop_data(
    client: Arc<Client>,
    site: &str,
    ids: Vec<String>,
) -> Result<HashMap<String, Value>> {
    let bar = indicatif::ProgressBar::new(ids.len() as u64);
    bar.set_message("dane sklepu");
    let mut found = HashMap::new();
    for chunk in ids.chunks(8) {
        let mut set = tokio::task::JoinSet::new();
        for id in chunk {
            let client = client.clone();
            let path = format!("/sites/{site}/modules/commerce/products/{id}");
            let id = id.clone();
            set.spawn(async move { (id, client.get(&path, &[]).await) });
        }
        while let Some(done) = set.join_next().await {
            let (id, answer) = done?;
            found.insert(id, answer?);
            bar.inc(1);
        }
    }
    bar.finish_and_clear();
    Ok(found)
}

pub async fn export(ctx: &Ctx, site: Option<&str>, output: Option<&Path>) -> Result<()> {
    let client = Arc::new(ctx.client()?);
    let site = ctx.site_id(&client, site).await?;
    let docs = products(&client, &site).await?;
    let ids = docs.iter().map(|d| text(&d["id"])).collect();
    let shop = shop_data(client.clone(), &site, ids).await?;
    let mut rows = Vec::new();
    for doc in &docs {
        let id = text(&doc["id"]);
        let variants = shop
            .get(&id)
            .and_then(|p| p["variants"].as_array().cloned())
            .unwrap_or_default();
        if variants.is_empty() {
            rows.push(row(doc, &Value::Null));
        }
        for variant in &variants {
            rows.push(row(doc, variant));
        }
    }
    let json_wanted =
        ctx.json || output.is_some_and(|p| p.extension().is_some_and(|e| e == "json"));
    if json_wanted {
        let objects: Vec<Value> = rows
            .iter()
            .map(|r| {
                Value::Object(
                    COLUMNS
                        .iter()
                        .zip(r)
                        .map(|(k, v)| ((*k).to_owned(), json!(v)))
                        .collect(),
                )
            })
            .collect();
        let text = serde_json::to_string_pretty(&objects)?;
        crate::cmd::content::write_or_print(&text, output)?;
        return Ok(());
    }
    let mut writer = csv::WriterBuilder::new()
        .delimiter(b';')
        .from_writer(Vec::new());
    writer.write_record(COLUMNS)?;
    for r in &rows {
        writer.write_record(r)?;
    }
    let mut bytes = "\u{feff}".as_bytes().to_vec();
    bytes.extend(writer.into_inner().map_err(|e| anyhow::anyhow!("{e}"))?);
    match output {
        Some(path) => {
            std::fs::write(path, bytes)?;
            eprintln!(
                "{} wierszy ({} produktów) w {}",
                rows.len(),
                docs.len(),
                path.display()
            );
        }
        None => print!("{}", String::from_utf8_lossy(&bytes[3..])),
    }
    Ok(())
}

fn row(doc: &Value, variant: &Value) -> Vec<String> {
    let options = variant["options"]
        .as_object()
        .map(|o| {
            o.iter()
                .map(|(k, v)| format!("{k}={}", text(v)))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    vec![
        text(&doc["id"]),
        text(&doc["title"]),
        text(&doc["status"]),
        text(&doc["published_path"]),
        text(&variant["sku"]),
        text(&variant["id"]),
        options,
        money(&variant["price"]),
        money(&variant["sale_price"]),
        date(&variant["sale_from"]),
        date(&variant["sale_to"]),
        text(&variant["stock"]),
        if variant.is_null() {
            String::new()
        } else if variant["active"] == false {
            "nie".into()
        } else {
            "tak".into()
        },
    ]
}

/// Grosze → `1234,56`.
fn money(value: &Value) -> String {
    value
        .as_i64()
        .map(|v| {
            format!(
                "{}{},{:02}",
                if v < 0 { "-" } else { "" },
                v.abs() / 100,
                v.abs() % 100
            )
        })
        .unwrap_or_default()
}

fn date(value: &Value) -> String {
    let raw = text(value);
    raw.get(..10).map(str::to_owned).unwrap_or(raw)
}

/// `1 234,56` / `1234.56` / `1234` → grosze.
fn parse_money(value: &str) -> Result<i64> {
    let cleaned: String = value
        .trim()
        .trim_end_matches("zł")
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '\u{a0}')
        .collect::<String>()
        .replace(',', ".");
    let (whole, fraction) = cleaned.split_once('.').unwrap_or((&cleaned, ""));
    if whole.is_empty()
        || !whole.chars().all(|c| c.is_ascii_digit())
        || fraction.len() > 2
        || !fraction.chars().all(|c| c.is_ascii_digit())
    {
        bail!("„{value}” to nie kwota");
    }
    let fraction = format!("{fraction:0<2}");
    Ok(whole.parse::<i64>()? * 100 + fraction.parse::<i64>()?)
}

/// `2026-11-01` → start of that day (UTC); full RFC 3339 passes through.
fn parse_date(value: &str, end: bool) -> Result<String> {
    let value = value.trim();
    if value.len() == 10 && chrono_like(value) {
        return Ok(format!(
            "{value}T{}Z",
            if end { "23:59:59" } else { "00:00:00" }
        ));
    }
    if value.contains('T') {
        return Ok(value.to_owned());
    }
    bail!("„{value}” to nie data (RRRR-MM-DD)")
}

fn chrono_like(value: &str) -> bool {
    let parts: Vec<&str> = value.split('-').collect();
    parts.len() == 3 && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit()))
}

/// Rows of a CSV as maps of column → cell (header names lower-cased).
pub fn read_csv(file: &Path) -> Result<Vec<BTreeMap<String, String>>> {
    let raw =
        std::fs::read(file).with_context(|| format!("nie mogę przeczytać {}", file.display()))?;
    let raw = raw.strip_prefix("\u{feff}".as_bytes()).unwrap_or(&raw);
    let first_line = raw.split(|b| *b == b'\n').next().unwrap_or_default();
    let delimiter = if first_line.iter().filter(|b| **b == b';').count()
        >= first_line.iter().filter(|b| **b == b',').count()
    {
        b';'
    } else {
        b','
    };
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .from_reader(raw);
    let headers: Vec<String> = reader
        .headers()?
        .iter()
        .map(|h| h.trim().to_lowercase())
        .collect();
    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record?;
        rows.push(
            headers
                .iter()
                .zip(record.iter())
                .map(|(h, v)| (h.clone(), v.trim().to_owned()))
                .collect(),
        );
    }
    Ok(rows)
}

/// What a row changes in a variant (empty cell: no change, `-`: clear).
fn apply(variant: &mut Value, row: &BTreeMap<String, String>) -> Result<Vec<String>> {
    let mut changes = Vec::new();
    let cell = |name: &str| row.get(name).map(String::as_str).filter(|v| !v.is_empty());
    let set =
        |variant: &mut Value, changes: &mut Vec<String>, field: &str, new: Value, shown: String| {
            if variant[field] != new {
                changes.push(format!(
                    "{field}: {} → {shown}",
                    describe(&variant[field], field)
                ));
                variant[field] = new;
            }
        };
    if let Some(v) = cell("price") {
        let price = parse_money(v)?;
        set(
            variant,
            &mut changes,
            "price",
            json!(price),
            money(&json!(price)),
        );
    }
    if let Some(v) = cell("sale_price") {
        let new = if v == "-" {
            Value::Null
        } else {
            json!(parse_money(v)?)
        };
        let shown = if new.is_null() {
            "brak".into()
        } else {
            money(&new)
        };
        set(variant, &mut changes, "sale_price", new, shown);
    }
    for (field, end) in [("sale_from", false), ("sale_to", true)] {
        if let Some(v) = cell(field) {
            let new = if v == "-" {
                Value::Null
            } else {
                json!(parse_date(v, end)?)
            };
            let unchanged = date(&variant[field]) == date(&new);
            if !unchanged {
                let shown = if new.is_null() {
                    "brak".into()
                } else {
                    date(&new)
                };
                set(variant, &mut changes, field, new, shown);
            }
        }
    }
    if let Some(v) = cell("stock") {
        let new = if v == "-" {
            Value::Null
        } else {
            json!(
                v.parse::<i32>()
                    .with_context(|| format!("„{v}” to nie stan magazynu"))?
            )
        };
        let shown = if new.is_null() {
            "bez limitu".into()
        } else {
            text(&new)
        };
        set(variant, &mut changes, "stock", new, shown);
    }
    if let Some(v) = cell("active") {
        let new = matches!(v.to_lowercase().as_str(), "tak" | "1" | "true" | "yes");
        set(
            variant,
            &mut changes,
            "active",
            json!(new),
            if new { "tak".into() } else { "nie".into() },
        );
    }
    Ok(changes)
}

fn describe(value: &Value, field: &str) -> String {
    if value.is_null() {
        return "brak".into();
    }
    match field {
        "price" | "sale_price" => money(value),
        "sale_from" | "sale_to" => date(value),
        _ => text(value),
    }
}

/// The variant as the module's input (it refuses unknown fields).
fn variant_input(variant: &Value) -> Value {
    let mut input = serde_json::Map::new();
    for key in [
        "id",
        "sku",
        "options",
        "price",
        "sale_price",
        "sale_from",
        "sale_to",
        "stock",
        "weight_g",
        "length_mm",
        "width_mm",
        "height_mm",
        "parcels",
        "gallery",
        "active",
    ] {
        if !variant[key].is_null() {
            input.insert(key.into(), variant[key].clone());
        }
    }
    Value::Object(input)
}

pub async fn import(
    ctx: &Ctx,
    site: Option<&str>,
    file: &Path,
    dry_run: bool,
    then_publish: bool,
) -> Result<()> {
    let client = Arc::new(ctx.client()?);
    let site = ctx.site_id(&client, site).await?;
    let rows = read_csv(file)?;
    if rows.is_empty() {
        bail!("pusty plik");
    }
    if !rows[0].contains_key("sku") && !rows[0].contains_key("variant_id") {
        bail!("brak kolumny sku (albo variant_id); najprościej zacząć od `nexo shop export`");
    }
    // Which product each row belongs to: its document_id, or found by SKU.
    let need_lookup = rows
        .iter()
        .any(|r| r.get("document_id").is_none_or(|v| v.is_empty()));
    let docs = products(&client, &site).await?;
    let titles: HashMap<String, String> = docs
        .iter()
        .map(|d| (text(&d["id"]), text(&d["title"])))
        .collect();
    let wanted: Vec<String> = if need_lookup {
        titles.keys().cloned().collect()
    } else {
        let mut ids: Vec<String> = rows
            .iter()
            .filter_map(|r| r.get("document_id").cloned())
            .collect();
        ids.sort();
        ids.dedup();
        ids
    };
    let mut shop = shop_data(client.clone(), &site, wanted).await?;
    let mut by_sku: HashMap<String, String> = HashMap::new();
    for (doc, product) in &shop {
        for v in product["variants"].as_array().into_iter().flatten() {
            by_sku.insert(text(&v["sku"]).to_uppercase(), doc.clone());
        }
    }

    let mut touched: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut renamed: BTreeMap<String, String> = BTreeMap::new();
    let mut problems = Vec::new();
    for (line, row) in rows.iter().enumerate() {
        let line = line + 2;
        let sku = row.get("sku").cloned().unwrap_or_default();
        let doc = row
            .get("document_id")
            .filter(|v| !v.is_empty())
            .cloned()
            .or_else(|| by_sku.get(&sku.to_uppercase()).cloned());
        let Some(doc) = doc else {
            problems.push(format!("wiersz {line}: nie ma produktu z SKU „{sku}”"));
            continue;
        };
        let Some(product) = shop.get_mut(&doc) else {
            problems.push(format!("wiersz {line}: nie ma produktu {doc}"));
            continue;
        };
        let variant_id = row.get("variant_id").cloned().unwrap_or_default();
        let Some(variant) = product["variants"].as_array_mut().and_then(|vs| {
            vs.iter_mut().find(|v| {
                (!variant_id.is_empty() && text(&v["id"]) == variant_id)
                    || (!sku.is_empty() && text(&v["sku"]).eq_ignore_ascii_case(&sku))
            })
        }) else {
            problems.push(format!(
                "wiersz {line}: produkt {doc} nie ma wariantu {sku}{variant_id}"
            ));
            continue;
        };
        match apply(variant, row) {
            Ok(changes) if !changes.is_empty() => {
                touched
                    .entry(doc.clone())
                    .or_default()
                    .extend(changes.into_iter().map(|c| format!("{sku}: {c}")));
            }
            Ok(_) => {}
            Err(e) => problems.push(format!("wiersz {line}: {e}")),
        }
        if let Some(title) = row.get("title").filter(|t| !t.is_empty())
            && titles.get(&doc).is_some_and(|current| current != title)
        {
            renamed.insert(doc.clone(), title.clone());
        }
    }

    for problem in &problems {
        eprintln!("✗ {problem}");
    }
    for (doc, changes) in &touched {
        eprintln!("● {} ({doc})", titles.get(doc).cloned().unwrap_or_default());
        for change in changes {
            eprintln!("    {change}");
        }
    }
    for (doc, title) in &renamed {
        eprintln!("● {doc}: nazwa → „{title}”");
    }
    if touched.is_empty() && renamed.is_empty() {
        eprintln!("Brak zmian.");
        return if problems.is_empty() {
            Ok(())
        } else {
            bail!("{} wierszy z błędami", problems.len())
        };
    }
    if dry_run {
        eprintln!(
            "Próba: {} produktów do zmiany, nic nie zapisano.",
            touched.len() + renamed.len()
        );
        return Ok(());
    }
    if !problems.is_empty() {
        bail!(
            "popraw {} wierszy z błędami i uruchom ponownie (nic nie zapisano)",
            problems.len()
        );
    }
    for doc in touched.keys() {
        let product = &shop[doc];
        let body = json!({
            "tax_rate": product["tax_rate"],
            "shipping_class": product["shipping_class"],
            "variants": product["variants"].as_array().into_iter().flatten().map(variant_input).collect::<Vec<_>>(),
        });
        client
            .put(
                &format!("/sites/{site}/modules/commerce/products/{doc}"),
                body,
            )
            .await
            .with_context(|| format!("zapis {doc}"))?;
    }
    for (doc, title) in &renamed {
        let current = client.get(&format!("/documents/{doc}"), &[]).await?;
        let mut draft = current["data"]["draft"].clone();
        draft["title"] = json!(title);
        client
            .put(
                &format!("/documents/{doc}/draft"),
                json!({ "revision": current["data"]["revision"], "draft": draft }),
            )
            .await?;
        if then_publish {
            publish(&client, doc, Some("Import CSV (nexo)")).await?;
        }
    }
    eprintln!(
        "Zapisano: {} produktów (ceny i stany działają od razu){}.",
        touched.len(),
        if renamed.is_empty() {
            String::new()
        } else if then_publish {
            format!(", {} nazw opublikowanych", renamed.len())
        } else {
            format!(", {} nazw w szkicach (dodaj --publish)", renamed.len())
        }
    );
    Ok(())
}

/// Adds assets to a product's gallery and saves the draft (optionally publishes).
async fn add_to_gallery(
    client: &Client,
    doc: &str,
    assets: &[(String, String)],
    first: bool,
    replace: bool,
    then_publish: bool,
) -> Result<()> {
    let current = client.get(&format!("/documents/{doc}"), &[]).await?;
    let mut draft = current["data"]["draft"].clone();
    let mut gallery = if replace {
        Vec::new()
    } else {
        draft["fields"]["gallery"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    };
    let present: Vec<String> = gallery.iter().map(|g| text(&g["asset_id"])).collect();
    let new: Vec<Value> = assets
        .iter()
        .filter(|(id, _)| !present.contains(id))
        .map(|(id, alt)| json!({ "asset_id": id, "alt": alt }))
        .collect();
    if first {
        gallery.splice(0..0, new);
    } else {
        gallery.extend(new);
    }
    draft["fields"]["gallery"] = json!(gallery);
    client
        .put(
            &format!("/documents/{doc}/draft"),
            json!({ "revision": current["data"]["revision"], "draft": draft }),
        )
        .await?;
    if then_publish {
        let result = publish(client, doc, Some("Zdjęcia (nexo)")).await?;
        if result["data"]["status"] != "published" {
            bail!("nie opublikowano {doc}: {}", result["data"]["issues"]);
        }
    }
    Ok(())
}

pub async fn photos_add(
    ctx: &Ctx,
    site: Option<&str>,
    product: &str,
    files: &[PathBuf],
    first: bool,
    replace: bool,
    then_publish: bool,
) -> Result<()> {
    let client = ctx.client()?;
    let site = ctx.site_id(&client, site).await?;
    let doc = find_product(&client, &site, product).await?;
    let title =
        text(&client.get(&format!("/documents/{doc}"), &[]).await?["data"]["draft"]["title"]);
    let mut assets = Vec::new();
    for file in files {
        let asset = upload(&client, &site, file, Some(&title)).await?;
        eprintln!("  wgrano {} → {}", file.display(), text(&asset["id"]));
        assets.push((text(&asset["id"]), title.clone()));
    }
    add_to_gallery(&client, &doc, &assets, first, replace, then_publish).await?;
    eprintln!(
        "Galeria „{title}” zapisana{}.",
        if then_publish {
            " i opublikowana"
        } else {
            " w szkicu (opublikuj: --publish albo nexo docs publish)"
        }
    );
    Ok(())
}

/// `MON001_2.jpg` → `MON001`: the file name up to the first `_`, `-`, space or dot.
fn code_of(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let code: String = name
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    (!code.is_empty()).then(|| code.to_uppercase())
}

/// Codes a product title carries: words with letters and digits (MON001).
fn codes_in(title: &str) -> Vec<String> {
    title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| {
            w.chars().any(|c| c.is_ascii_digit()) && w.chars().any(|c| c.is_ascii_alphabetic())
        })
        .map(str::to_uppercase)
        .collect()
}

pub async fn photos_import(
    ctx: &Ctx,
    site: Option<&str>,
    folder: &Path,
    replace: bool,
    then_publish: bool,
    dry_run: bool,
) -> Result<()> {
    let client = ctx.client()?;
    let site = ctx.site_id(&client, site).await?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(folder)
        .with_context(|| format!("nie mogę otworzyć {}", folder.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                matches!(
                    e.to_lowercase().as_str(),
                    "jpg" | "jpeg" | "png" | "webp" | "avif" | "gif"
                )
            })
        })
        .collect();
    files.sort_by_key(|p| natural(p));
    let docs = products(&client, &site).await?;
    let mut by_code: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for doc in &docs {
        for code in codes_in(&text(&doc["title"])) {
            by_code
                .entry(code)
                .or_default()
                .push((text(&doc["id"]), text(&doc["title"])));
        }
    }
    let mut plan: BTreeMap<String, (String, Vec<PathBuf>)> = BTreeMap::new();
    let mut unmatched = Vec::new();
    for file in files {
        match code_of(&file).and_then(|code| by_code.get(&code).cloned()) {
            Some(found) if found.len() == 1 => {
                let (id, title) = found[0].clone();
                plan.entry(id)
                    .or_insert_with(|| (title, Vec::new()))
                    .1
                    .push(file);
            }
            Some(found) => unmatched.push(format!(
                "{}: kod pasuje do {} produktów",
                file.display(),
                found.len()
            )),
            None => unmatched.push(format!("{}: brak produktu z tym kodem", file.display())),
        }
    }
    for (doc, (title, files)) in &plan {
        eprintln!(
            "● {title} ({doc}): {}",
            files
                .iter()
                .filter_map(|f| f.file_name()?.to_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    for line in &unmatched {
        eprintln!("✗ {line}");
    }
    if dry_run || plan.is_empty() {
        eprintln!(
            "{} produktów, {} plików bez dopasowania{}.",
            plan.len(),
            unmatched.len(),
            if dry_run {
                " (próba, nic nie wgrano)"
            } else {
                ""
            }
        );
        return Ok(());
    }
    let total: usize = plan.values().map(|(_, f)| f.len()).sum();
    let bar = indicatif::ProgressBar::new(total as u64);
    let mut failed = 0;
    for (doc, (title, files)) in &plan {
        let mut assets = Vec::new();
        for file in files {
            match upload(&client, &site, file, Some(title)).await {
                Ok(asset) => assets.push((text(&asset["id"]), title.clone())),
                Err(e) => {
                    failed += 1;
                    bar.println(format!("✗ {}: {e:#}", file.display()));
                }
            }
            bar.inc(1);
        }
        if let Err(e) = add_to_gallery(&client, doc, &assets, false, replace, then_publish).await {
            failed += 1;
            bar.println(format!("✗ {title}: {e:#}"));
        }
    }
    bar.finish_and_clear();
    eprintln!(
        "Zdjęcia w {} produktach{}.",
        plan.len(),
        if then_publish {
            ", opublikowane"
        } else {
            " (szkice; dodaj --publish albo nexo docs publish)"
        }
    );
    if failed > 0 {
        bail!("{failed} błędów");
    }
    Ok(())
}

/// Sort key that puts `MON001_2` before `MON001_10`.
fn natural(path: &Path) -> Vec<(String, u64)> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_lowercase();
    let mut parts = Vec::new();
    let mut text_part = String::new();
    let mut number = String::new();
    for c in name.chars() {
        if c.is_ascii_digit() {
            number.push(c);
        } else {
            if !number.is_empty() {
                parts.push((std::mem::take(&mut text_part), number.parse().unwrap_or(0)));
                number.clear();
            }
            text_part.push(c);
        }
    }
    parts.push((text_part, number.parse().unwrap_or(0)));
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_money_in_polish_and_english() {
        assert_eq!(parse_money("1234,56").unwrap(), 123_456);
        assert_eq!(parse_money("1 234,5 zł").unwrap(), 123_450);
        assert_eq!(parse_money("1234.56").unwrap(), 123_456);
        assert_eq!(parse_money("999").unwrap(), 99_900);
        assert!(parse_money("12,345").is_err());
        assert!(parse_money("abc").is_err());
        assert_eq!(money(&json!(123_456)), "1234,56");
    }

    #[test]
    fn applies_only_filled_cells() {
        let mut variant = json!({ "sku": "MON001", "price": 10_000, "sale_price": 9_000, "stock": 3, "active": true });
        let row: BTreeMap<String, String> = [
            ("sku", "MON001"),
            ("price", "120,00"),
            ("sale_price", "-"),
            ("stock", ""),
            ("active", "tak"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        let changes = apply(&mut variant, &row).unwrap();
        assert_eq!(changes.len(), 2, "{changes:?}");
        assert_eq!(variant["price"], json!(12_000));
        assert!(variant["sale_price"].is_null());
        assert_eq!(variant["stock"], json!(3));
    }

    #[test]
    fn matches_files_to_product_codes() {
        assert_eq!(
            code_of(Path::new("MON001_2.jpg")).as_deref(),
            Some("MON001")
        );
        assert_eq!(
            code_of(Path::new("mon001-tyl.webp")).as_deref(),
            Some("MON001")
        );
        assert_eq!(
            codes_in("Komoda Monara MON001 prowansalska"),
            vec!["MON001"]
        );
        let mut files = vec![
            PathBuf::from("A_10.jpg"),
            PathBuf::from("A_2.jpg"),
            PathBuf::from("A.jpg"),
        ];
        files.sort_by_key(|p| natural(p));
        assert_eq!(
            files,
            vec![
                PathBuf::from("A.jpg"),
                PathBuf::from("A_2.jpg"),
                PathBuf::from("A_10.jpg")
            ]
        );
    }
}
