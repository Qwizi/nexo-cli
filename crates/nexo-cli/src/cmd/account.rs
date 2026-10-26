//! Signing in, profiles, sessions and raw API calls.

use std::io::IsTerminal;

use anyhow::{Context as _, Result, bail};
use nexo_core::config::normalize_url;
use nexo_core::{Body, Profile, auth, client};
use reqwest::Method;
use serde_json::Value;

use crate::cli::{ApiArgs, LoginArgs, ProfileCommand, SessionsCommand};
use crate::ctx::{Ctx, query_pairs, text};
use crate::out;

pub async fn login(ctx: &mut Ctx, mut args: LoginArgs) -> Result<()> {
    let known = match &ctx.requested_profile {
        Some(name) => ctx.config.profiles.contains_key(name),
        None => !ctx.config.profiles.is_empty(),
    };
    if args.url.is_none() && !known {
        args.url = Some(ask_url()?);
    }
    let name = match (&ctx.requested_profile, &args.url) {
        (Some(name), _) => name.clone(),
        (None, Some(url)) => profile_name_for(url),
        (None, None) => ctx.profile_name()?,
    };
    let url = match &args.url {
        Some(url) => normalize_url(url)?,
        None => ctx
            .config
            .profiles
            .get(&name)
            .map(|p| p.url.clone())
            .context("podaj adres panelu: nexo login --url https://cms.twoja-domena.pl")?,
    };
    let http = client::http()?;
    let device = args.device_name.clone().unwrap_or_else(auth::device_name);
    let code = auth::start(&http, &url, &device).await?;

    eprintln!();
    eprintln!("  Kod: {}", code.user_code);
    eprintln!(
        "  Zatwierdź go w panelu: {}",
        code.verification_uri_complete
    );
    eprintln!();
    if !args.no_browser
        && std::io::stderr().is_terminal()
        && open::that(&code.verification_uri_complete).is_ok()
    {
        eprintln!("Otworzyłem przeglądarkę. Sprawdź, czy kod się zgadza, i kliknij Zezwól.");
    }
    let spinner = indicatif::ProgressBar::new_spinner();
    spinner.set_message("Czekam na zatwierdzenie w panelu…");
    spinner.enable_steady_tick(std::time::Duration::from_millis(120));
    let tokens = auth::wait(&http, &url, &code).await;
    spinner.finish_and_clear();
    let tokens = tokens?;

    let store = nexo_core::TokenStore::new(&ctx.paths);
    store.save(&name, &tokens)?;
    let previous_site = ctx.config.profiles.get(&name).and_then(|p| p.site.clone());
    ctx.config.profiles.insert(
        name.clone(),
        Profile {
            url,
            site: args.site.or(previous_site),
        },
    );
    if ctx.config.default_profile.is_none() {
        ctx.config.default_profile = Some(name.clone());
    }
    ctx.config.save(&ctx.paths)?;
    eprintln!(
        "Zalogowano jako {} (profil `{name}`, tokeny: {}).",
        tokens.email.as_deref().unwrap_or("?"),
        store.describe()
    );
    Ok(())
}

/// The panel address, asked on the first sign-in.
fn ask_url() -> Result<String> {
    if !std::io::stdin().is_terminal() {
        bail!("podaj adres panelu: nexo login --url https://cms.twoja-domena.pl (albo NEXO_URL)");
    }
    let url: String = dialoguer::Input::new()
        .with_prompt("Adres panelu CMS (np. https://cms.twoja-domena.pl)")
        .validate_with(|input: &String| normalize_url(input).map(|_| ()).map_err(|e| e.to_string()))
        .interact_text()?;
    Ok(normalize_url(&url)?)
}

/// `https://cms.comfyroom.pl` → `comfyroom`, `http://localhost:35173` → `local`.
fn profile_name_for(url: &str) -> String {
    let host = url
        .split("://")
        .nth(1)
        .unwrap_or(url)
        .split(['/', ':'])
        .next()
        .unwrap_or_default();
    if host == "localhost" || host == "127.0.0.1" || host.ends_with(".localhost") {
        return "local".into();
    }
    let parts: Vec<&str> = host.split('.').collect();
    match parts.len() {
        0 | 1 => host.to_owned(),
        n => parts[n - 2].to_owned(),
    }
}

pub async fn logout(ctx: &Ctx) -> Result<()> {
    let client = ctx.client()?;
    let ended = client.post("/auth/logout", Value::Null).await;
    client.store().delete(client.profile())?;
    match ended {
        Ok(_) => eprintln!("Wylogowano (profil `{}`).", client.profile()),
        Err(e) => {
            eprintln!("Tokeny usunięte z tego komputera; sesji w panelu nie udało się zamknąć: {e}")
        }
    }
    Ok(())
}

pub async fn status(ctx: &Ctx) -> Result<()> {
    let name = ctx.profile_name()?;
    let profile = ctx.config.profile(&name)?;
    let client = ctx.client()?;
    let me = client.get("/me", &[]).await;
    if ctx.json {
        out::value(&serde_json::json!({
            "profile": name,
            "url": profile.url,
            "site": profile.site,
            "user": me.as_ref().ok().map(|m| m["data"]["user"].clone()),
        }));
        return Ok(());
    }
    println!("Profil:   {name}");
    println!("Panel:    {}", profile.url);
    println!(
        "Witryna:  {}",
        profile.site.as_deref().unwrap_or("(nie ustawiona)")
    );
    println!("Tokeny:   {}", client.store().describe());
    match me {
        Ok(me) => {
            let user = &me["data"]["user"];
            println!(
                "Konto:    {} <{}>",
                text(&user["display_name"]),
                text(&user["email"])
            );
        }
        Err(e) => println!("Konto:    — ({e})"),
    }
    Ok(())
}

pub fn profile(ctx: &mut Ctx, command: ProfileCommand) -> Result<()> {
    match command {
        ProfileCommand::List => {
            let default = ctx.config.default_profile.clone();
            let rows: Vec<Value> = ctx
                .config
                .profiles
                .iter()
                .map(|(name, p)| {
                    serde_json::json!({
                        "name": name,
                        "url": p.url,
                        "site": p.site,
                        "default": default.as_deref() == Some(name.as_str()),
                    })
                })
                .collect();
            out::list(
                ctx.json,
                &rows,
                &[
                    ("", &|r: &Value| {
                        if r["default"] == true {
                            "*".into()
                        } else {
                            String::new()
                        }
                    }),
                    ("Profil", &out::field("name")),
                    ("Panel", &out::field("url")),
                    ("Witryna", &out::field("site")),
                ],
            );
        }
        ProfileCommand::Use { name } => {
            ctx.config.profile(&name)?;
            ctx.config.default_profile = Some(name.clone());
            ctx.config.save(&ctx.paths)?;
            eprintln!("Domyślny profil: {name}");
        }
        ProfileCommand::Site { site } => {
            let name = ctx.profile_name()?;
            let profile = ctx
                .config
                .profiles
                .get_mut(&name)
                .context("nie ma takiego profilu")?;
            profile.site = Some(site.clone());
            ctx.config.save(&ctx.paths)?;
            eprintln!("Domyślna witryna profilu `{name}`: {site}");
        }
        ProfileCommand::Remove { name } => {
            if ctx.config.profiles.remove(&name).is_none() {
                bail!("nie ma profilu `{name}`");
            }
            if ctx.config.default_profile.as_deref() == Some(name.as_str()) {
                ctx.config.default_profile = None;
            }
            ctx.config.save(&ctx.paths)?;
            nexo_core::TokenStore::new(&ctx.paths).delete(&name)?;
            eprintln!("Usunięto profil `{name}` (sesja w panelu zostaje do wylogowania tam).");
        }
    }
    Ok(())
}

pub async fn sessions(ctx: &Ctx, command: SessionsCommand) -> Result<()> {
    let client = ctx.client()?;
    match command {
        SessionsCommand::List => {
            let list = client.get("/auth/sessions", &[]).await?;
            let rows = list["data"].as_array().cloned().unwrap_or_default();
            out::list(
                ctx.json,
                &rows,
                &[
                    ("", &|r: &Value| {
                        if r["current"] == true {
                            "*".into()
                        } else {
                            String::new()
                        }
                    }),
                    ("Id", &out::field("id")),
                    ("Rodzaj", &|r: &Value| {
                        if r["client"] == "cli" {
                            format!("terminal · {}", text(&r["device_name"]))
                        } else {
                            "przeglądarka".into()
                        }
                    }),
                    ("Ostatnio", &|r: &Value| out::when(&r["last_seen_at"])),
                    ("Program", &|r: &Value| {
                        text(&r["user_agent"]).chars().take(60).collect()
                    }),
                ],
            );
        }
        SessionsCommand::End { id } => {
            client
                .json(
                    Method::DELETE,
                    &format!("/auth/sessions/{id}"),
                    &[],
                    Body::None,
                )
                .await?;
            eprintln!("Sesja {id} wylogowana.");
        }
    }
    Ok(())
}

pub async fn api(ctx: &Ctx, args: ApiArgs) -> Result<()> {
    let client = ctx.client()?;
    let method: Method = args
        .method
        .to_uppercase()
        .parse()
        .context("metoda: GET, POST, PUT, PATCH albo DELETE")?;
    let body = match args.data {
        Some(data) => {
            let text = match data.strip_prefix('@') {
                Some(path) => std::fs::read_to_string(path)
                    .with_context(|| format!("nie mogę przeczytać {path}"))?,
                None => data,
            };
            Body::Json(serde_json::from_str(&text).context("--data to nie jest poprawny JSON")?)
        }
        None => Body::None,
    };
    let path = args.path.trim_start_matches("/api/v1");
    let value = client
        .json(method, path, &query_pairs(&args.query)?, body)
        .await?;
    out::value(&value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::profile_name_for;

    #[test]
    fn names_profiles_after_the_domain() {
        assert_eq!(profile_name_for("https://cms.comfyroom.pl"), "comfyroom");
        assert_eq!(profile_name_for("https://panel.stb.tech"), "stb");
        assert_eq!(profile_name_for("http://localhost:35173"), "local");
    }
}
