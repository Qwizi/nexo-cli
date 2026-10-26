//! `nexo images`: product photos edited by Gemini from the product's real
//! photo, the `cr` workflow for Nexo products. Results are saved locally,
//! optionally uploaded and put into the product's gallery (as a draft).

use anyhow::{Context as _, Result, bail};
use nexo_core::images::{Gemini, Preset, is_generated, output_name, to_jpeg};
use nexo_core::{Client, client};
use serde_json::{Value, json};

use crate::cli::{GenerateArgs, ImagesCommand, PromptsCommand};
use crate::cmd::content::{publish, upload_bytes};
use crate::cmd::shop::find_product;
use crate::ctx::{Ctx, text};

const KEY: &str = "gemini";
/// Collection holding a site's prompts: not public, so the site never shows it.
const PROMPTS_COLLECTION: &str = "nexo-ai-prompty";

pub async fn run(ctx: &Ctx, command: ImagesCommand) -> Result<()> {
    match command {
        ImagesCommand::Setup => {
            let key = dialoguer::Password::new()
                .with_prompt("Klucz Gemini API (aistudio.google.com/apikey)")
                .interact()?;
            if key.trim().is_empty() {
                bail!("pusty klucz");
            }
            let store = nexo_core::TokenStore::new(&ctx.paths);
            store.save_secret(KEY, key.trim())?;
            eprintln!("Klucz zapisany ({}).", store.describe());
            Ok(())
        }
        ImagesCommand::Prompts(command) => prompts(ctx, command).await,
        ImagesCommand::Generate(args) => generate(ctx, args).await,
    }
}

fn gemini_key(ctx: &Ctx) -> Result<String> {
    if let Ok(key) = std::env::var("GEMINI_API_KEY")
        && !key.trim().is_empty()
    {
        return Ok(key);
    }
    nexo_core::TokenStore::new(&ctx.paths)
        .secret(KEY)?
        .context("brak klucza Gemini: `nexo images setup` albo GEMINI_API_KEY")
}

struct Source {
    asset_id: String,
    filename: String,
    mime: String,
    alt: String,
}

async fn generate(ctx: &Ctx, args: GenerateArgs) -> Result<()> {
    let presets: Vec<Preset> = if args.preset.is_empty() {
        Preset::ALL.to_vec()
    } else {
        args.preset
            .iter()
            .map(|p| {
                Preset::parse(p).with_context(|| {
                    format!("nie ma presetu `{p}` (lifestyle-room, warm-packshot)")
                })
            })
            .collect::<Result<_>>()?
    };
    let client = ctx.client()?;
    let site = ctx.site_id(&client, args.site.site.as_deref()).await?;
    let gemini = if args.dry_run {
        None
    } else {
        Some(Gemini::new(
            client::http()?,
            gemini_key(ctx)?,
            ctx.config.images.clone(),
        ))
    };
    let (_, cms) = cms_prompts(&client, &site).await.unwrap_or_default();
    let mut failed = 0;
    for wanted in &args.products {
        match product(
            ctx,
            &client,
            gemini.as_ref(),
            &site,
            wanted,
            &presets,
            &args,
            &cms,
        )
        .await
        {
            Ok(()) => {}
            Err(e) => {
                failed += 1;
                eprintln!("✗ {wanted}: {e:#}");
            }
        }
    }
    if failed > 0 {
        bail!("{failed} z {} produktów się nie udało", args.products.len());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments, reason = "one product of a generate run")]
async fn product(
    ctx: &Ctx,
    client: &Client,
    gemini: Option<&Gemini>,
    site: &str,
    wanted: &str,
    presets: &[Preset],
    args: &GenerateArgs,
    cms: &[(String, String, String)],
) -> Result<()> {
    let id = find_product(client, site, wanted).await?;
    let document = client.get(&format!("/documents/{id}"), &[]).await?["data"].clone();
    let title = text(&document["draft"]["title"]);
    eprintln!("● {title} ({id})");
    let gallery = document["draft"]["fields"]["gallery"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut sources = Vec::new();
    for entry in &gallery {
        let asset_id = text(&entry["asset_id"]);
        let asset = client.get(&format!("/media/{asset_id}"), &[]).await?["data"].clone();
        let filename = text(&asset["filename"]);
        if is_generated(&filename) {
            continue;
        }
        sources.push(Source {
            asset_id,
            filename,
            mime: text(&asset["mime"]),
            alt: text(&entry["alt"]).trim().to_owned(),
        });
    }
    if sources.is_empty() {
        bail!("produkt nie ma własnych zdjęć (tylko wygenerowane albo żadnych)");
    }
    // Like cr: the room from the first photo, the packshot from the next one.
    let source_for = |preset: Preset| match preset {
        Preset::LifestyleRoom => &sources[0],
        Preset::WarmPackshot => sources.get(1).unwrap_or(&sources[0]),
    };

    let folder = args.out.join(&id);
    std::fs::create_dir_all(&folder)?;
    let mut results: Vec<(Preset, String, Vec<u8>, String)> = Vec::new();
    for &preset in presets {
        let source = source_for(preset);
        if args.dry_run {
            eprintln!("  {} ← {}", preset.slug(), source.filename);
            continue;
        }
        let original = client
            .bytes(&format!("/media/{}/download", source.asset_id))
            .await?;
        let name = output_name(&source.filename, preset, &original);
        let path = folder.join(&name);
        let jpeg = if path.exists() {
            eprintln!(
                "  {} ← {}: już jest ({})",
                preset.slug(),
                source.filename,
                path.display()
            );
            std::fs::read(&path)?
        } else {
            let Some(gemini) = gemini else { continue };
            eprintln!("  {} ← {}: generuję…", preset.slug(), source.filename);
            let prompt = prompt_for(ctx, preset, cms);
            let mime = if source.mime.is_empty() {
                "image/jpeg"
            } else {
                &source.mime
            };
            let image = gemini.edit(&prompt, &original, mime).await?;
            let jpeg = to_jpeg(&image, ctx.config.images.quality)?;
            std::fs::write(&path, &jpeg)?;
            eprintln!("    zapisano {}", path.display());
            jpeg
        };
        let alt = if source.alt.is_empty() {
            title.clone()
        } else {
            source.alt.clone()
        };
        results.push((preset, name, jpeg, alt));
    }
    if args.dry_run || !args.upload {
        return Ok(());
    }

    let mut uploaded = Vec::new();
    for (preset, name, jpeg, alt) in results {
        let asset = upload_bytes(client, site, jpeg, &name, Some(&alt)).await?;
        eprintln!("  wgrano {} → {}", name, text(&asset["id"]));
        uploaded.push((preset, text(&asset["id"]), alt));
    }
    if !args.replace {
        return Ok(());
    }

    // The new room photo leads the gallery, the packshot follows; earlier
    // results of the same presets go, every other photo keeps its place.
    let mut kept = Vec::new();
    for entry in gallery {
        let asset = client
            .get(&format!("/media/{}", text(&entry["asset_id"])), &[])
            .await?;
        let filename = text(&asset["data"]["filename"]);
        let replaced = uploaded.iter().any(|(preset, _, _)| {
            filename.contains(&format!("__cr-{}-", preset.slug().replace('_', "-")))
        });
        if !replaced {
            kept.push(entry);
        }
    }
    let mut new_gallery: Vec<Value> = uploaded
        .iter()
        .map(|(_, asset_id, alt)| json!({ "asset_id": asset_id, "alt": alt }))
        .collect();
    new_gallery.extend(kept);
    let mut draft = document["draft"].clone();
    draft["fields"]["gallery"] = json!(new_gallery);
    client
        .put(
            &format!("/documents/{id}/draft"),
            json!({ "revision": document["revision"], "draft": draft }),
        )
        .await?;
    eprintln!("  galeria podmieniona w szkicu");
    if args.publish {
        let result = publish(client, &id, Some("Zdjęcia AI (nexo images)")).await?;
        if result["data"]["status"] == "published" {
            eprintln!("  opublikowano");
        } else {
            eprintln!("  nie opublikowano: {}", result["data"]["issues"]);
        }
    } else {
        eprintln!("  opublikuj: nexo docs publish {id}");
    }
    Ok(())
}

/// Prompts kept in the site's CMS, by preset slug.
async fn cms_prompts(
    client: &Client,
    site: &str,
) -> Result<(Option<String>, Vec<(String, String, String)>)> {
    let list = client
        .get("/collections", &crate::ctx::query(&[("site", Some(site))]))
        .await?;
    let Some(collection) = list["data"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|c| c["key"] == PROMPTS_COLLECTION)
    else {
        return Ok((None, Vec::new()));
    };
    let id = text(&collection["id"]);
    let entries = client
        .list_all(
            "/collection-entries",
            &crate::ctx::query(&[("collection", Some(&id))]),
            100,
        )
        .await?;
    Ok((
        Some(id),
        entries
            .iter()
            .map(|e| {
                (
                    text(&e["id"]),
                    text(&e["data"]["preset"]),
                    join_parts(&e["data"]),
                )
            })
            .collect(),
    ))
}

async fn prompts_collection(client: &Client, site: &str) -> Result<String> {
    if let (Some(id), _) = cms_prompts(client, site).await? {
        return Ok(id);
    }
    let created = client
        .post(
            "/collections",
            json!({
                "site_id": site,
                "key": PROMPTS_COLLECTION,
                "name": "Prompty zdjęć AI",
                "singular_name": "Prompt",
                "description": "Prompty `nexo images` (Gemini). Niepubliczne: witryna ich nie pokazuje.",
                "settings": { "public": false, "title_field": "preset", "sort": "title" },
                "fields": prompt_fields()
            }),
        )
        .await?;
    Ok(text(&created["data"]["id"]))
}

async fn prompts(ctx: &Ctx, command: PromptsCommand) -> Result<()> {
    let dir = ctx.paths.prompts_dir();
    match command {
        PromptsCommand::List { site } => {
            let client = ctx.client()?;
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let (_, cms) = cms_prompts(&client, &site).await?;
            for preset in Preset::ALL {
                let local = dir.join(format!("{}.md", preset.slug()));
                let source = if local.exists() {
                    format!("ten komputer ({})", local.display())
                } else if cms
                    .iter()
                    .any(|(_, p, t)| p == preset.slug() && !t.trim().is_empty())
                {
                    "CMS witryny".into()
                } else {
                    "wbudowany (ogólny)".into()
                };
                println!("  {:<15} {source}", preset.slug());
            }
            Ok(())
        }
        PromptsCommand::Push { site, from } => {
            let client = ctx.client()?;
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let folder = from.unwrap_or_else(|| dir.clone());
            let collection = prompts_collection(&client, &site).await?;
            let (_, existing) = cms_prompts(&client, &site).await?;
            let mut pushed = 0;
            for preset in Preset::ALL {
                let file = folder.join(format!("{}.md", preset.slug()));
                let Ok(prompt) = std::fs::read_to_string(&file) else {
                    continue;
                };
                if prompt.trim().is_empty() {
                    continue;
                }
                match existing.iter().find(|(_, p, _)| p == preset.slug()) {
                    Some((id, _, _)) => {
                        client
                            .json(
                                reqwest::Method::PATCH,
                                &format!("/collection-entries/{id}"),
                                &[],
                                nexo_core::Body::Json(
                                    json!({ "data": prompt_data(preset.slug(), &prompt) }),
                                ),
                            )
                            .await?;
                    }
                    None => {
                        client
                            .post(
                                "/collection-entries",
                                json!({
                                    "collection_id": collection,
                                    "data": prompt_data(preset.slug(), &prompt),
                                    "published": false
                                }),
                            )
                            .await?;
                    }
                }
                eprintln!("  {} → CMS", preset.slug());
                pushed += 1;
            }
            if pushed == 0 {
                bail!(
                    "w {} nie ma plików lifestyle_room.md ani warm_packshot.md",
                    folder.display()
                );
            }
            eprintln!("Prompty zapisane w CMS (kolekcja „Prompty zdjęć AI”, niepubliczna).");
            Ok(())
        }
        PromptsCommand::Pull { site } => {
            let client = ctx.client()?;
            let site = ctx.site_id(&client, site.site.as_deref()).await?;
            let (_, cms) = cms_prompts(&client, &site).await?;
            if cms.is_empty() {
                bail!("CMS tej witryny nie ma promptów (nexo images prompts push)");
            }
            for (_, preset, prompt) in cms {
                let target = dir.join(format!("{preset}.md"));
                nexo_core::config::write_private(&target, prompt.as_bytes())?;
                eprintln!("  {preset} → {}", target.display());
            }
            Ok(())
        }
        PromptsCommand::Set { preset, file } => {
            let preset = Preset::parse(&preset).with_context(|| {
                format!("nie ma presetu `{preset}` (lifestyle-room, warm-packshot)")
            })?;
            let target = dir.join(format!("{}.md", preset.slug()));
            match file {
                Some(file) => {
                    let text = std::fs::read_to_string(&file)
                        .with_context(|| format!("nie mogę przeczytać {}", file.display()))?;
                    if text.trim().is_empty() {
                        bail!("plik jest pusty");
                    }
                    nexo_core::config::write_private(&target, text.as_bytes())?;
                    eprintln!(
                        "Preset {} używa na tym komputerze {}.",
                        preset.slug(),
                        target.display()
                    );
                }
                None => {
                    if target.exists() {
                        std::fs::remove_file(&target)?;
                    }
                    eprintln!(
                        "Lokalny prompt {} usunięty (zostaje CMS albo wbudowany).",
                        preset.slug()
                    );
                }
            }
            Ok(())
        }
    }
}

/// The prompt of a preset: this machine's file, then the site's CMS, then the
/// generic built-in one.
fn prompt_for(ctx: &Ctx, preset: Preset, cms: &[(String, String, String)]) -> String {
    let local = ctx
        .paths
        .prompts_dir()
        .join(format!("{}.md", preset.slug()));
    if local.exists() {
        return preset.prompt(&ctx.paths.prompts_dir());
    }
    cms.iter()
        .find(|(_, p, t)| p == preset.slug() && !t.trim().is_empty())
        .map(|(_, _, t)| t.clone())
        .unwrap_or_else(|| preset.prompt(&ctx.paths.prompts_dir()))
}

/// A collection text area holds 5000 characters; longer prompts are split.
const PART: usize = 4900;
const PARTS: usize = 4;

fn prompt_fields() -> Value {
    let mut fields = vec![json!({
        "name": "preset", "label": "Preset", "kind": "text", "required": true, "public": false
    })];
    for i in 1..=PARTS {
        fields.push(json!({
            "name": format!("prompt_{i}"),
            "label": if i == 1 { "Prompt".to_owned() } else { format!("Prompt (ciąg dalszy {i})") },
            "kind": "textarea",
            "required": i == 1,
            "public": false
        }));
    }
    json!(fields)
}

/// Entry data: the prompt in parts cut at paragraph or line ends.
fn prompt_data(preset: &str, prompt: &str) -> Value {
    let mut data = serde_json::Map::new();
    data.insert("preset".into(), json!(preset));
    for (i, part) in split_prompt(prompt).into_iter().enumerate() {
        data.insert(format!("prompt_{}", i + 1), json!(part));
    }
    Value::Object(data)
}

/// Parts cut between paragraphs (or words), without the separator: the CMS
/// trims field edges, so the cut is restored as a blank line when joined.
fn split_prompt(prompt: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut rest = prompt.trim();
    while rest.chars().count() > PART {
        let limit = rest.char_indices().nth(PART).map_or(rest.len(), |(i, _)| i);
        let head = &rest[..limit];
        let (cut, skip) = head
            .rfind("\n\n")
            .map(|i| (i, 2))
            .or_else(|| head.rfind(' ').map(|i| (i, 1)))
            .filter(|&(i, _)| i > 0)
            .unwrap_or((limit, 0));
        parts.push(rest[..cut].trim().to_owned());
        rest = rest[cut + skip..].trim_start();
    }
    parts.push(rest.to_owned());
    parts
}

fn join_parts(data: &Value) -> String {
    (1..=PARTS)
        .map(|i| text(&data[format!("prompt_{i}")]))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_prompts_survive_the_round_trip() {
        let paragraph = format!("{}koniec.\n\n", "słowo ".repeat(300));
        let prompt = paragraph.repeat(6);
        let parts = split_prompt(&prompt);
        assert!(parts.len() > 1 && parts.len() <= PARTS);
        assert!(parts.iter().all(|p| p.chars().count() <= PART));
        assert_eq!(join_parts(&prompt_data("x", &prompt)), prompt.trim());
        assert_eq!(split_prompt("krótki"), vec!["krótki"]);
    }
}
