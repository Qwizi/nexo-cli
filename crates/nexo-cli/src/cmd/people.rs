//! Users, groups and the audit log.

use anyhow::{Context as _, Result};
use nexo_core::{Body, Client};
use reqwest::Method;
use serde_json::{Value, json};

use crate::cli::{AuditArgs, GroupsCommand, UsersCommand};
use crate::cmd::content::confirm;
use crate::ctx::{Ctx, query, text};
use crate::out;

async fn user_id(client: &Client, wanted: &str) -> Result<String> {
    if wanted.starts_with("usr_") {
        return Ok(wanted.to_owned());
    }
    let found = client
        .list_all("/users", &query(&[("q", Some(wanted))]), 50)
        .await?;
    found
        .iter()
        .find(|u| text(&u["email"]).eq_ignore_ascii_case(wanted))
        .map(|u| text(&u["id"]))
        .with_context(|| format!("nie ma konta {wanted}"))
}

async fn group_ids(client: &Client, wanted: &[String]) -> Result<Vec<String>> {
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let groups = client.get("/groups", &[]).await?;
    let all = groups["data"].as_array().cloned().unwrap_or_default();
    wanted
        .iter()
        .map(|w| {
            all.iter()
                .find(|g| text(&g["id"]) == *w || text(&g["name"]).eq_ignore_ascii_case(w))
                .map(|g| text(&g["id"]))
                .with_context(|| format!("nie ma grupy „{w}” (nexo groups list)"))
        })
        .collect()
}

pub async fn users(ctx: &Ctx, command: UsersCommand) -> Result<()> {
    let client = ctx.client()?;
    match command {
        UsersCommand::List { q, status } => {
            let rows = client
                .list_all(
                    "/users",
                    &query(&[("q", q.as_deref()), ("status", status.as_deref())]),
                    1000,
                )
                .await?;
            out::list(
                ctx.json,
                &rows,
                &[
                    ("Id", &out::field("id")),
                    ("Imię i nazwisko", &out::field("display_name")),
                    ("E-mail", &out::field("email")),
                    ("Status", &out::field("status")),
                    ("Logowanie", &|r: &Value| {
                        let mut ways = Vec::new();
                        if r["has_password"] == true {
                            ways.push("hasło");
                        }
                        if r["has_totp"] == true {
                            ways.push("2FA");
                        }
                        ways.join(", ")
                    }),
                    ("Ostatnio", &|r: &Value| out::when(&r["last_login_at"])),
                ],
            );
        }
        UsersCommand::Show { user } => {
            let id = user_id(&client, &user).await?;
            out::value(&client.get(&format!("/users/{id}"), &[]).await?["data"]);
        }
        UsersCommand::Invite {
            email,
            name,
            groups,
        } => {
            let display_name =
                name.unwrap_or_else(|| email.split('@').next().unwrap_or(&email).to_owned());
            let group_ids = group_ids(&client, &groups).await?;
            let user = client
                .post(
                    "/users",
                    json!({ "email": email, "display_name": display_name, "group_ids": group_ids }),
                )
                .await?;
            let id = text(&user["data"]["id"]);
            client
                .post(&format!("/users/{id}/password-link"), Value::Null)
                .await?;
            eprintln!("Zaproszono {email} ({id}); link do ustawienia hasła poszedł e-mailem.");
        }
        UsersCommand::Disable { user, yes } => {
            let id = user_id(&client, &user).await?;
            if !confirm(
                yes,
                &format!("Wyłączyć konto {user}? Wszystkie jego sesje kończą się od razu."),
            )? {
                return Ok(());
            }
            set_status(&client, &id, "disabled").await?;
            eprintln!("Konto {user} wyłączone.");
        }
        UsersCommand::Enable { user } => {
            let id = user_id(&client, &user).await?;
            set_status(&client, &id, "active").await?;
            eprintln!("Konto {user} włączone.");
        }
        UsersCommand::ResendInvite { user } => {
            let id = user_id(&client, &user).await?;
            client
                .post(&format!("/users/{id}/password-link"), Value::Null)
                .await?;
            eprintln!("Link do ustawienia hasła wysłany do {user}.");
        }
        UsersCommand::ResetTotp { user, yes } => {
            let id = user_id(&client, &user).await?;
            if !confirm(
                yes,
                &format!("Usunąć aplikację uwierzytelniającą {user}? Osoba zostanie wylogowana."),
            )? {
                return Ok(());
            }
            client
                .json(
                    Method::DELETE,
                    &format!("/users/{id}/totp"),
                    &[],
                    Body::None,
                )
                .await?;
            eprintln!("Usunięto 2FA konta {user}; przy następnym logowaniu ustawi nową aplikację.");
        }
    }
    Ok(())
}

async fn set_status(client: &Client, id: &str, status: &str) -> Result<()> {
    client
        .json(
            Method::PATCH,
            &format!("/users/{id}"),
            &[],
            Body::Json(json!({ "status": status })),
        )
        .await?;
    Ok(())
}

pub async fn groups(ctx: &Ctx, command: GroupsCommand) -> Result<()> {
    let client = ctx.client()?;
    let list = client.get("/groups", &[]).await?;
    let rows = list["data"].as_array().cloned().unwrap_or_default();
    match command {
        GroupsCommand::List => out::list(
            ctx.json,
            &rows,
            &[
                ("Id", &out::field("id")),
                ("Nazwa", &out::field("name")),
                ("Osoby", &out::field("member_count")),
                ("Witryny", &|r: &Value| {
                    if r["all_sites"] == true {
                        "wszystkie".into()
                    } else {
                        r["site_ids"].as_array().map_or(0, Vec::len).to_string()
                    }
                }),
                ("Uprawnienia", &|r: &Value| {
                    if r["is_admin"] == true {
                        "wszystkie (administratorzy)".into()
                    } else {
                        r["permissions"].as_array().map_or(0, Vec::len).to_string()
                    }
                }),
            ],
        ),
        GroupsCommand::Show { group } => {
            let found = rows
                .iter()
                .find(|g| text(&g["id"]) == group || text(&g["name"]).eq_ignore_ascii_case(&group))
                .with_context(|| format!("nie ma grupy „{group}”"))?;
            out::value(found);
        }
    }
    Ok(())
}

pub async fn audit(ctx: &Ctx, args: AuditArgs) -> Result<()> {
    let client = ctx.client()?;
    let site = match args.site.site {
        Some(s) => Some(ctx.site_id(&client, Some(&s)).await?),
        None => None,
    };
    let actor = match args.user {
        Some(u) => Some(user_id(&client, &u).await?),
        None => None,
    };
    let rows = client
        .list_all(
            "/audit-events",
            &query(&[("site", site.as_deref()), ("actor", actor.as_deref())]),
            args.limit,
        )
        .await?;
    out::list(
        ctx.json,
        &rows,
        &[
            ("Kiedy", &|r: &Value| out::when(&r["created_at"])),
            ("Kto", &|r: &Value| {
                let name = text(&r["actor"]["name"]);
                if name.is_empty() {
                    "system".into()
                } else {
                    name
                }
            }),
            ("Zdarzenie", &out::field("action")),
            ("Zasób", &|r: &Value| {
                format!("{} {}", text(&r["resource_kind"]), text(&r["resource_id"]))
            }),
        ],
    );
    Ok(())
}
