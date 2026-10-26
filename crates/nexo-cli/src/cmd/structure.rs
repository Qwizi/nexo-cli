//! The site's structure: menus, taxonomies, redirects, layouts, the theme and
//! extensions.

use anyhow::{Context as _, Result, bail};
use nexo_core::{Body, Client};
use reqwest::Method;
use serde_json::{Value, json};

use crate::cli::{
    ExtensionsCommand, LayoutsCommand, MenusCommand, RedirectsCommand, TermsCommand, ThemeCommand,
};
use crate::cmd::content::{confirm, write_or_print};
use crate::ctx::{Ctx, query, text};
use crate::out;

/// An item of `list` whose id, key, slug or name is `wanted`.
fn pick<'a>(items: &'a [Value], wanted: &str, what: &str) -> Result<&'a Value> {
    let lower = wanted.to_lowercase();
    items
        .iter()
        .find(|i| text(&i["id"]) == wanted)
        .or_else(|| {
            items
                .iter()
                .find(|i| text(&i["key"]).to_lowercase() == lower)
        })
        .or_else(|| {
            items
                .iter()
                .find(|i| text(&i["slug"]).to_lowercase() == lower)
        })
        .or_else(|| {
            items
                .iter()
                .find(|i| text(&i["name"]).to_lowercase() == lower)
        })
        .or_else(|| {
            items
                .iter()
                .find(|i| text(&i["kind"]).to_lowercase() == lower)
        })
        .with_context(|| format!("nie ma {what} „{wanted}”"))
}

fn data_list(value: &Value) -> Vec<Value> {
    value["data"].as_array().cloned().unwrap_or_default()
}

/// A menu by id, or by key or name in the given language (default: the
/// site's main language; a menu exists once per language).
async fn menu(
    ctx: &Ctx,
    client: &Client,
    site: Option<&str>,
    wanted: &str,
    locale: Option<&str>,
) -> Result<(String, Value)> {
    let site = ctx.site_id(client, site).await?;
    if wanted.starts_with("menu_") {
        let found = client.get(&format!("/menus/{wanted}"), &[]).await?["data"].clone();
        return Ok((site, found));
    }
    let locale = match locale {
        Some(l) => l.to_owned(),
        None => default_locale(client, &site).await?,
    };
    let menus = data_list(
        &client
            .get(
                "/menus",
                &query(&[("site", Some(&site)), ("locale", Some(&locale))]),
            )
            .await?,
    );
    let found = pick(&menus, wanted, "menu")?.clone();
    Ok((site, found))
}

pub async fn default_locale(client: &Client, site: &str) -> Result<String> {
    Ok(text(
        &client.get(&format!("/sites/{site}"), &[]).await?["data"]["default_locale"],
    ))
}

pub async fn menus(ctx: &Ctx, command: MenusCommand) -> Result<()> {
    let client = ctx.client()?;
    match command {
        MenusCommand::List { site } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let rows = data_list(
                &client
                    .get("/menus", &query(&[("site", Some(&site))]))
                    .await?,
            );
            out::list(
                ctx.json,
                &rows,
                &[
                    ("Id", &out::field("id")),
                    ("Klucz", &out::field("key")),
                    ("Nazwa", &out::field("name")),
                    ("Język", &out::field("locale")),
                    ("Pozycje", &|r: &Value| {
                        r["items"].as_array().map_or(0, Vec::len).to_string()
                    }),
                    ("Stan", &|r: &Value| {
                        if r["has_unpublished_changes"] == true {
                            "zmiany w szkicu".into()
                        } else {
                            "opublikowane".into()
                        }
                    }),
                ],
            );
        }
        MenusCommand::Show {
            site,
            menu: wanted,
            locale,
        } => {
            let (_, menu) = menu(
                ctx,
                &client,
                site.site.as_deref(),
                &wanted,
                locale.as_deref(),
            )
            .await?;
            if ctx.json {
                out::value(&menu);
            } else {
                println!(
                    "{} ({}, {})",
                    text(&menu["name"]),
                    text(&menu["key"]),
                    text(&menu["id"])
                );
                print_items(
                    menu["items"]
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                    1,
                );
            }
        }
        MenusCommand::Export {
            site,
            menu: wanted,
            locale,
            output,
        } => {
            let (_, menu) = menu(
                ctx,
                &client,
                site.site.as_deref(),
                &wanted,
                locale.as_deref(),
            )
            .await?;
            write_or_print(
                &serde_json::to_string_pretty(&menu["items"])?,
                output.as_deref(),
            )?;
        }
        MenusCommand::Import {
            site,
            menu: wanted,
            locale,
            file,
            publish,
        } => {
            let (_, menu) = menu(
                ctx,
                &client,
                site.site.as_deref(),
                &wanted,
                locale.as_deref(),
            )
            .await?;
            let id = text(&menu["id"]);
            let raw = std::fs::read_to_string(&file)
                .with_context(|| format!("nie mogę przeczytać {}", file.display()))?;
            let items: Value =
                serde_json::from_str(&raw).context("plik to nie jest JSON z `menus export`")?;
            let items = items.get("items").cloned().unwrap_or(items);
            client
                .json(
                    Method::PATCH,
                    &format!("/menus/{id}"),
                    &[],
                    Body::Json(json!({ "items": items })),
                )
                .await?;
            eprintln!("Szkic menu zapisany.");
            if publish {
                client
                    .post(&format!("/menus/{id}/publish"), Value::Null)
                    .await?;
                eprintln!("Menu opublikowane.");
            }
        }
        MenusCommand::Publish {
            site,
            menu: wanted,
            locale,
        } => {
            let (_, menu) = menu(
                ctx,
                &client,
                site.site.as_deref(),
                &wanted,
                locale.as_deref(),
            )
            .await?;
            client
                .post(
                    &format!("/menus/{}/publish", text(&menu["id"])),
                    Value::Null,
                )
                .await?;
            eprintln!("Opublikowano menu {}.", text(&menu["name"]));
        }
    }
    Ok(())
}

fn print_items(items: &[Value], depth: usize) {
    for item in items {
        let target = {
            let link = &item["link"];
            let href = text(&link["href"]);
            if href.is_empty() {
                text(&link["document_id"])
            } else {
                href
            }
        };
        println!("{}{}  → {target}", "  ".repeat(depth), text(&item["label"]));
        if let Some(children) = item["children"].as_array() {
            print_items(children, depth + 1);
        }
    }
}

async fn terms_of(client: &Client, site: &str, taxonomy: Option<&str>) -> Result<Vec<Value>> {
    Ok(data_list(
        &client
            .get(
                "/terms",
                &query(&[("site", Some(site)), ("taxonomy", taxonomy)]),
            )
            .await?,
    ))
}

pub async fn terms(ctx: &Ctx, command: TermsCommand) -> Result<()> {
    let client = ctx.client()?;
    match command {
        TermsCommand::List { site, taxonomy } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let rows = terms_of(&client, &site, taxonomy.as_deref()).await?;
            let names: std::collections::HashMap<String, String> = rows
                .iter()
                .map(|t| (text(&t["id"]), text(&t["name"])))
                .collect();
            out::list(
                ctx.json,
                &rows,
                &[
                    ("Id", &out::field("id")),
                    ("Taksonomia", &out::field("taxonomy")),
                    ("Nazwa", &out::field("name")),
                    ("Adres", &out::field("slug")),
                    ("Nadrzędne", &|r: &Value| {
                        names
                            .get(&text(&r["parent_id"]))
                            .cloned()
                            .unwrap_or_default()
                    }),
                ],
            );
        }
        TermsCommand::Add {
            site,
            name,
            taxonomy,
            slug,
            parent,
            description,
        } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let locale =
                text(&client.get(&format!("/sites/{site}"), &[]).await?["data"]["default_locale"]);
            let mut body =
                json!({ "site_id": site, "locale": locale, "taxonomy": taxonomy, "name": name });
            if let Some(slug) = slug {
                body["slug"] = json!(slug);
            }
            if let Some(description) = description {
                body["description"] = json!(description);
            }
            if let Some(parent) = parent {
                let all = terms_of(&client, &site, Some(&taxonomy)).await?;
                body["parent_id"] = pick(&all, &parent, "pojęcia")?["id"].clone();
            }
            let created = client.post("/terms", body).await?;
            eprintln!("Dodano {} „{name}”.", text(&created["data"]["id"]));
        }
        TermsCommand::Import {
            site,
            file,
            taxonomy,
            dry_run,
        } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let locale = default_locale(&client, &site).await?;
            let rows = crate::cmd::catalog::read_csv(&file)?;
            let mut all = terms_of(&client, &site, Some(&taxonomy)).await?;
            let (mut added, mut skipped) = (0, 0);
            for row in &rows {
                let Some(name) = row
                    .get("name")
                    .or_else(|| row.get("nazwa"))
                    .filter(|v| !v.is_empty())
                else {
                    continue;
                };
                if pick(&all, name, "pojęcia").is_ok() {
                    skipped += 1;
                    continue;
                }
                let mut body = json!({ "site_id": site, "locale": locale, "taxonomy": taxonomy, "name": name });
                for (column, field) in [
                    ("slug", "slug"),
                    ("description", "description"),
                    ("opis", "description"),
                ] {
                    if let Some(v) = row.get(column).filter(|v| !v.is_empty()) {
                        body[field] = json!(v);
                    }
                }
                if let Some(parent) = row
                    .get("parent")
                    .or_else(|| row.get("nadrzedne"))
                    .filter(|v| !v.is_empty())
                {
                    body["parent_id"] = pick(&all, parent, "pojęcia nadrzędnego")?["id"].clone();
                }
                if dry_run {
                    eprintln!("  + {name}");
                    added += 1;
                    continue;
                }
                let created = client.post("/terms", body).await?;
                all.push(created["data"].clone());
                added += 1;
            }
            eprintln!(
                "{} {added}, już były {skipped}.",
                if dry_run { "Do dodania" } else { "Dodano" }
            );
        }
        TermsCommand::Edit {
            site,
            term,
            name,
            slug,
            description,
        } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let all = terms_of(&client, &site, None).await?;
            let current = pick(&all, &term, "pojęcia")?.clone();
            let mut body = json!({
                "site_id": current["site_id"],
                "locale": current["locale"],
                "taxonomy": current["taxonomy"],
                "name": name.unwrap_or_else(|| text(&current["name"])),
                "slug": slug.map(Value::from).unwrap_or(current["slug"].clone()),
                "description": description.map(Value::from).unwrap_or(current["description"].clone()),
                "parent_id": current["parent_id"],
                "page_id": current["page_id"],
                "image_asset_id": current["image_asset_id"],
            });
            if let Some(position) = current["position"].as_i64() {
                body["position"] = json!(position);
            }
            let id = text(&current["id"]);
            client
                .json(
                    Method::PATCH,
                    &format!("/terms/{id}"),
                    &[],
                    Body::Json(body),
                )
                .await?;
            eprintln!("Zapisano {id}.");
        }
        TermsCommand::Delete { site, term, yes } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let all = terms_of(&client, &site, None).await?;
            let current = pick(&all, &term, "pojęcia")?;
            let id = text(&current["id"]);
            if !confirm(
                yes,
                &format!(
                    "Usunąć „{}” ({id})? Dokumenty stracą to przypisanie.",
                    text(&current["name"])
                ),
            )? {
                return Ok(());
            }
            client
                .json(Method::DELETE, &format!("/terms/{id}"), &[], Body::None)
                .await?;
            eprintln!("Usunięto {id}.");
        }
    }
    Ok(())
}

pub async fn redirects(ctx: &Ctx, command: RedirectsCommand) -> Result<()> {
    let client = ctx.client()?;
    match command {
        RedirectsCommand::List { site, q, limit } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let rows = client
                .list_all(
                    "/redirects",
                    &query(&[("site", Some(&site)), ("q", q.as_deref())]),
                    limit,
                )
                .await?;
            out::list(
                ctx.json,
                &rows,
                &[
                    ("Id", &out::field("id")),
                    ("Z", &out::field("from_path")),
                    ("Na", &out::field("to_target")),
                    ("Kod", &out::field("status_code")),
                    ("Wejścia", &out::field("hits")),
                ],
            );
        }
        RedirectsCommand::Add {
            site,
            from,
            to,
            code,
        } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            if to.is_none() && code != 410 {
                bail!("podaj adres docelowy (albo --code 410 dla usuniętej strony)");
            }
            let created = client
                .post(
                    "/redirects",
                    json!({ "site_id": site, "from_path": from, "to_target": to, "status_code": code }),
                )
                .await?;
            eprintln!(
                "Dodano {}: {from} → {}",
                text(&created["data"]["id"]),
                to.unwrap_or_else(|| "410".into())
            );
        }
        RedirectsCommand::Import {
            site,
            file,
            dry_run,
        } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let rows = crate::cmd::catalog::read_csv(&file)?;
            let existing: std::collections::HashSet<String> = client
                .list_all("/redirects", &query(&[("site", Some(&site))]), 100_000)
                .await?
                .iter()
                .map(|r| text(&r["from_path"]))
                .collect();
            let (mut added, mut skipped, mut failed) = (0, 0, 0);
            for (line, row) in rows.iter().enumerate() {
                let get = |names: &[&str]| {
                    names
                        .iter()
                        .find_map(|n| row.get(*n).filter(|v| !v.is_empty()).cloned())
                };
                let Some(from) = get(&["from", "from_path", "z", "source", "old"]) else {
                    eprintln!("✗ wiersz {}: brak kolumny from", line + 2);
                    failed += 1;
                    continue;
                };
                let from = path_of(&from);
                let to = get(&["to", "to_target", "na", "target", "new"]);
                let code: u16 = get(&["code", "status", "status_code", "kod"])
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(if to.is_some() { 301 } else { 410 });
                if existing.contains(&from) {
                    skipped += 1;
                    continue;
                }
                if dry_run {
                    eprintln!("  {from} → {} ({code})", to.as_deref().unwrap_or("—"));
                    added += 1;
                    continue;
                }
                match client
                    .post(
                        "/redirects",
                        json!({ "site_id": site, "from_path": from, "to_target": to, "status_code": code }),
                    )
                    .await
                {
                    Ok(_) => added += 1,
                    Err(e) => {
                        failed += 1;
                        eprintln!("✗ {from}: {e}");
                    }
                }
            }
            eprintln!(
                "{} {added}, już były {skipped}, błędy {failed}.",
                if dry_run { "Do dodania" } else { "Dodano" }
            );
            if failed > 0 {
                bail!("{failed} wierszy się nie udało");
            }
        }
        RedirectsCommand::Export { site, output } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let rows = client
                .list_all("/redirects", &query(&[("site", Some(&site))]), 100_000)
                .await?;
            let mut writer = csv::WriterBuilder::new().from_writer(Vec::new());
            writer.write_record(["from", "to", "code", "hits"])?;
            for r in &rows {
                writer.write_record([
                    text(&r["from_path"]),
                    text(&r["to_target"]),
                    text(&r["status_code"]),
                    text(&r["hits"]),
                ])?;
            }
            let bytes = writer.into_inner().map_err(|e| anyhow::anyhow!("{e}"))?;
            write_or_print(&String::from_utf8_lossy(&bytes), output.as_deref())?;
        }
        RedirectsCommand::Delete {
            site,
            redirect,
            yes,
        } => {
            let id = if redirect.starts_with("rdr_") {
                redirect.clone()
            } else {
                let site = ctx.site_id(&client, site.site.as_deref()).await?;
                let all = client
                    .list_all(
                        "/redirects",
                        &query(&[("site", Some(&site)), ("q", Some(&path_of(&redirect)))]),
                        100,
                    )
                    .await?;
                all.iter()
                    .find(|r| text(&r["from_path"]) == path_of(&redirect))
                    .map(|r| text(&r["id"]))
                    .with_context(|| format!("nie ma przekierowania z {redirect}"))?
            };
            if !confirm(yes, &format!("Usunąć przekierowanie {id}?"))? {
                return Ok(());
            }
            client
                .json(Method::DELETE, &format!("/redirects/{id}"), &[], Body::None)
                .await?;
            eprintln!("Usunięto {id}.");
        }
    }
    Ok(())
}

pub async fn layouts(ctx: &Ctx, command: LayoutsCommand) -> Result<()> {
    let client = ctx.client()?;
    let list = |site: String| {
        let client = &client;
        async move {
            Ok::<_, anyhow::Error>(data_list(
                &client
                    .get("/layouts", &query(&[("site", Some(&site))]))
                    .await?,
            ))
        }
    };
    match command {
        LayoutsCommand::List { site } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let rows = list(site).await?;
            out::list(
                ctx.json,
                &rows,
                &[
                    ("Id", &out::field("id")),
                    ("Rodzaj", &out::field("kind")),
                    ("Język", &out::field("locale")),
                    ("Bloki", &|r: &Value| {
                        r["blocks"].as_array().map_or(0, Vec::len).to_string()
                    }),
                    ("Stan", &|r: &Value| {
                        if r["published_at"].is_null() {
                            "szkic".into()
                        } else if r["has_unpublished_changes"] == true {
                            "zmiany w szkicu".into()
                        } else {
                            "opublikowany".into()
                        }
                    }),
                ],
            );
        }
        LayoutsCommand::Show { site, layout } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let rows = list(site).await?;
            out::value(pick(&rows, &layout, "szablonu")?);
        }
        LayoutsCommand::Publish { site, layout } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let rows = list(site).await?;
            let found = pick(&rows, &layout, "szablonu")?;
            let id = text(&found["id"]);
            client
                .post(
                    &format!("/layouts/{id}/publish"),
                    json!({ "revision": found["revision"] }),
                )
                .await?;
            eprintln!("Opublikowano szablon {} ({id}).", text(&found["kind"]));
        }
    }
    Ok(())
}

pub async fn theme(ctx: &Ctx, command: ThemeCommand) -> Result<()> {
    let client = ctx.client()?;
    match command {
        ThemeCommand::Show { site } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            out::value(&client.get(&format!("/sites/{site}/theme"), &[]).await?["data"]);
        }
        ThemeCommand::Publish { site } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            client
                .post(&format!("/sites/{site}/theme/publish"), Value::Null)
                .await?;
            eprintln!("Wygląd opublikowany.");
        }
        ThemeCommand::List => {
            let rows = data_list(&client.get("/themes", &[]).await?);
            out::list(
                ctx.json,
                &rows,
                &[
                    ("Motyw", &out::field("id")),
                    ("Nazwa", &out::field("name")),
                    ("Pakiet", &out::field("extension_id")),
                ],
            );
        }
    }
    Ok(())
}

pub async fn extensions(ctx: &Ctx, command: ExtensionsCommand) -> Result<()> {
    let client = ctx.client()?;
    match command {
        ExtensionsCommand::List { site } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let rows = data_list(
                &client
                    .get(&format!("/sites/{site}/extensions"), &[])
                    .await?,
            );
            out::list(
                ctx.json,
                &rows,
                &[
                    ("Rozszerzenie", &out::field("id")),
                    ("Nazwa", &out::field("name")),
                    ("Wersja", &out::field("version")),
                    ("W witrynie", &|r: &Value| {
                        if r["enabled"] == true {
                            "włączone".into()
                        } else {
                            "wyłączone".into()
                        }
                    }),
                ],
            );
        }
        ExtensionsCommand::Enable { site, extension } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            client
                .json(
                    Method::PUT,
                    &format!("/sites/{site}/extensions/{extension}"),
                    &[],
                    Body::None,
                )
                .await?;
            eprintln!("Włączono {extension}.");
        }
        ExtensionsCommand::Disable {
            site,
            extension,
            yes,
        } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            if !confirm(
                yes,
                &format!(
                    "Wyłączyć {extension}? Jego bloki i rodzaje znikną z panelu i witryny (dane zostają)."
                ),
            )? {
                return Ok(());
            }
            client
                .json(
                    Method::DELETE,
                    &format!("/sites/{site}/extensions/{extension}"),
                    &[],
                    Body::None,
                )
                .await?;
            eprintln!("Wyłączono {extension}.");
        }
    }
    Ok(())
}

/// `https://comfyroom.pl/sklep/?a=1` → `/sklep/?a=1`; a bare path stays.
fn path_of(value: &str) -> String {
    let value = value.trim();
    let rest = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
        .map(|r| r.find('/').map_or("/", |i| &r[i..]))
        .unwrap_or(value);
    let path = if rest.starts_with('/') {
        rest.to_owned()
    } else {
        format!("/{rest}")
    };
    // The API stores paths without a trailing slash.
    let (path, query) = path
        .split_once('?')
        .map_or((path.as_str(), None), |(p, q)| (p, Some(q)));
    let trimmed = path.trim_end_matches('/');
    let path = if trimmed.is_empty() { "/" } else { trimmed };
    match query {
        Some(q) => format!("{path}?{q}"),
        None => path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::path_of;

    #[test]
    fn keeps_only_the_path() {
        assert_eq!(
            path_of("https://comfyroom.pl/sklep/kanapa/"),
            "/sklep/kanapa"
        );
        assert_eq!(path_of("https://comfyroom.pl"), "/");
        assert_eq!(path_of("stary-adres"), "/stary-adres");
        assert_eq!(path_of("/a?b=1"), "/a?b=1");
    }
}
