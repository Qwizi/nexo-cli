//! Sites, documents and the media library.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use nexo_core::{Body, Client, FilePart};
use reqwest::Method;
use serde_json::{Value, json};

use crate::cli::{DocsCommand, MediaCommand, SitesCommand};
use crate::ctx::{Ctx, query, text};
use crate::out;

pub async fn sites(ctx: &Ctx, command: SitesCommand) -> Result<()> {
    let client = ctx.client()?;
    match command {
        SitesCommand::List => {
            let list = client.get("/sites", &[]).await?;
            let rows = list["data"].as_array().cloned().unwrap_or_default();
            out::list(
                ctx.json,
                &rows,
                &[
                    ("Id", &out::field("id")),
                    ("Klucz", &out::field("key")),
                    ("Nazwa", &out::field("name")),
                    ("Status", &out::field("status")),
                    ("Adres", &out::field("base_url")),
                ],
            );
        }
        SitesCommand::Show { site } => {
            let id = ctx.site_id(&client, site.as_deref()).await?;
            out::value(&client.get(&format!("/sites/{id}"), &[]).await?["data"]);
        }
    }
    Ok(())
}

pub async fn docs(ctx: &Ctx, command: DocsCommand) -> Result<()> {
    let client = ctx.client()?;
    match command {
        DocsCommand::List {
            site,
            kind,
            status,
            q,
            limit,
        } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let rows = client
                .list_all(
                    "/documents",
                    &query(&[
                        ("site", Some(&site)),
                        ("kind", kind.as_deref()),
                        ("status", status.as_deref()),
                        ("q", q.as_deref()),
                    ]),
                    limit,
                )
                .await?;
            print_documents(ctx.json, &rows);
        }
        DocsCommand::Show { site, doc } => {
            let id = doc_id(ctx, &client, site.site.as_deref(), &doc).await?;
            out::value(&client.get(&format!("/documents/{id}"), &[]).await?["data"]);
        }
        DocsCommand::Tree { site } => {
            let site_id = ctx.site_id(&client, site.site.as_deref()).await?;
            let locale = crate::cmd::structure::default_locale(&client, &site_id).await?;
            let tree = client
                .get(
                    "/pages/tree",
                    &query(&[("site", Some(&site_id)), ("locale", Some(&locale))]),
                )
                .await?;
            if ctx.json {
                out::value(&tree["data"]);
            } else {
                print_tree(
                    tree["data"]
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                    0,
                );
            }
        }
        DocsCommand::Kinds => {
            let kinds = client.get("/document-kinds", &[]).await?;
            let rows = kinds["data"]["kinds"]
                .as_array()
                .or_else(|| kinds["data"].as_array())
                .cloned()
                .unwrap_or_default();
            out::list(
                ctx.json,
                &rows,
                &[
                    ("Rodzaj", &out::field("key")),
                    ("Nazwa", &|r: &Value| {
                        let label = text(&r["label"]);
                        if label.is_empty() {
                            text(&r["name"])
                        } else {
                            label
                        }
                    }),
                    ("Moduł", &out::field("module")),
                ],
            );
        }
        DocsCommand::Create {
            site,
            title,
            kind,
            slug,
            parent,
            locale,
        } => {
            let site_id = ctx.site_id(&client, site.site.as_deref()).await?;
            let locale = match locale {
                Some(l) => l,
                None => text(
                    &client.get(&format!("/sites/{site_id}"), &[]).await?["data"]["default_locale"],
                ),
            };
            let mut body =
                json!({ "site_id": site_id, "kind": kind, "locale": locale, "title": title });
            if let Some(slug) = slug {
                body["slug"] = json!(slug);
            }
            if let Some(parent) = parent {
                body["parent_id"] = json!(doc_id(ctx, &client, Some(&site_id), &parent).await?);
            }
            let created = client.post("/documents", body).await?;
            if ctx.json {
                out::value(&created["data"]);
            } else {
                eprintln!(
                    "Utworzono szkic {} „{title}”.",
                    text(&created["data"]["id"])
                );
            }
        }
        DocsCommand::Publish { site, doc, note } => {
            let id = doc_id(ctx, &client, site.site.as_deref(), &doc).await?;
            let result = publish(&client, &id, note.as_deref()).await?;
            report_publish(&id, &result)?;
        }
        DocsCommand::Unpublish { site, doc, archive } => {
            let id = doc_id(ctx, &client, site.site.as_deref(), &doc).await?;
            client
                .post(
                    &format!("/documents/{id}/unpublish"),
                    json!({ "archive": archive }),
                )
                .await?;
            eprintln!(
                "{} {id}.",
                if archive {
                    "Zarchiwizowano"
                } else {
                    "Wycofano publikację"
                }
            );
        }
        DocsCommand::Duplicate { site, doc } => {
            let id = doc_id(ctx, &client, site.site.as_deref(), &doc).await?;
            let copy = client
                .post(&format!("/documents/{id}/duplicate"), Value::Null)
                .await?;
            eprintln!("Kopia: {}", text(&copy["data"]["id"]));
        }
        DocsCommand::Delete { site, doc, yes } => {
            let id = doc_id(ctx, &client, site.site.as_deref(), &doc).await?;
            let current = client.get(&format!("/documents/{id}"), &[]).await?;
            let title = text(&current["data"]["draft"]["title"]);
            if !confirm(
                yes,
                &format!("Przenieść „{title}” ({id}) do kosza? Zniknie z witryny."),
            )? {
                return Ok(());
            }
            client
                .json(Method::DELETE, &format!("/documents/{id}"), &[], Body::None)
                .await?;
            eprintln!("„{title}” w koszu.");
        }
        DocsCommand::Versions { site, doc } => {
            let id = doc_id(ctx, &client, site.site.as_deref(), &doc).await?;
            let list = client
                .get(&format!("/documents/{id}/versions"), &[])
                .await?;
            let rows = list["data"].as_array().cloned().unwrap_or_default();
            out::list(
                ctx.json,
                &rows,
                &[
                    ("Wersja", &out::field("number")),
                    ("Kiedy", &|r: &Value| out::when(&r["created_at"])),
                    ("Kto", &|r: &Value| {
                        let name = text(&r["created_by_name"]);
                        if name.is_empty() {
                            text(&r["created_by"])
                        } else {
                            name
                        }
                    }),
                    ("Notatka", &out::field("note")),
                ],
            );
        }
        DocsCommand::Restore { site, doc, version } => {
            let id = doc_id(ctx, &client, site.site.as_deref(), &doc).await?;
            client
                .post(
                    &format!("/documents/{id}/versions/{version}/restore"),
                    Value::Null,
                )
                .await?;
            eprintln!("Wersja {version} przywrócona do szkicu. Opublikuj: nexo docs publish {id}");
        }
        DocsCommand::Export { site, doc, output } => {
            let id = doc_id(ctx, &client, site.site.as_deref(), &doc).await?;
            let doc = client.get(&format!("/documents/{id}"), &[]).await?;
            let export = json!({
                "id": doc["data"]["id"],
                "revision": doc["data"]["revision"],
                "draft": doc["data"]["draft"],
            });
            write_or_print(&serde_json::to_string_pretty(&export)?, output.as_deref())?;
        }
        DocsCommand::Import {
            site,
            doc,
            file,
            publish: then_publish,
        } => {
            let id = doc_id(ctx, &client, site.site.as_deref(), &doc).await?;
            let raw = std::fs::read_to_string(&file)
                .with_context(|| format!("nie mogę przeczytać {}", file.display()))?;
            let export: Value =
                serde_json::from_str(&raw).context("plik to nie jest JSON z `docs export`")?;
            let draft = export.get("draft").cloned().unwrap_or(export.clone());
            // The exported revision must still be current, so a change made in
            // the panel meanwhile is not overwritten silently.
            let current = client.get(&format!("/documents/{id}"), &[]).await?;
            let revision = current["data"]["revision"].clone();
            if let Some(exported) = export.get("revision")
                && exported != &revision
            {
                bail!(
                    "dokument zmienił się od eksportu (wersja {} → {}); wyeksportuj go ponownie",
                    text(exported),
                    text(&revision)
                );
            }
            client
                .put(
                    &format!("/documents/{id}/draft"),
                    json!({ "revision": revision, "draft": draft }),
                )
                .await?;
            eprintln!("Szkic zapisany.");
            if then_publish {
                let result = publish(&client, &id, None).await?;
                report_publish(&id, &result)?;
            } else {
                eprintln!("Opublikuj: nexo docs publish {id}");
            }
        }
    }
    Ok(())
}

/// `doc_…` as is; a path such as `/kontakt` is looked up among the site's
/// documents.
pub async fn doc_id(
    ctx: &Ctx,
    client: &Client,
    site: Option<&str>,
    wanted: &str,
) -> Result<String> {
    if wanted.starts_with("doc_") {
        return Ok(wanted.to_owned());
    }
    let site = ctx.site_id(client, site).await?;
    let path = if wanted.starts_with('/') {
        wanted.to_owned()
    } else {
        format!("/{wanted}")
    };
    let docs = client
        .list_all("/documents", &query(&[("site", Some(&site))]), 5000)
        .await?;
    docs.iter()
        .find(|d| d["published_path"].as_str() == Some(path.as_str()))
        .or_else(|| {
            docs.iter()
                .find(|d| d["slug"].as_str() == Some(path.trim_start_matches('/')))
        })
        .map(|d| text(&d["id"]))
        .with_context(|| format!("nie ma dokumentu pod adresem {path} (podaj doc_…)"))
}

fn print_tree(nodes: &[Value], depth: usize) {
    for node in nodes {
        let draft = if node["status"] == "published" {
            ""
        } else {
            "  [szkic]"
        };
        println!(
            "{}{}  {}{draft}",
            "  ".repeat(depth),
            text(&node["title"]),
            text(&node["published_path"]),
        );
        if let Some(children) = node["children"].as_array() {
            print_tree(children, depth + 1);
        }
    }
}

/// Asks before a destructive action, unless `--yes`.
pub fn confirm(yes: bool, question: &str) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        bail!("{question} Dodaj --yes, żeby potwierdzić bez pytania.");
    }
    let answer = dialoguer::Confirm::new()
        .with_prompt(question)
        .default(false)
        .interact()?;
    if !answer {
        eprintln!("Anulowano.");
    }
    Ok(answer)
}

pub fn write_or_print(text: &str, output: Option<&Path>) -> Result<()> {
    match output {
        Some(path) => {
            std::fs::write(path, text)?;
            eprintln!("Zapisano w {}", path.display());
        }
        None => println!("{text}"),
    }
    Ok(())
}

pub fn print_documents(json: bool, rows: &[Value]) {
    out::list(
        json,
        rows,
        &[
            ("Id", &out::field("id")),
            ("Rodzaj", &out::field("kind")),
            ("Tytuł", &|r: &Value| {
                text(&r["title"]).chars().take(60).collect()
            }),
            ("Status", &|r: &Value| {
                let mut status = text(&r["status"]);
                if r["has_unpublished_changes"] == true {
                    status.push_str(" (zmiany)");
                }
                status
            }),
            ("Adres", &out::field("published_path")),
            ("Zmieniony", &|r: &Value| out::when(&r["updated_at"])),
        ],
    );
}

/// Publishes the current draft of a document.
pub async fn publish(client: &Client, id: &str, note: Option<&str>) -> Result<Value> {
    let current = client.get(&format!("/documents/{id}"), &[]).await?;
    let mut body = json!({ "revision": current["data"]["revision"] });
    if let Some(note) = note {
        body["note"] = json!(note);
    }
    Ok(client
        .post(&format!("/documents/{id}/publish"), body)
        .await?)
}

fn report_publish(id: &str, result: &Value) -> Result<()> {
    let data = &result["data"];
    if data["status"] == "published" {
        eprintln!("Opublikowano {id}.");
        return Ok(());
    }
    let issues: Vec<String> = data["issues"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|issue| format!("  • {}", text(&issue["message"])))
        .collect();
    bail!("nie opublikowano {id}:\n{}", issues.join("\n"))
}

pub async fn media(ctx: &Ctx, command: MediaCommand) -> Result<()> {
    let client = ctx.client()?;
    match command {
        MediaCommand::List {
            site,
            q,
            kind,
            limit,
        } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let rows = client
                .list_all(
                    "/media",
                    &query(&[
                        ("site", Some(&site)),
                        ("q", q.as_deref()),
                        ("kind", kind.as_deref()),
                    ]),
                    limit,
                )
                .await?;
            out::list(
                ctx.json,
                &rows,
                &[
                    ("Id", &out::field("id")),
                    ("Plik", &out::field("filename")),
                    ("Rozmiar", &|r: &Value| {
                        r["byte_size"].as_u64().map(size).unwrap_or_default()
                    }),
                    (
                        "Wymiary",
                        &|r: &Value| match (r["width"].as_u64(), r["height"].as_u64()) {
                            (Some(w), Some(h)) => format!("{w}×{h}"),
                            _ => String::new(),
                        },
                    ),
                    ("Opis", &|r: &Value| {
                        text(&r["alt"]).chars().take(40).collect()
                    }),
                ],
            );
        }
        MediaCommand::Upload { site, paths, alt } => {
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let files = expand(&paths)?;
            if files.is_empty() {
                bail!("brak plików do wgrania");
            }
            let bar = indicatif::ProgressBar::new(files.len() as u64);
            let mut uploaded = Vec::new();
            let mut failed = 0;
            for file in &files {
                bar.set_message(file.display().to_string());
                match upload(&client, &site, file, alt.as_deref()).await {
                    Ok(asset) => uploaded.push(asset),
                    Err(e) => {
                        failed += 1;
                        bar.println(format!("✗ {}: {e}", file.display()));
                    }
                }
                bar.inc(1);
            }
            bar.finish_and_clear();
            if ctx.json {
                out::value(&json!(uploaded));
            } else {
                for asset in &uploaded {
                    println!("{}  {}", text(&asset["id"]), text(&asset["filename"]));
                }
                eprintln!("Wgrano {} z {} plików.", uploaded.len(), files.len());
            }
            if failed > 0 {
                bail!("{failed} plików się nie wgrało");
            }
        }
        MediaCommand::Show { id } => {
            out::value(&client.get(&format!("/media/{id}"), &[]).await?["data"]);
        }
        MediaCommand::Edit {
            id,
            alt,
            title,
            caption,
            filename,
        } => {
            let mut body = serde_json::Map::new();
            for (key, value) in [
                ("alt", alt),
                ("title", title),
                ("caption", caption),
                ("filename", filename),
            ] {
                if let Some(value) = value {
                    body.insert(key.into(), json!(value));
                }
            }
            if body.is_empty() {
                bail!("podaj, co zmienić: --alt, --title, --caption albo --filename");
            }
            client
                .json(
                    Method::PATCH,
                    &format!("/media/{id}"),
                    &[],
                    Body::Json(Value::Object(body)),
                )
                .await?;
            eprintln!("Zapisano {id}.");
        }
        MediaCommand::Usages { id } => {
            let list = client.get(&format!("/media/{id}/usages"), &[]).await?;
            let rows = list["data"].as_array().cloned().unwrap_or_default();
            out::list(
                ctx.json,
                &rows,
                &[
                    ("Gdzie", &|r: &Value| {
                        let kind = text(&r["kind"]);
                        if kind.is_empty() {
                            text(&r["object"])
                        } else {
                            kind
                        }
                    }),
                    ("Id", &|r: &Value| {
                        let id = text(&r["document_id"]);
                        if id.is_empty() { text(&r["id"]) } else { id }
                    }),
                    ("Tytuł", &out::field("title")),
                ],
            );
        }
        MediaCommand::Delete { id, force, yes } => {
            if !confirm(yes, &format!("Usunąć plik {id}?"))? {
                return Ok(());
            }
            let q = if force {
                vec![("force".to_owned(), "true".to_owned())]
            } else {
                vec![]
            };
            client
                .json(Method::DELETE, &format!("/media/{id}"), &q, Body::None)
                .await?;
            eprintln!("Usunięto {id}.");
        }
        MediaCommand::Download { id, output } => {
            let asset = client.get(&format!("/media/{id}"), &[]).await?;
            let bytes = client.bytes(&format!("/media/{id}/download")).await?;
            let path = output.unwrap_or_else(|| PathBuf::from(text(&asset["data"]["filename"])));
            std::fs::write(&path, bytes)?;
            eprintln!("Zapisano {}", path.display());
        }
    }
    Ok(())
}

/// Uploads one file; returns the asset.
pub async fn upload(client: &Client, site: &str, file: &Path, alt: Option<&str>) -> Result<Value> {
    let bytes =
        std::fs::read(file).with_context(|| format!("nie mogę przeczytać {}", file.display()))?;
    let filename = file
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("plik")
        .to_owned();
    upload_bytes(client, site, bytes, &filename, alt).await
}

pub async fn upload_bytes(
    client: &Client,
    site: &str,
    bytes: Vec<u8>,
    filename: &str,
    alt: Option<&str>,
) -> Result<Value> {
    let mime = nexo_core::client::mime_of(filename);
    let mut fields = vec![("site_id".to_owned(), site.to_owned())];
    if let Some(alt) = alt.filter(|a| !a.trim().is_empty()) {
        fields.push(("alt".to_owned(), alt.chars().take(300).collect()));
    }
    let body = Body::Multipart {
        fields,
        file: FilePart {
            field: "file".into(),
            filename: filename.to_owned(),
            mime,
            bytes,
        },
    };
    let answer = client
        .json(reqwest::Method::POST, "/media", &[], body)
        .await?;
    Ok(answer["data"]["asset"].clone())
}

/// Files of the given paths; folders are read one level deep, hidden files skipped.
fn expand(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            let mut inside: Vec<PathBuf> = std::fs::read_dir(path)?
                .filter_map(|entry| entry.ok().map(|e| e.path()))
                .filter(|p| p.is_file())
                .filter(|p| {
                    !p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with('.'))
                })
                .collect();
            inside.sort();
            files.extend(inside);
        } else if path.is_file() {
            files.push(path.clone());
        } else {
            bail!("nie ma pliku {}", path.display());
        }
    }
    Ok(files)
}

fn size(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{} kB", b / 1024),
        b => format!("{:.1} MB", b as f64 / 1024.0 / 1024.0),
    }
}
