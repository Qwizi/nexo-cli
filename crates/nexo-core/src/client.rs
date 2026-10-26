//! Calls to the API of one profile. The access token lives 15 minutes; it is
//! refreshed ahead of time under a file lock, because the API ends the whole
//! session when the same refresh token is presented twice (two `nexo`
//! commands running at once must not both refresh).

use std::path::PathBuf;
use std::time::Duration;

use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::config::Paths;
use crate::tokens::{TokenStore, Tokens};
use crate::{CMS_VERSION, Error, Result, user_agent};

/// Request body, kept so a request can be sent again after a refresh.
#[derive(Debug, Clone)]
pub enum Body {
    None,
    Json(Value),
    /// `multipart/form-data`: text fields and one file.
    Multipart {
        fields: Vec<(String, String)>,
        file: FilePart,
    },
}

#[derive(Debug, Clone)]
pub struct FilePart {
    pub field: String,
    pub filename: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

pub struct Client {
    http: reqwest::Client,
    /// `https://cms.example.pl/api/v1`
    api: String,
    profile: String,
    store: TokenStore,
    lock: PathBuf,
    tokens: Mutex<Option<Tokens>>,
}

/// The HTTP client every part of the CLI uses.
pub fn http() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(user_agent())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .build()?)
}

pub fn api_base(panel_url: &str) -> String {
    format!("{}/api/v1", panel_url.trim_end_matches('/'))
}

impl Client {
    pub fn new(paths: &Paths, profile: &str, panel_url: &str) -> Result<Self> {
        Ok(Self {
            http: http()?,
            api: api_base(panel_url),
            profile: profile.to_owned(),
            store: TokenStore::new(paths),
            lock: paths.lock_file(profile),
            tokens: Mutex::new(None),
        })
    }

    pub fn profile(&self) -> &str {
        &self.profile
    }

    pub fn store(&self) -> &TokenStore {
        &self.store
    }

    /// JSON answer of a call; API errors become [`Error::Api`].
    pub async fn json(
        &self,
        method: Method,
        path: &str,
        query: &[(String, String)],
        body: Body,
    ) -> Result<Value> {
        let response = self.send(method, path, query, &body).await?;
        let status = response.status();
        let bytes = response.bytes().await?;
        if !status.is_success() {
            return Err(api_error(status, &bytes));
        }
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub async fn get(&self, path: &str, query: &[(String, String)]) -> Result<Value> {
        self.json(Method::GET, path, query, Body::None).await
    }

    pub async fn post(&self, path: &str, body: Value) -> Result<Value> {
        self.json(Method::POST, path, &[], Body::Json(body)).await
    }

    pub async fn put(&self, path: &str, body: Value) -> Result<Value> {
        self.json(Method::PUT, path, &[], Body::Json(body)).await
    }

    /// Raw bytes of a call (file downloads).
    pub async fn bytes(&self, path: &str) -> Result<Vec<u8>> {
        let response = self.send(Method::GET, path, &[], &Body::None).await?;
        let status = response.status();
        let bytes = response.bytes().await?;
        if !status.is_success() {
            return Err(api_error(status, &bytes));
        }
        Ok(bytes.to_vec())
    }

    /// Every item of a cursor-paginated list, up to `max`.
    pub async fn list_all(
        &self,
        path: &str,
        query: &[(String, String)],
        max: usize,
    ) -> Result<Vec<Value>> {
        let mut items = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut q = query.to_vec();
            q.push(("limit".into(), "100".into()));
            if let Some(c) = &cursor {
                q.push(("cursor".into(), c.clone()));
            }
            let page = self.get(path, &q).await?;
            if let Some(data) = page["data"].as_array() {
                items.extend(data.iter().cloned());
            }
            cursor = page["next_cursor"].as_str().map(str::to_owned);
            if items.len() >= max || page["has_more"] != json!(true) || cursor.is_none() {
                break;
            }
        }
        items.truncate(max);
        Ok(items)
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        query: &[(String, String)],
        body: &Body,
    ) -> Result<reqwest::Response> {
        let token = self.access_token(false).await?;
        let response = self
            .request(&method, path, query, body, &token)
            .send()
            .await?;
        if response.status() != StatusCode::UNAUTHORIZED {
            return Ok(response);
        }
        // Expired early or rotated by another process: refresh once and retry.
        let token = self.access_token(true).await?;
        Ok(self
            .request(&method, path, query, body, &token)
            .send()
            .await?)
    }

    fn request(
        &self,
        method: &Method,
        path: &str,
        query: &[(String, String)],
        body: &Body,
        token: &str,
    ) -> reqwest::RequestBuilder {
        let path = if path.starts_with('/') {
            path.to_owned()
        } else {
            format!("/{path}")
        };
        let mut request = self
            .http
            .request(method.clone(), format!("{}{path}", self.api))
            .bearer_auth(token)
            .header("cms-version", CMS_VERSION)
            .query(query);
        match body {
            Body::None => {}
            Body::Json(value) => request = request.json(value),
            Body::Multipart { fields, file } => {
                let mut form = reqwest::multipart::Form::new();
                for (key, value) in fields {
                    form = form.text(key.clone(), value.clone());
                }
                let part = reqwest::multipart::Part::bytes(file.bytes.clone())
                    .file_name(file.filename.clone())
                    .mime_str(&file.mime)
                    .unwrap_or_else(|_| reqwest::multipart::Part::bytes(file.bytes.clone()));
                form = form.part(file.field.clone(), part);
                request = request.multipart(form);
            }
        }
        request
    }

    /// A valid access token, refreshed when it is about to expire (or when
    /// `force`, after the API refused it).
    pub async fn access_token(&self, force: bool) -> Result<String> {
        let mut cached = self.tokens.lock().await;
        if cached.is_none() {
            *cached = self.store.load(&self.profile)?;
        }
        let tokens = cached
            .clone()
            .ok_or_else(|| Error::NotSignedIn(self.profile.clone()))?;
        if !force && tokens.fresh(30) {
            return Ok(tokens.access_token);
        }
        let refreshed = self.refresh(&tokens).await?;
        let token = refreshed.access_token.clone();
        *cached = Some(refreshed);
        Ok(token)
    }

    async fn refresh(&self, known: &Tokens) -> Result<Tokens> {
        let _lock = lock(self.lock.clone()).await?;
        // Another process may have refreshed while this one waited.
        if let Some(stored) = self.store.load(&self.profile)? {
            if stored.refresh_token != known.refresh_token && stored.fresh(30) {
                return Ok(stored);
            }
            if stored.refresh_token != known.refresh_token {
                return self.rotate(stored).await;
            }
        }
        self.rotate(known.clone()).await
    }

    async fn rotate(&self, tokens: Tokens) -> Result<Tokens> {
        let response = self
            .http
            .post(format!("{}/auth/refresh", self.api))
            .header("cms-version", CMS_VERSION)
            .json(&json!({ "refresh_token": tokens.refresh_token }))
            .send()
            .await?;
        let status = response.status();
        let bytes = response.bytes().await?;
        if status == StatusCode::UNAUTHORIZED {
            self.store.delete(&self.profile)?;
            return Err(Error::SessionEnded);
        }
        if !status.is_success() {
            return Err(api_error(status, &bytes));
        }
        let body: Value = serde_json::from_slice(&bytes)?;
        let refreshed = tokens_from(&body["data"], tokens.email.clone())?;
        self.store.save(&self.profile, &refreshed)?;
        Ok(refreshed)
    }
}

/// Exclusive lock on a file, released when the returned handle is dropped.
async fn lock(path: PathBuf) -> Result<std::fs::File> {
    tokio::task::spawn_blocking(move || -> Result<std::fs::File> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)?;
        file.lock()?;
        Ok(file)
    })
    .await
    .map_err(|e| Error::Other(e.to_string()))?
}

/// Tokens from a `token_pair` answer.
pub fn tokens_from(data: &Value, email: Option<String>) -> Result<Tokens> {
    let field = |name: &str| {
        data[name]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| Error::Other(format!("odpowiedź bez {name}")))
    };
    let time = |name: &str| -> Result<chrono::DateTime<chrono::Utc>> {
        field(name)?
            .parse()
            .map_err(|_| Error::Other(format!("błędne {name}")))
    };
    Ok(Tokens {
        access_token: field("access_token")?,
        access_expires_at: time("access_expires_at")?,
        refresh_token: field("refresh_token")?,
        refresh_expires_at: time("refresh_expires_at")?,
        email: data["user"]["email"].as_str().map(str::to_owned).or(email),
    })
}

/// The API's error envelope as an [`Error`].
pub fn api_error(status: StatusCode, body: &[u8]) -> Error {
    let parsed: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let error = &parsed["error"];
    let mut message = error["message"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| status.canonical_reason().unwrap_or("error").to_owned());
    if let Some(param) = error["param"].as_str() {
        message = format!("{message} [{param}]");
    }
    Error::Api {
        status: status.as_u16(),
        code: error["code"].as_str().unwrap_or("unknown").to_owned(),
        message,
    }
}

/// Media type of a file from its name (the API checks the contents anyway).
pub fn mime_of(filename: &str) -> String {
    mime_guess::from_path(filename)
        .first_or_octet_stream()
        .essence_str()
        .to_owned()
}
