//! The shop module: products are documents of kind `product`, prices, stock
//! and orders come from the module's own API.

use anyhow::{Result, bail};
use nexo_core::Client;
use serde_json::{Value, json};

use crate::cli::{PhotosCommand, ShopCommand};
use crate::cmd::content::print_documents;
use crate::ctx::{Ctx, query, text};
use crate::out;

pub async fn run(ctx: &Ctx, command: ShopCommand) -> Result<()> {
    let client = ctx.client()?;
    match command {
        ShopCommand::Products { site, q, limit } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let rows = client
                .list_all(
                    "/documents",
                    &query(&[
                        ("site", Some(&site)),
                        ("kind", Some("product")),
                        ("q", q.as_deref()),
                    ]),
                    limit,
                )
                .await?;
            print_documents(ctx.json, &rows);
        }
        ShopCommand::Product { site, product } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let id = find_product(&client, &site, &product).await?;
            let document = client.get(&format!("/documents/{id}"), &[]).await?;
            let shop = client
                .get(
                    &format!("/sites/{site}/modules/commerce/products/{id}"),
                    &[],
                )
                .await
                .unwrap_or(Value::Null);
            out::value(&json!({ "document": document["data"], "shop": shop }));
        }
        ShopCommand::Orders {
            site,
            status,
            q,
            limit,
        } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let rows = client
                .list_all(
                    &format!("/sites/{site}/modules/commerce/orders"),
                    &query(&[("status", status.as_deref()), ("q", q.as_deref())]),
                    limit,
                )
                .await?;
            out::list(
                ctx.json,
                &rows,
                &[
                    ("Numer", &out::field("number")),
                    ("Status", &|r: &Value| {
                        let label = text(&r["status_label"]);
                        if label.is_empty() {
                            text(&r["status"])
                        } else {
                            label
                        }
                    }),
                    ("Klient", &out::field("customer")),
                    ("E-mail", &out::field("email")),
                    ("Kwota", &|r: &Value| {
                        out::money(&r["total"], r["currency"].as_str().unwrap_or("PLN"))
                    }),
                    ("Data", &|r: &Value| out::when(&r["created_at"])),
                ],
            );
        }
        ShopCommand::Order { site, id } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let id = order_id(&client, &site, &id).await?;
            out::value(
                &client
                    .get(&format!("/sites/{site}/modules/commerce/orders/{id}"), &[])
                    .await?,
            );
        }
        ShopCommand::Status { site, id, status } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let id = order_id(&client, &site, &id).await?;
            let order = client
                .post(
                    &format!("/sites/{site}/modules/commerce/orders/{id}/status"),
                    json!({ "status": status }),
                )
                .await?;
            eprintln!(
                "Zamówienie {}: {}.",
                text(&order["number"]),
                text(&order["status_label"])
            );
        }
        ShopCommand::Export { site, output } => {
            crate::cmd::catalog::export(ctx, site.site.as_deref(), output.as_deref()).await?;
        }
        ShopCommand::Import {
            site,
            file,
            dry_run,
            publish,
        } => {
            crate::cmd::catalog::import(ctx, site.site.as_deref(), &file, dry_run, publish).await?;
        }
        ShopCommand::Photos(PhotosCommand::Add {
            site,
            product,
            files,
            first,
            replace,
            publish,
        }) => {
            crate::cmd::catalog::photos_add(
                ctx,
                site.site.as_deref(),
                &product,
                &files,
                first,
                replace,
                publish,
            )
            .await?;
        }
        ShopCommand::Photos(PhotosCommand::Import {
            site,
            folder,
            replace,
            publish,
            dry_run,
        }) => {
            crate::cmd::catalog::photos_import(
                ctx,
                site.site.as_deref(),
                &folder,
                replace,
                publish,
                dry_run,
            )
            .await?;
        }
        ShopCommand::Customers { site, q } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let page = client
                .get(
                    &format!("/sites/{site}/modules/commerce/admin/customers"),
                    &query(&[("q", q.as_deref())]),
                )
                .await?;
            print_panel_list(ctx.json, &page);
        }
    }
    Ok(())
}

/// `doc_…` as is; otherwise the one product whose title or code matches.
pub async fn find_product(client: &Client, site: &str, wanted: &str) -> Result<String> {
    if wanted.starts_with("doc_") {
        return Ok(wanted.to_owned());
    }
    let found = client
        .list_all(
            "/documents",
            &query(&[
                ("site", Some(site)),
                ("kind", Some("product")),
                ("q", Some(wanted)),
            ]),
            20,
        )
        .await?;
    match found.as_slice() {
        [] => bail!("nie znalazłem produktu „{wanted}”"),
        [one] => Ok(text(&one["id"])),
        many => {
            // An exact code (e.g. MON001) in the title wins over partial matches.
            let upper = wanted.to_uppercase();
            let exact: Vec<&Value> = many
                .iter()
                .filter(|d| {
                    text(&d["title"])
                        .split_whitespace()
                        .any(|word| word.to_uppercase() == upper)
                })
                .collect();
            if let [one] = exact.as_slice() {
                return Ok(text(&one["id"]));
            }
            let names: Vec<String> = many
                .iter()
                .take(8)
                .map(|d| format!("  {}  {}", text(&d["id"]), text(&d["title"])))
                .collect();
            bail!(
                "„{wanted}” pasuje do kilku produktów, podaj doc_…:\n{}",
                names.join("\n")
            )
        }
    }
}

/// `ord_…` as is; an order number such as `CR/2026/0042` is looked up.
async fn order_id(client: &Client, site: &str, wanted: &str) -> Result<String> {
    if wanted.starts_with("ord_") {
        return Ok(wanted.to_owned());
    }
    let found = client
        .list_all(
            &format!("/sites/{site}/modules/commerce/orders"),
            &query(&[("q", Some(wanted))]),
            20,
        )
        .await?;
    found
        .iter()
        .find(|o| text(&o["number"]).eq_ignore_ascii_case(wanted))
        .or_else(|| (found.len() == 1).then(|| &found[0]))
        .map(|o| text(&o["id"]))
        .ok_or_else(|| anyhow::anyhow!("nie ma zamówienia {wanted}"))
}

/// A list page of a module's panel (columns and rows) as a table.
fn print_panel_list(json: bool, page: &Value) {
    let rows = page["rows"].as_array().cloned().unwrap_or_default();
    if json {
        out::value(&json!(rows));
        return;
    }
    let columns: Vec<(String, String)> = page["columns"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| (text(&c["key"]), text(&c["label"])))
        .collect();
    let getters: Vec<out::Getter> = columns
        .iter()
        .map(|(key, _)| {
            let key = key.clone();
            Box::new(move |row: &Value| {
                let cell = &row["cells"][&key];
                let mut value = text(&cell["text"]);
                if value.is_empty() {
                    value = text(&cell["badge"]["label"]);
                }
                if value.is_empty() {
                    value = out::when(&cell["date"]);
                }
                value
            }) as out::Getter
        })
        .collect();
    let id = |row: &Value| text(&row["id"]);
    let mut table: Vec<out::Column> = vec![("Id", &id)];
    for ((_, label), get) in columns.iter().zip(&getters) {
        table.push((label.as_str(), get.as_ref()));
    }
    out::list(false, &rows, &table);
}
