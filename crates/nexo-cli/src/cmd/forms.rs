//! Forms with their submissions, and collections with their entries.

use anyhow::{Context as _, Result, bail};
use nexo_core::{Body, Client};
use reqwest::Method;
use serde_json::{Value, json};

use crate::cli::{CollectionsCommand, FormsCommand};
use crate::cmd::content::{confirm, write_or_print};
use crate::ctx::{Ctx, query, text};
use crate::out;

async fn form_id(ctx: &Ctx, client: &Client, site: Option<&str>, wanted: &str) -> Result<String> {
    if wanted.starts_with("form_") {
        return Ok(wanted.to_owned());
    }
    let site = ctx.site_id(client, site).await?;
    let forms = client
        .get("/forms", &query(&[("site", Some(&site))]))
        .await?;
    let lower = wanted.to_lowercase();
    forms["data"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|f| {
            text(&f["key"]).to_lowercase() == lower || text(&f["name"]).to_lowercase() == lower
        })
        .map(|f| text(&f["id"]))
        .with_context(|| format!("nie ma formularza „{wanted}” (nexo forms list)"))
}

pub async fn forms(ctx: &Ctx, command: FormsCommand) -> Result<()> {
    let client = ctx.client()?;
    match command {
        FormsCommand::List { site } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let list = client
                .get("/forms", &query(&[("site", Some(&site))]))
                .await?;
            let rows = list["data"].as_array().cloned().unwrap_or_default();
            out::list(
                ctx.json,
                &rows,
                &[
                    ("Id", &out::field("id")),
                    ("Klucz", &out::field("key")),
                    ("Nazwa", &out::field("name")),
                    ("Zgłoszenia", &out::field("submission_count")),
                    ("Nowe", &out::field("unread_count")),
                ],
            );
        }
        FormsCommand::Show { site, form } => {
            let id = form_id(ctx, &client, site.site.as_deref(), &form).await?;
            out::value(&client.get(&format!("/forms/{id}"), &[]).await?["data"]);
        }
        FormsCommand::Submissions {
            site,
            form,
            status,
            q,
            limit,
        } => {
            let site_id = ctx.site_id(&client, site.site.as_deref()).await?;
            let form = match form {
                Some(f) => Some(form_id(ctx, &client, Some(&site_id), &f).await?),
                None => None,
            };
            let rows = client
                .list_all(
                    "/form-submissions",
                    &query(&[
                        ("site", Some(&site_id)),
                        ("form", form.as_deref()),
                        ("status", status.as_deref()),
                        ("q", q.as_deref()),
                    ]),
                    limit,
                )
                .await?;
            out::list(
                ctx.json,
                &rows,
                &[
                    ("Id", &out::field("id")),
                    ("Kiedy", &|r: &Value| out::when(&r["created_at"])),
                    ("Status", &out::field("status")),
                    ("Formularz", &|r: &Value| {
                        let name = text(&r["form_name"]);
                        if name.is_empty() {
                            text(&r["form_id"])
                        } else {
                            name
                        }
                    }),
                    ("Od", &|r: &Value| {
                        let data = &r["data"];
                        let email = text(&data["email"]);
                        let name = ["imie", "imię", "name", "imie_nazwisko", "firma", "company"]
                            .iter()
                            .map(|k| text(&data[*k]))
                            .find(|v| !v.is_empty())
                            .unwrap_or_default();
                        match (name.is_empty(), email.is_empty()) {
                            (false, false) => format!("{name} <{email}>"),
                            (false, true) => name,
                            _ => email,
                        }
                    }),
                    ("Strona", &out::field("page_path")),
                ],
            );
        }
        FormsCommand::Submission { id } => {
            let submission = client.get(&format!("/form-submissions/{id}"), &[]).await?;
            out::value(&submission["data"]);
        }
        FormsCommand::Mark { id, status } => {
            client
                .json(
                    Method::PATCH,
                    &format!("/form-submissions/{id}"),
                    &[],
                    Body::Json(json!({ "status": status })),
                )
                .await?;
            eprintln!("Zgłoszenie {id}: {status}.");
        }
        FormsCommand::Export { site, form, output } => {
            let id = form_id(ctx, &client, site.site.as_deref(), &form).await?;
            let csv = client.bytes(&format!("/forms/{id}/export")).await?;
            let text = String::from_utf8_lossy(&csv);
            write_or_print(&text, output.as_deref())?;
        }
    }
    Ok(())
}

async fn collection(ctx: &Ctx, client: &Client, site: Option<&str>, wanted: &str) -> Result<Value> {
    let site = ctx.site_id(client, site).await?;
    let list = client
        .get("/collections", &query(&[("site", Some(&site))]))
        .await?;
    let lower = wanted.to_lowercase();
    list["data"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|c| {
            text(&c["id"]) == wanted
                || text(&c["key"]).to_lowercase() == lower
                || text(&c["name"]).to_lowercase() == lower
        })
        .cloned()
        .with_context(|| format!("nie ma kolekcji „{wanted}” (nexo collections list)"))
}

pub async fn collections(ctx: &Ctx, command: CollectionsCommand) -> Result<()> {
    let client = ctx.client()?;
    match command {
        CollectionsCommand::List { site } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let list = client
                .get("/collections", &query(&[("site", Some(&site))]))
                .await?;
            let rows = list["data"].as_array().cloned().unwrap_or_default();
            out::list(
                ctx.json,
                &rows,
                &[
                    ("Id", &out::field("id")),
                    ("Klucz", &out::field("key")),
                    ("Nazwa", &out::field("name")),
                    ("Wpisy", &out::field("entry_count")),
                    ("Widoczne", &out::field("published_count")),
                ],
            );
        }
        CollectionsCommand::Entries {
            site,
            collection: wanted,
            q,
            limit,
        } => {
            let found = collection(ctx, &client, site.site.as_deref(), &wanted).await?;
            let rows = client
                .list_all(
                    "/collection-entries",
                    &query(&[
                        ("collection", Some(&text(&found["id"]))),
                        ("q", q.as_deref()),
                    ]),
                    limit,
                )
                .await?;
            // The first two fields of the collection say what an entry is.
            let fields: Vec<String> = found["fields"]
                .as_array()
                .into_iter()
                .flatten()
                .take(2)
                .map(|f| text(&f["name"]))
                .collect();
            let first = fields.first().cloned().unwrap_or_default();
            let second = fields.get(1).cloned().unwrap_or_default();
            out::list(
                ctx.json,
                &rows,
                &[
                    ("Id", &out::field("id")),
                    (&first, &|r: &Value| {
                        text(&r["data"][&first]).chars().take(50).collect()
                    }),
                    (&second, &|r: &Value| {
                        text(&r["data"][&second]).chars().take(50).collect()
                    }),
                    ("Widoczny", &|r: &Value| {
                        if r["published"] == true {
                            "tak".into()
                        } else {
                            "nie".into()
                        }
                    }),
                ],
            );
        }
        CollectionsCommand::Add {
            site,
            collection: wanted,
            data,
            publish,
        } => {
            let found = collection(ctx, &client, site.site.as_deref(), &wanted).await?;
            let raw = match data.strip_prefix('@') {
                Some(path) => std::fs::read_to_string(path)
                    .with_context(|| format!("nie mogę przeczytać {path}"))?,
                None => data,
            };
            let fields: Value =
                serde_json::from_str(&raw).context("--data to nie jest poprawny JSON")?;
            if !fields.is_object() {
                bail!("--data to obiekt pól, np. {{\"imie\": \"Jan\"}}");
            }
            let created = client
                .post(
                    "/collection-entries",
                    json!({ "collection_id": found["id"], "data": fields, "published": publish }),
                )
                .await?;
            eprintln!("Dodano {}.", text(&created["data"]["id"]));
        }
        CollectionsCommand::Import {
            site,
            collection: wanted,
            file,
            publish,
            dry_run,
        } => {
            let found = collection(ctx, &client, site.site.as_deref(), &wanted).await?;
            let names: Vec<String> = found["fields"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|f| text(&f["name"]))
                .collect();
            let rows = crate::cmd::catalog::read_csv(&file)?;
            if let Some(first) = rows.first() {
                let unknown: Vec<&String> = first.keys().filter(|k| !names.contains(k)).collect();
                if !unknown.is_empty() {
                    bail!(
                        "kolumny spoza pól kolekcji: {}; pola to: {}",
                        unknown
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                        names.join(", ")
                    );
                }
            }
            let mut added = 0;
            for (line, row) in rows.iter().enumerate() {
                let data: serde_json::Map<String, Value> = row
                    .iter()
                    .filter(|(_, v)| !v.is_empty())
                    .map(|(k, v)| (k.clone(), json!(v)))
                    .collect();
                if data.is_empty() {
                    continue;
                }
                if dry_run {
                    added += 1;
                    continue;
                }
                client
                    .post(
                        "/collection-entries",
                        json!({ "collection_id": found["id"], "data": data, "published": publish }),
                    )
                    .await
                    .with_context(|| format!("wiersz {}", line + 2))?;
                added += 1;
            }
            eprintln!(
                "{} {added} wpisów do „{}”.",
                if dry_run { "Do dodania" } else { "Dodano" },
                text(&found["name"])
            );
        }
        CollectionsCommand::Delete { id, yes } => {
            if !confirm(yes, &format!("Usunąć wpis {id}?"))? {
                return Ok(());
            }
            client
                .json(
                    Method::DELETE,
                    &format!("/collection-entries/{id}"),
                    &[],
                    Body::None,
                )
                .await?;
            eprintln!("Usunięto {id}.");
        }
        CollectionsCommand::Sync {
            site,
            collection: wanted,
        } => {
            let found = collection(ctx, &client, site.site.as_deref(), &wanted).await?;
            client
                .post(
                    &format!("/collections/{}/sync", text(&found["id"])),
                    Value::Null,
                )
                .await?;
            eprintln!("Synchronizacja kolekcji {} zlecona.", text(&found["name"]));
        }
    }
    Ok(())
}
