//! What every command needs: settings, the chosen profile and its client.

use anyhow::{Context as _, Result, anyhow};
use nexo_core::{Client, Config, Paths};
use serde_json::Value;

pub struct Ctx {
    pub paths: Paths,
    pub config: Config,
    pub requested_profile: Option<String>,
    pub json: bool,
}

impl Ctx {
    pub fn new(profile: Option<String>, json: bool) -> Result<Self> {
        let paths = Paths::discover()?;
        let config = Config::load(&paths)?;
        Ok(Self {
            paths,
            config,
            requested_profile: profile,
            json,
        })
    }

    pub fn profile_name(&self) -> Result<String> {
        Ok(self
            .config
            .profile_name(self.requested_profile.as_deref())?)
    }

    pub fn client(&self) -> Result<Client> {
        let name = self.profile_name()?;
        let profile = self.config.profile(&name)?;
        Ok(Client::new(&self.paths, &name, &profile.url)?)
    }

    /// `site_…` id of `--site`, or of the profile's default site.
    pub async fn site_id(&self, client: &Client, requested: Option<&str>) -> Result<String> {
        let name = self.profile_name()?;
        let wanted = requested
            .map(str::to_owned)
            .or_else(|| self.config.profiles.get(&name).and_then(|p| p.site.clone()))
            .ok_or_else(|| {
                anyhow!("nie wiem, której witryny użyć: podaj --site albo ustaw `nexo profile site <klucz>`")
            })?;
        if wanted.starts_with("site_") {
            return Ok(wanted);
        }
        let sites = client.get("/sites", &[]).await?;
        sites["data"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|site| site["key"].as_str() == Some(wanted.as_str()))
            .and_then(|site| site["id"].as_str().map(str::to_owned))
            .with_context(|| format!("nie ma witryny `{wanted}` (nexo sites list)"))
    }
}

/// `key=value` pairs of `-q` options.
pub fn query_pairs(items: &[String]) -> Result<Vec<(String, String)>> {
    items
        .iter()
        .map(|item| {
            item.split_once('=')
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .ok_or_else(|| anyhow!("`{item}`: parametr ma postać klucz=wartość"))
        })
        .collect()
}

/// Optional query values, skipping empty ones.
pub fn query(items: &[(&str, Option<&str>)]) -> Vec<(String, String)> {
    items
        .iter()
        .filter_map(|(k, v)| {
            v.filter(|v| !v.is_empty())
                .map(|v| ((*k).to_owned(), v.to_owned()))
        })
        .collect()
}

pub fn text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
