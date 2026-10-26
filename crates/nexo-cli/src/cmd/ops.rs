//! The whole API from its OpenAPI description: every operation the
//! installation offers (menus, taxonomies, forms, collections, themes,
//! extensions…) can be listed and called by name, so the CLI keeps up with
//! the CMS without new commands.

use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result, bail};
use nexo_core::{Body, Client};
use reqwest::Method;
use serde_json::Value;

use crate::cli::CallArgs;
use crate::ctx::{Ctx, query_pairs, text};
use crate::out;

/// The specification of the profile's installation, cached for a day.
async fn spec(ctx: &Ctx, client: &Client) -> Result<Value> {
    let cache = ctx
        .paths
        .dir
        .join("cache")
        .join(format!("openapi-{}.json", client.profile()));
    let fresh = std::fs::metadata(&cache)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < Duration::from_secs(86_400));
    if fresh
        && let Ok(text) = std::fs::read_to_string(&cache)
        && let Ok(value) = serde_json::from_str(&text)
    {
        return Ok(value);
    }
    let value = client
        .get("/openapi.json", &[])
        .await
        .context("nie udało się pobrać opisu API")?;
    if let Some(parent) = cache.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&cache, serde_json::to_vec(&value)?)?;
    Ok(value)
}

struct Operation {
    id: String,
    method: String,
    path: String,
    summary: String,
    tag: String,
    params: Vec<(String, String, bool)>,
    has_body: bool,
}

fn operations(spec: &Value) -> Vec<Operation> {
    let mut ops = Vec::new();
    for (path, item) in spec["paths"].as_object().into_iter().flatten() {
        for (method, op) in item.as_object().into_iter().flatten() {
            let Some(id) = op["operationId"].as_str() else {
                continue;
            };
            if op["security"]
                .as_array()
                .is_some_and(|s| s.iter().any(|req| req.get("rendererAuth").is_some()))
            {
                continue; // renderer-only operations need a renderer token
            }
            let params = op["parameters"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|p| p["name"] != "Cms-Version")
                .map(|p| (text(&p["name"]), text(&p["in"]), p["required"] == true))
                .collect();
            ops.push(Operation {
                id: id.to_owned(),
                method: method.to_uppercase(),
                path: path.trim_start_matches("/api/v1").to_owned(),
                summary: text(&op["summary"])
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
                tag: op["tags"][0].as_str().unwrap_or_default().to_owned(),
                params,
                has_body: !op["requestBody"].is_null(),
            });
        }
    }
    ops.sort_by(|a, b| (&a.tag, &a.path, &a.method).cmp(&(&b.tag, &b.path, &b.method)));
    ops
}

pub async fn list(ctx: &Ctx, filter: Option<String>) -> Result<()> {
    let client = ctx.client()?;
    let spec = spec(ctx, &client).await?;
    let needle = filter.map(|f| f.to_lowercase());
    let rows: Vec<Value> = operations(&spec)
        .into_iter()
        .filter(|op| {
            needle.as_ref().is_none_or(|n| {
                format!("{} {} {} {}", op.id, op.path, op.summary, op.tag)
                    .to_lowercase()
                    .contains(n)
            })
        })
        .map(|op| {
            serde_json::json!({
                "operation": op.id,
                "method": op.method,
                "path": op.path,
                "group": op.tag,
                "summary": op.summary,
            })
        })
        .collect();
    out::list(
        ctx.json,
        &rows,
        &[
            ("Operacja", &out::field("operation")),
            ("Metoda", &out::field("method")),
            ("Ścieżka", &out::field("path")),
            ("Opis", &|r: &Value| {
                text(&r["summary"]).chars().take(70).collect()
            }),
        ],
    );
    Ok(())
}

pub async fn call(ctx: &Ctx, args: CallArgs) -> Result<()> {
    let client = ctx.client()?;
    let spec = spec(ctx, &client).await?;
    let ops = operations(&spec);
    let Some(op) = ops
        .iter()
        .find(|op| op.id.eq_ignore_ascii_case(&args.operation))
    else {
        let close: Vec<&str> = ops
            .iter()
            .filter(|op| {
                op.id
                    .to_lowercase()
                    .contains(&args.operation.to_lowercase())
            })
            .map(|op| op.id.as_str())
            .take(10)
            .collect();
        bail!(
            "nie ma operacji `{}`{}",
            args.operation,
            if close.is_empty() {
                String::new()
            } else {
                format!("; może: {}", close.join(", "))
            }
        );
    };
    let mut path = op.path.clone();
    let mut query = Vec::new();
    for (key, value) in query_pairs(&args.params)? {
        let placeholder = format!("{{{key}}}");
        if path.contains(&placeholder) {
            path = path.replace(&placeholder, &value);
        } else {
            query.push((key, value));
        }
    }
    let missing: Vec<&str> = op
        .params
        .iter()
        .filter(|(name, place, required)| {
            *required
                && ((place == "path" && path.contains(&format!("{{{name}}}")))
                    || (place == "query" && !query.iter().any(|(k, _)| k == name)))
        })
        .map(|(name, _, _)| name.as_str())
        .collect();
    if !missing.is_empty() {
        bail!("{} potrzebuje: {}", op.id, missing.join(", "));
    }
    let body = match args.data {
        Some(data) => {
            let text = match data.strip_prefix('@') {
                Some(file) => std::fs::read_to_string(file)
                    .with_context(|| format!("nie mogę przeczytać {file}"))?,
                None => data,
            };
            Body::Json(serde_json::from_str(&text).context("--data to nie jest poprawny JSON")?)
        }
        None if op.has_body && op.method != "DELETE" => {
            Body::Json(Value::Object(Default::default()))
        }
        None => Body::None,
    };
    let method: Method = op.method.parse().context("metoda")?;
    out::value(&client.json(method, &path, &query, body).await?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_operations_from_the_spec() {
        let spec = json!({ "paths": {
            "/api/v1/menus/{id}/publish": { "post": {
                "operationId": "publishMenu", "tags": ["menus"], "summary": "Publishes a menu.",
                "parameters": [{ "name": "id", "in": "path", "required": true }],
                "security": [{ "bearerAuth": [] }]
            }},
            "/api/v1/render/sites/{id}/bundle": { "get": {
                "operationId": "renderSiteBundle", "security": [{ "rendererAuth": [] }]
            }}
        }});
        let ops = operations(&spec);
        assert_eq!(ops.len(), 1, "renderer operations are left out");
        assert_eq!(ops[0].id, "publishMenu");
        assert_eq!(ops[0].path, "/menus/{id}/publish");
        assert_eq!(ops[0].params, vec![("id".into(), "path".into(), true)]);
    }
}
