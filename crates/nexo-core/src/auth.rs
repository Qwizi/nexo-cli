//! Device sign-in (RFC 8628): the CLI shows a code, the person approves it
//! in the panel, the CLI polls until it gets a session.

use std::time::Duration;

use serde_json::{Value, json};

use crate::client::{api_base, api_error, tokens_from};
use crate::tokens::Tokens;
use crate::{CMS_VERSION, Error, Result};

#[derive(Debug, Clone)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    pub interval: u64,
}

pub async fn start(
    http: &reqwest::Client,
    panel_url: &str,
    device_name: &str,
) -> Result<DeviceCode> {
    let response = http
        .post(format!("{}/auth/device/code", api_base(panel_url)))
        .header("cms-version", CMS_VERSION)
        .json(&json!({ "client_name": device_name }))
        .send()
        .await?;
    let status = response.status();
    let bytes = response.bytes().await?;
    if !status.is_success() {
        return Err(api_error(status, &bytes));
    }
    let body: Value = serde_json::from_slice(&bytes)?;
    let data = &body["data"];
    let text = |name: &str| data[name].as_str().unwrap_or_default().to_owned();
    Ok(DeviceCode {
        device_code: text("device_code"),
        user_code: text("user_code"),
        verification_uri: text("verification_uri"),
        verification_uri_complete: text("verification_uri_complete"),
        expires_in: data["expires_in"].as_u64().unwrap_or(600),
        interval: data["interval"].as_u64().unwrap_or(5),
    })
}

/// Polls until the person decides or the code expires.
pub async fn wait(http: &reqwest::Client, panel_url: &str, code: &DeviceCode) -> Result<Tokens> {
    let mut interval = code.interval.max(1);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(code.expires_in);
    loop {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        if tokio::time::Instant::now() > deadline {
            return Err(Error::Other(
                "kod wygasł; uruchom `nexo login` ponownie".into(),
            ));
        }
        let response = http
            .post(format!("{}/auth/device/token", api_base(panel_url)))
            .header("cms-version", CMS_VERSION)
            .json(&json!({ "device_code": code.device_code }))
            .send()
            .await?;
        let status = response.status();
        let bytes = response.bytes().await?;
        if status.is_success() {
            let body: Value = serde_json::from_slice(&bytes)?;
            return tokens_from(&body["data"], None);
        }
        let error = api_error(status, &bytes);
        match error.code() {
            Some("authorization_pending") => {}
            Some("slow_down") => interval += 5,
            Some("access_denied") => {
                return Err(Error::Other("logowanie odrzucono w panelu".into()));
            }
            Some("expired_token") => {
                return Err(Error::Other(
                    "kod wygasł; uruchom `nexo login` ponownie".into(),
                ));
            }
            _ => return Err(error),
        }
    }
}

/// A name for this machine, shown in the panel: the host name when known.
pub fn device_name() -> String {
    let host = std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .and_then(|out| String::from_utf8(out.stdout).ok())
        })
        .map(|h| h.trim().to_owned())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "komputer".to_owned());
    host.chars().filter(|c| !c.is_control()).take(64).collect()
}
