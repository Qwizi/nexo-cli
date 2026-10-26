//! Product photos re-imagined by Gemini from the real photo (image editing,
//! not text-to-image): a lived-in room or a warm catalogue packshot. The
//! built-in prompts are generic; a shop keeps its own in the prompts folder
//! (`nexo images set-prompt`). Results keep the `__cr-<preset>-<hash>`
//! naming of the `cr` tool, so a generated photo is never used as a source.

use std::path::Path;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use crate::config::ImagesConfig;
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    LifestyleRoom,
    WarmPackshot,
}

impl Preset {
    pub const ALL: [Preset; 2] = [Preset::LifestyleRoom, Preset::WarmPackshot];

    pub fn slug(self) -> &'static str {
        match self {
            Self::LifestyleRoom => "lifestyle_room",
            Self::WarmPackshot => "warm_packshot",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let value = value.replace('-', "_");
        Self::ALL.into_iter().find(|p| p.slug() == value)
    }

    fn builtin(self) -> &'static str {
        match self {
            Self::LifestyleRoom => include_str!("../../../prompts/lifestyle_room.md"),
            Self::WarmPackshot => include_str!("../../../prompts/warm_packshot.md"),
        }
    }

    /// The prompt: `<prompts dir>/<slug>.md` when the user wrote one, else the
    /// built-in text.
    pub fn prompt(self, prompts_dir: &Path) -> String {
        std::fs::read_to_string(prompts_dir.join(format!("{}.md", self.slug())))
            .ok()
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| self.builtin().to_owned())
    }
}

/// A file the tool generated (never used as a source).
pub fn is_generated(filename: &str) -> bool {
    filename.contains("__cr-")
}

/// `kredens-main__cr-lifestyle-room-1a2b3c4d.jpg`: source name, preset and a
/// hash of preset and source bytes (the same input gives the same name, which
/// lets a rerun skip work already done).
pub fn output_name(source_filename: &str, preset: Preset, source: &[u8]) -> String {
    let stem = Path::new(source_filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("photo");
    let stem: String = stem
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let mut hasher = blake3::Hasher::new();
    hasher.update(preset.slug().as_bytes());
    hasher.update(source);
    let hash = hasher.finalize().to_hex();
    format!(
        "{}__cr-{}-{}.jpg",
        if stem.is_empty() { "photo" } else { &stem },
        preset.slug().replace('_', "-"),
        &hash[..8]
    )
}

/// Decodes whatever the model returned and saves it as JPEG.
pub fn to_jpeg(bytes: &[u8], quality: u8) -> Result<Vec<u8>> {
    let image = image::load_from_memory(bytes).map_err(|e| Error::Other(format!("image: {e}")))?;
    let mut out = Vec::new();
    let encoder =
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
    image
        .to_rgb8()
        .write_with_encoder(encoder)
        .map_err(|e| Error::Other(format!("image: {e}")))?;
    Ok(out)
}

pub struct Gemini {
    http: reqwest::Client,
    key: String,
    config: ImagesConfig,
    base: String,
}

impl Gemini {
    pub fn new(http: reqwest::Client, key: String, config: ImagesConfig) -> Self {
        Self {
            http,
            key,
            config,
            base: "https://generativelanguage.googleapis.com".into(),
        }
    }

    pub fn with_base(mut self, base: &str) -> Self {
        self.base = base.trim_end_matches('/').to_owned();
        self
    }

    /// The product photo edited by the prompt. Retries rate limits and
    /// server errors (up to four attempts, backing off).
    pub async fn edit(&self, prompt: &str, source: &[u8], mime: &str) -> Result<Vec<u8>> {
        let body = json!({
            "contents": [{ "parts": [
                { "text": prompt },
                { "inline_data": { "mime_type": mime, "data": STANDARD.encode(source) } }
            ]}],
            "generationConfig": {
                "responseModalities": ["IMAGE"],
                "imageConfig": {
                    "aspectRatio": self.config.aspect_ratio,
                    "imageSize": self.config.image_size
                }
            }
        });
        let url = format!(
            "{}/v1beta/models/{}:generateContent",
            self.base, self.config.model
        );
        let mut delay = Duration::from_secs(2);
        let mut attempt = 0;
        loop {
            attempt += 1;
            let response = self
                .http
                .post(&url)
                // In a header, not the URL, so it never lands in logs.
                .header("x-goog-api-key", &self.key)
                .json(&body)
                .send()
                .await;
            let retry = match response {
                Ok(response) => {
                    let status = response.status();
                    let wait = response
                        .headers()
                        .get(reqwest::header::RETRY_AFTER)
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<u64>().ok())
                        .map(Duration::from_secs);
                    let value: Value = response.json().await.unwrap_or(Value::Null);
                    if status.is_success() {
                        return image_of(&value);
                    }
                    let message = value["error"]["message"]
                        .as_str()
                        .unwrap_or("no message")
                        .to_owned();
                    if status.as_u16() == 429 || status.is_server_error() {
                        Some((wait, format!("Gemini {status}: {message}")))
                    } else {
                        return Err(Error::Other(format!("Gemini {status}: {message}")));
                    }
                }
                Err(e) if e.is_timeout() || e.is_connect() => Some((None, format!("Gemini: {e}"))),
                Err(e) => return Err(e.into()),
            };
            if let Some((wait, message)) = retry {
                if attempt >= 4 {
                    return Err(Error::Other(message));
                }
                tokio::time::sleep(wait.unwrap_or(delay).min(Duration::from_secs(60))).await;
                delay *= 2;
            }
        }
    }
}

fn image_of(value: &Value) -> Result<Vec<u8>> {
    let parts = value["candidates"][0]["content"]["parts"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for part in parts {
        let data = part["inline_data"]["data"]
            .as_str()
            .or_else(|| part["inlineData"]["data"].as_str());
        if let Some(data) = data {
            return STANDARD
                .decode(data)
                .map_err(|e| Error::Other(format!("Gemini image: {e}")));
        }
    }
    let reason = value["candidates"][0]["finishReason"]
        .as_str()
        .or_else(|| value["promptFeedback"]["blockReason"].as_str())
        .unwrap_or("no image in the answer");
    Err(Error::Other(format!(
        "Gemini nie zwrócił zdjęcia ({reason})"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn names_results_like_cr() {
        let name = output_name("ST907_01-1.webp", Preset::WarmPackshot, b"abc");
        assert!(name.starts_with("st907-01-1__cr-warm-packshot-"), "{name}");
        assert!(name.ends_with(".jpg"));
        assert_eq!(
            name,
            output_name("ST907_01-1.webp", Preset::WarmPackshot, b"abc")
        );
        assert_ne!(
            name,
            output_name("ST907_01-1.webp", Preset::LifestyleRoom, b"abc")
        );
        assert!(is_generated(&name));
        assert!(!is_generated("ST907_01-1.webp"));
    }

    #[test]
    fn parses_presets() {
        assert_eq!(Preset::parse("lifestyle-room"), Some(Preset::LifestyleRoom));
        assert_eq!(Preset::parse("warm_packshot"), Some(Preset::WarmPackshot));
        assert_eq!(Preset::parse("other"), None);
    }

    #[test]
    fn user_prompts_override_builtin() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Preset::WarmPackshot.prompt(dir.path()).len() > 100);
        std::fs::write(dir.path().join("warm_packshot.md"), "mój prompt").unwrap();
        assert_eq!(Preset::WarmPackshot.prompt(dir.path()), "mój prompt");
    }

    fn png() -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(4, 4, image::Rgb([200, 150, 100]))
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[tokio::test]
    async fn edits_with_the_key_in_a_header_and_retries() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1beta/models/m:generateContent"))
            .respond_with(
                ResponseTemplate::new(503).set_body_json(json!({"error": {"message": "busy"}})),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1beta/models/m:generateContent"))
            .and(header("x-goog-api-key", "k"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{ "content": { "parts": [
                    { "inlineData": { "mimeType": "image/png", "data": STANDARD.encode(png()) } }
                ]}}]
            })))
            .mount(&server)
            .await;
        let config = ImagesConfig {
            model: "m".into(),
            ..ImagesConfig::default()
        };
        let gemini =
            Gemini::new(reqwest::Client::new(), "k".into(), config).with_base(&server.uri());
        let bytes = gemini.edit("p", b"src", "image/jpeg").await.unwrap();
        let jpeg = to_jpeg(&bytes, 90).unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    }
}
