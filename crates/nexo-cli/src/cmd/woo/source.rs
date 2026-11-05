//! Read-only WooCommerce REST client (`/wp-json/wc/v3`). The keys go in the
//! `Authorization` header only; nothing here writes to the shop.

use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;

pub struct Woo {
    http: reqwest::Client,
    base: String,
    key: String,
    secret: String,
}

impl Woo {
    pub fn new(url: &str, key: &str, secret: &str) -> Result<Self> {
        let url = url.trim().trim_end_matches('/');
        if !url.starts_with("https://") && !url.starts_with("http://") {
            bail!("--url: adres sklepu z https://");
        }
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("nexo-cli/", env!("CARGO_PKG_VERSION"), " (import)"))
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(120))
                .build()?,
            base: format!("{url}/wp-json/wc/v3"),
            key: key.to_owned(),
            secret: secret.to_owned(),
        })
    }

    /// The shop's host (`sklep.example.pl`).
    pub fn host(&self) -> &str {
        self.base
            .split("//")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .unwrap_or_default()
    }

    async fn page<T: DeserializeOwned>(&self, path: &str, page: u32) -> Result<(Vec<T>, u32)> {
        let separator = if path.contains('?') { '&' } else { '?' };
        let response = self
            .http
            .get(format!(
                "{}/{path}{separator}per_page=100&page={page}",
                self.base
            ))
            .basic_auth(&self.key, Some(&self.secret))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("WooCommerce {path}: {}", e.without_url()))?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            bail!("WooCommerce {path}: klucze REST API nie pasują albo nie mają prawa odczytu");
        }
        if !status.is_success() {
            bail!("WooCommerce {path}: HTTP {status}");
        }
        let pages = response
            .headers()
            .get("x-wp-totalpages")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        let items = response
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("WooCommerce {path}: {}", e.without_url()))?;
        Ok((items, pages))
    }

    /// Every page of a list endpoint.
    pub async fn all<T: DeserializeOwned>(&self, path: &str) -> Result<Vec<T>> {
        let (mut items, pages) = self.page(path, 1).await?;
        for page in 2..=pages {
            items.extend(self.page::<T>(path, page).await?.0);
        }
        Ok(items)
    }

    /// A public file of the shop (product photos): bytes and content type.
    pub async fn download(&self, url: &str) -> Result<(Vec<u8>, Option<String>)> {
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("zdjęcie: {}", e.without_url()))?;
        if !response.status().is_success() {
            bail!("zdjęcie: HTTP {}", response.status());
        }
        let mime = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let bytes = response.bytes().await.context("zdjęcie")?;
        Ok((bytes.to_vec(), mime))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Category {
    pub id: u64,
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub parent: u64,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Attribute {
    pub id: u64,
    pub name: String,
    pub slug: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AttributeTerm {
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Ref {
    pub id: u64,
}

/// An attribute of a product: global ones have an id, local ones 0.
#[derive(Debug, Clone, Deserialize)]
pub struct ProductAttribute {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub options: Vec<String>,
}

/// The value a variation takes of an attribute.
#[derive(Debug, Clone, Deserialize)]
pub struct VariationAttribute {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub option: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Image {
    pub id: u64,
    pub src: String,
    #[serde(default)]
    pub alt: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Dimensions {
    #[serde(default)]
    pub length: String,
    #[serde(default)]
    pub width: String,
    #[serde(default)]
    pub height: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Meta {
    pub key: String,
    #[serde(default)]
    pub value: serde_json::Value,
}

/// What a product and a variation share: what becomes a variant.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Offer {
    pub id: u64,
    #[serde(default)]
    pub sku: String,
    #[serde(default)]
    pub regular_price: String,
    #[serde(default)]
    pub price: String,
    #[serde(default)]
    pub sale_price: String,
    #[serde(default)]
    pub date_on_sale_from_gmt: Option<String>,
    #[serde(default)]
    pub date_on_sale_to_gmt: Option<String>,
    /// `true`, `false` or `"parent"` (a variation following its product).
    #[serde(default)]
    pub manage_stock: serde_json::Value,
    #[serde(default)]
    pub stock_quantity: Option<i64>,
    #[serde(default)]
    pub stock_status: String,
    #[serde(default)]
    pub weight: String,
    #[serde(default)]
    pub dimensions: Dimensions,
    #[serde(default)]
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Product {
    #[serde(flatten)]
    pub offer: Offer,
    pub name: String,
    pub slug: String,
    /// `simple`, `variable`, `grouped`, `external`.
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub catalog_visibility: String,
    /// WooCommerce sells it (it has a price); one it cannot sell it keeps
    /// out of its catalogue.
    #[serde(default = "yes")]
    pub purchasable: bool,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub short_description: String,
    #[serde(default)]
    pub shipping_class: String,
    #[serde(default)]
    pub categories: Vec<Ref>,
    #[serde(default)]
    pub attributes: Vec<ProductAttribute>,
    #[serde(default)]
    pub images: Vec<Image>,
    #[serde(default)]
    pub meta_data: Vec<Meta>,
    #[serde(default)]
    pub yoast_head_json: Option<serde_json::Value>,
    #[serde(default)]
    pub date_modified_gmt: String,
}

impl Product {
    pub fn meta(&self, key: &str) -> Option<&serde_json::Value> {
        self.meta_data
            .iter()
            .find(|m| m.key == key)
            .map(|m| &m.value)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Variation {
    #[serde(flatten)]
    pub offer: Offer,
    #[serde(default)]
    pub attributes: Vec<VariationAttribute>,
    #[serde(default)]
    pub image: Option<Image>,
    #[serde(default)]
    pub date_modified_gmt: String,
}

fn yes() -> bool {
    true
}
