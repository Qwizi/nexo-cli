//! `nexo shop import woo`: a WooCommerce shop into the Sklep extension, through
//! the CMS API only. Categories and attributes become terms, products become
//! documents of kind `product` with their shop data (variants, prices, stock,
//! sizes), photos go to the media library.
//!
//! A run can be repeated: terms are matched by slug, products by address,
//! and what was imported is remembered in a state file next to the CLI's
//! settings (product versions and uploaded photos), so unchanged products
//! and photos are skipped. Nothing is written to the WooCommerce shop.

mod html;
mod source;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use nexo_core::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cli::{WooImport, WooPhotos};
use crate::cmd::content::upload_bytes;
use crate::ctx::{Ctx, text};
use source::{Offer, Product, Variation, Woo};

/// Images per gallery (the shop's `gallery` field).
const MAX_GALLERY: usize = 24;
const MAX_TABS: usize = 12;
const MAX_VARIANTS: usize = 100;
const PLACEHOLDER: &[u8] = include_bytes!("../../../assets/placeholder.png");

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    placeholder: Option<String>,
    /// WooCommerce image id → asset id.
    #[serde(default)]
    media: BTreeMap<String, String>,
    /// WooCommerce product id → what was imported.
    #[serde(default)]
    products: BTreeMap<String, Imported>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Imported {
    document: String,
    version: String,
    photos: String,
}

impl State {
    fn load(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("{}: uszkodzony plik stanu importu", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("nie mogę przeczytać {}", path.display())),
        }
    }

    fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&temp, path)?;
        Ok(())
    }
}

#[derive(Debug, Default)]
struct Report {
    terms: usize,
    media: usize,
    created: usize,
    updated: usize,
    unchanged: usize,
    published: usize,
    warnings: Vec<String>,
    failed: Vec<String>,
}

/// What a run needs at hand.
struct Run<'a> {
    client: &'a Client,
    woo: &'a Woo,
    site: String,
    locale: String,
    options: &'a WooImport,
    hosts: Vec<String>,
    state_path: PathBuf,
    state: State,
    report: Report,
}

pub async fn run(ctx: &Ctx, options: WooImport) -> Result<()> {
    let client = ctx.client()?;
    let site = ctx.site_id(&client, options.site.site.as_deref()).await?;
    let locale = match &options.locale {
        Some(locale) => locale.clone(),
        None => text(&client.get(&format!("/sites/{site}"), &[]).await?["data"]["default_locale"]),
    };
    let woo = Woo::new(&options.url, &options.key, &options.secret)?;
    let host = woo.host().to_lowercase();
    let bare = host.strip_prefix("www.").unwrap_or(&host).to_owned();
    let mut hosts = vec![bare.clone(), format!("www.{bare}")];
    hosts.extend(options.own_host.iter().map(|h| h.to_lowercase()));
    let state_path = options.state.clone().unwrap_or_else(|| {
        ctx.paths
            .dir
            .join("imports")
            .join(format!("woo-{site}-{}.json", slugify(&bare)))
    });
    let state = State::load(&state_path)?;
    let mut run = Run {
        client: &client,
        woo: &woo,
        site,
        locale,
        options: &options,
        hosts,
        state_path,
        state,
        report: Report::default(),
    };
    eprintln!(
        "WooCommerce {host} → witryna {} ({}).",
        run.site, run.locale
    );

    let mut products: Vec<Product> = woo.all("products?status=publish").await?;
    products.sort_by_key(|p| p.offer.id);
    if let Some(limit) = options.limit {
        products = sample(products, limit);
    }
    eprintln!("Produkty do przejrzenia: {}.", products.len());
    if options.dry_run {
        run.dry_run(&products).await?;
    } else {
        run.import(&products).await?;
    }
    run.print()
}

impl Run<'_> {
    async fn dry_run(&mut self, products: &[Product]) -> Result<()> {
        let existing = self.documents().await?;
        let mut created = 0;
        for product in products {
            let variations = self.variations(product).await?;
            let input = variants_of(product, &variations, &BTreeMap::new(), &[]);
            if let Err(problem) = check(&input) {
                self.report
                    .warnings
                    .push(format!("{}: {problem}", product.slug));
            }
            if product.images.len() > MAX_GALLERY {
                self.report.warnings.push(format!(
                    "{}: {} zdjęć, wejdzie pierwsze {MAX_GALLERY}",
                    product.slug,
                    product.images.len()
                ));
            }
            if existing.contains_key(&slugify(&product.slug)) {
                self.report.updated += 1;
            } else {
                created += 1;
            }
        }
        self.report.created = created;
        eprintln!("Próba: nic nie zapisano.");
        Ok(())
    }

    async fn import(&mut self, products: &[Product]) -> Result<()> {
        let terms = self.terms().await?;
        eprintln!(
            "Kategorie i atrybuty gotowe (nowych: {}).",
            self.report.terms
        );
        let documents = self.documents().await?;
        let bar = indicatif::ProgressBar::new(products.len() as u64);
        for product in products {
            bar.set_message(product.slug.clone());
            if let Err(error) = self.product(product, &terms, &documents).await {
                self.report
                    .failed
                    .push(format!("{}: {error:#}", product.slug));
            }
            bar.inc(1);
        }
        bar.finish_and_clear();
        Ok(())
    }

    fn print(&self) -> Result<()> {
        let r = &self.report;
        for warning in &r.warnings {
            eprintln!("! {warning}");
        }
        for failure in &r.failed {
            eprintln!("✗ {failure}");
        }
        if self.options.dry_run {
            eprintln!(
                "Do utworzenia: {}, do aktualizacji: {}, ostrzeżeń: {}.",
                r.created,
                r.updated,
                r.warnings.len()
            );
        } else {
            eprintln!(
                "Nowe: {}, zaktualizowane: {}, bez zmian: {}, opublikowane: {}, terminy: {}, zdjęcia: {}.",
                r.created, r.updated, r.unchanged, r.published, r.terms, r.media
            );
            eprintln!("Stan importu: {}", self.state_path.display());
        }
        if !r.failed.is_empty() {
            bail!("{} produktów nie weszło", r.failed.len());
        }
        Ok(())
    }

    // --- terms -----------------------------------------------------------------

    async fn term_list(&self, taxonomy: &str) -> Result<Vec<Value>> {
        let list = self
            .client
            .get(
                "/terms",
                &[
                    ("site".into(), self.site.clone()),
                    ("locale".into(), self.locale.clone()),
                    ("taxonomy".into(), taxonomy.into()),
                ],
            )
            .await?;
        Ok(list["data"].as_array().cloned().unwrap_or_default())
    }

    /// The term of `slug` under `parent`, created when missing. A slug taken
    /// under another parent (two attributes with a value „czarny”) gets the
    /// parent's slug in front.
    #[allow(clippy::too_many_arguments)]
    async fn term(
        &mut self,
        existing: &mut Vec<Value>,
        taxonomy: &str,
        name: &str,
        slug: &str,
        parent: Option<(&str, &str)>,
        description: &str,
    ) -> Result<String> {
        let parent_id = parent.map(|(id, _)| id);
        let same =
            |t: &Value, slug: &str| t["slug"] == slug && t["parent_id"].as_str() == parent_id;
        let mut slug = slug.to_owned();
        if let Some(found) = existing.iter().find(|t| same(t, &slug)) {
            return Ok(text(&found["id"]));
        }
        if existing.iter().any(|t| t["slug"] == slug.as_str()) {
            let prefix = parent.map_or("atrybut", |(_, s)| s);
            slug = clip(&format!("{prefix}-{slug}"), 96);
            if let Some(found) = existing.iter().find(|t| same(t, &slug)) {
                return Ok(text(&found["id"]));
            }
        }
        let mut body = json!({
            "site_id": self.site, "locale": self.locale, "taxonomy": taxonomy,
            "name": clip(name, 120), "slug": slug, "parent_id": parent_id,
        });
        if !description.is_empty() {
            body["description"] = json!(clip(description, 500));
        }
        let created = self
            .client
            .post("/terms", body)
            .await
            .with_context(|| format!("termin {slug}"))?;
        self.report.terms += 1;
        let term = created["data"].clone();
        let id = text(&term["id"]);
        existing.push(term);
        Ok(id)
    }

    async fn terms(&mut self) -> Result<Terms> {
        let mut existing = self.term_list("product_category").await?;
        let mut categories: Vec<source::Category> = self.woo.all("products/categories").await?;
        // Parents before their children.
        let depth = {
            let parents: HashMap<u64, u64> = categories.iter().map(|c| (c.id, c.parent)).collect();
            move |mut id: u64| {
                let mut depth = 0;
                while let Some(&parent) = parents.get(&id) {
                    if parent == 0 || depth > 16 {
                        break;
                    }
                    id = parent;
                    depth += 1;
                }
                depth
            }
        };
        categories.sort_by_key(|c| (depth(c.id), c.id));
        let mut by_id: HashMap<u64, (String, String)> = HashMap::new();
        for category in &categories {
            if matches!(category.slug.as_str(), "uncategorized" | "bez-kategorii") {
                continue;
            }
            let slug = slugify(&category.slug);
            let parent = by_id.get(&category.parent).cloned();
            let id = self
                .term(
                    &mut existing,
                    "product_category",
                    &html::text(&category.name),
                    &slug,
                    parent
                        .as_ref()
                        .map(|(id, slug)| (id.as_str(), slug.as_str())),
                    &html::text(&category.description),
                )
                .await?;
            by_id.insert(category.id, (id, slug));
        }

        let mut existing = self.term_list("product_attribute").await?;
        let attributes: Vec<source::Attribute> = self.woo.all("products/attributes").await?;
        let mut values = HashMap::new();
        for attribute in &attributes {
            let slug = slugify(attribute.slug.trim_start_matches("pa_"));
            let parent = self
                .term(
                    &mut existing,
                    "product_attribute",
                    &html::text(&attribute.name),
                    &slug,
                    None,
                    "",
                )
                .await?;
            let terms: Vec<source::AttributeTerm> = self
                .woo
                .all(&format!("products/attributes/{}/terms", attribute.id))
                .await?;
            for term in &terms {
                let name = html::text(&term.name);
                let id = self
                    .term(
                        &mut existing,
                        "product_attribute",
                        &name,
                        &slugify(&term.slug),
                        Some((&parent, &slug)),
                        &html::text(&term.description),
                    )
                    .await?;
                values.insert((attribute.id, name), id);
            }
        }
        Ok(Terms {
            categories: by_id.into_iter().map(|(k, (id, _))| (k, id)).collect(),
            values,
        })
    }

    // --- products --------------------------------------------------------------

    /// Product documents of the site in the import's language: slug → id.
    async fn documents(&self) -> Result<HashMap<String, String>> {
        let rows = self
            .client
            .list_all(
                "/documents",
                &[
                    ("site".into(), self.site.clone()),
                    ("kind".into(), "product".into()),
                    ("locale".into(), self.locale.clone()),
                ],
                1_000_000,
            )
            .await?;
        Ok(rows
            .iter()
            .map(|d| (text(&d["slug"]), text(&d["id"])))
            .collect())
    }

    async fn variations(&self, product: &Product) -> Result<Vec<Variation>> {
        if product.kind != "variable" {
            return Ok(Vec::new());
        }
        let mut variations: Vec<Variation> = self
            .woo
            .all(&format!("products/{}/variations", product.offer.id))
            .await?;
        variations.retain(|v| v.offer.status.is_empty() || v.offer.status == "publish");
        variations.sort_by_key(|v| v.offer.id);
        Ok(variations)
    }

    async fn product(
        &mut self,
        product: &Product,
        terms: &Terms,
        documents: &HashMap<String, String>,
    ) -> Result<()> {
        let source = product.offer.id.to_string();
        let variations = self.variations(product).await?;
        let version = std::iter::once(product.date_modified_gmt.as_str())
            .chain(variations.iter().map(|v| v.date_modified_gmt.as_str()))
            .collect::<Vec<_>>()
            .join("|");
        let photos = photos_name(self.options.photos);
        let slug = slugify(&product.slug);
        let previous = self.state.products.get(&source).cloned();
        if let Some(done) = &previous
            && done.version == version
            && (done.photos == photos || done.photos == "all")
            && !self.options.force
            && documents.get(&slug).is_none_or(|id| *id == done.document)
        {
            self.report.unchanged += 1;
            return Ok(());
        }
        let title = clip(&html::text(&product.name), 200);

        // The document: the one imported before, else one at the same address.
        let known = previous
            .as_ref()
            .map(|p| p.document.clone())
            .or_else(|| documents.get(&slug).cloned());
        let document = match &known {
            Some(id) => match self.client.get(&format!("/documents/{id}"), &[]).await {
                Ok(document) => Some(document),
                Err(e) if e.code() == Some("not_found") => None,
                Err(e) => return Err(e.into()),
            },
            None => None,
        };
        let is_new = document.is_none();
        let document = match document {
            Some(document) => document,
            None => self
                .client
                .post(
                    "/documents",
                    json!({ "site_id": self.site, "kind": "product", "locale": self.locale,
                            "title": title, "slug": slug }),
                )
                .await
                .context("nowy dokument")?,
        };
        let data = &document["data"];
        let id = text(&data["id"]);

        let gallery = self.gallery(product, &title).await?;
        let mut draft = data["draft"].clone();
        draft["title"] = json!(title);
        draft["slug"] = json!(slug);
        if !draft["fields"].is_object() {
            draft["fields"] = json!({});
        }
        self.fields(&mut draft["fields"], product, terms, gallery);
        if !draft["seo"].is_object() {
            draft["seo"] = json!({});
        }
        draft["seo"]["title"] = json!(seo_title(product, &title, &self.options.strip_title));
        draft["seo"]["description"] = json!(seo_description(product));
        // Hidden in WooCommerce: reachable by its address, not listed.
        draft["seo"]["exclude_from_listings"] = json!(product.catalog_visibility == "hidden");
        let saved = self
            .client
            .put(
                &format!("/documents/{id}/draft"),
                json!({ "revision": data["revision"], "draft": draft }),
            )
            .await
            .context("szkic")?;

        // Shop data: variants keep their ids by SKU.
        let shop_path = format!("/sites/{}/ext/commerce/products/{id}", self.site);
        let current = if is_new {
            Value::Null
        } else {
            self.client
                .get(&shop_path, &[])
                .await
                .unwrap_or(Value::Null)
        };
        let ids: BTreeMap<String, Value> = current["variants"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|v| (text(&v["sku"]).to_lowercase(), v.clone()))
            .collect();
        let images = self.variation_images(&variations).await;
        let variants = variants_of(product, &variations, &ids, &images);
        match check(&variants) {
            Ok(()) => {
                let mut body = json!({
                    "shipping_class": shipping_class(&product.shipping_class),
                    "variants": variants,
                });
                if let Some(rate) = current["tax_rate"].as_i64() {
                    body["tax_rate"] = json!(rate);
                }
                self.client
                    .put(&shop_path, body)
                    .await
                    .context("dane sklepu")?;
            }
            Err(problem) => self
                .report
                .warnings
                .push(format!("{slug}: warianty pominięte: {problem}")),
        }

        if self.options.no_publish {
            // Stays a draft.
        } else {
            let published = self
                .client
                .post(
                    &format!("/documents/{id}/publish"),
                    json!({ "revision": saved["data"]["revision"], "note": "Import WooCommerce (nexo)" }),
                )
                .await
                .context("publikacja")?;
            if published["data"]["status"] == "published" {
                self.report.published += 1;
            } else {
                let issues: Vec<String> = published["data"]["issues"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|i| text(&i["message"]))
                    .collect();
                self.report
                    .warnings
                    .push(format!("{slug}: nie opublikowano: {}", issues.join("; ")));
            }
        }
        if is_new {
            self.report.created += 1;
        } else {
            self.report.updated += 1;
        }
        self.state.products.insert(
            source,
            Imported {
                document: id,
                version,
                photos: photos.into(),
            },
        );
        self.state.save(&self.state_path)
    }

    fn fields(
        &self,
        fields: &mut Value,
        product: &Product,
        terms: &Terms,
        gallery: Option<Vec<Value>>,
    ) {
        let mut set = |key: &str, value: Option<Value>| match value {
            Some(value) => fields[key] = value,
            None => {
                if let Some(map) = fields.as_object_mut() {
                    map.remove(key);
                }
            }
        };
        if let Some(gallery) = gallery {
            set("gallery", Some(json!(gallery)));
        }
        set(
            "summary",
            html::to_doc(&product.short_description, &self.hosts),
        );
        set(
            "description",
            html::to_doc(&product.description, &self.hosts),
        );
        if let Some(key) = &self.options.tabs_meta {
            let tabs = tabs_of(product, key, &self.hosts);
            set("tabs", (!tabs.is_empty()).then(|| json!(tabs)));
        }
        let categories: Vec<&String> = product
            .categories
            .iter()
            .filter_map(|c| terms.categories.get(&c.id))
            .take(5)
            .collect();
        set(
            "category_ids",
            (!categories.is_empty()).then(|| json!(categories)),
        );
        let mut attributes: Vec<&String> = Vec::new();
        for attribute in product.attributes.iter().filter(|a| a.id != 0) {
            for option in &attribute.options {
                if let Some(id) = terms.values.get(&(attribute.id, html::text(option)))
                    && !attributes.contains(&id)
                {
                    attributes.push(id);
                }
            }
        }
        attributes.truncate(48);
        set(
            "attribute_ids",
            (!attributes.is_empty()).then(|| json!(attributes)),
        );
    }

    // --- photos ----------------------------------------------------------------

    /// The gallery the product gets; `None` keeps the one it has.
    async fn gallery(&mut self, product: &Product, title: &str) -> Result<Option<Vec<Value>>> {
        match self.options.photos {
            WooPhotos::None => Ok(None),
            WooPhotos::Placeholder => {
                let asset = self.placeholder().await?;
                let count = product.images.len().clamp(1, MAX_GALLERY);
                Ok(Some(
                    (0..count)
                        .map(|_| json!({ "asset_id": asset, "alt": title }))
                        .collect(),
                ))
            }
            WooPhotos::All => {
                let mut gallery = Vec::new();
                for image in product.images.iter().take(MAX_GALLERY) {
                    match self.photo(image, title).await {
                        Ok(asset) => {
                            let alt = html::text(&image.alt);
                            let mut entry = json!({ "asset_id": asset });
                            if !alt.is_empty() {
                                entry["alt"] = json!(clip(&alt, 300));
                            }
                            gallery.push(entry);
                        }
                        Err(error) => self
                            .report
                            .warnings
                            .push(format!("{}: {error:#}", product.slug)),
                    }
                }
                if product.images.len() > MAX_GALLERY {
                    self.report.warnings.push(format!(
                        "{}: {} zdjęć, weszło pierwsze {MAX_GALLERY}",
                        product.slug,
                        product.images.len()
                    ));
                }
                Ok(Some(gallery))
            }
        }
    }

    /// The asset of a shop photo, uploaded once.
    async fn photo(&mut self, image: &source::Image, title: &str) -> Result<String> {
        let key = image.id.to_string();
        if let Some(asset) = self.state.media.get(&key) {
            return Ok(asset.clone());
        }
        let (bytes, _) = self.woo.download(&image.src).await?;
        let filename = image
            .src
            .rsplit('/')
            .next()
            .map(percent_decode)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("{}.jpg", image.id));
        let alt = Some(html::text(&image.alt))
            .filter(|a| !a.is_empty())
            .unwrap_or_else(|| title.to_owned());
        let asset = upload_bytes(self.client, &self.site, bytes, &filename, Some(&alt))
            .await
            .with_context(|| format!("zdjęcie {}", image.id))?;
        let id = text(&asset["id"]);
        if id.is_empty() {
            bail!("zdjęcie {}: brak id w odpowiedzi", image.id);
        }
        self.report.media += 1;
        self.state.media.insert(key, id.clone());
        Ok(id)
    }

    /// Photos of variations (with `--photos all`): variation id → asset.
    async fn variation_images(&mut self, variations: &[Variation]) -> Vec<(u64, String)> {
        let mut found = Vec::new();
        if self.options.photos != WooPhotos::All {
            return found;
        }
        for variation in variations {
            if let Some(image) = &variation.image
                && image.id != 0
            {
                match self.photo(image, "").await {
                    Ok(asset) => found.push((variation.offer.id, asset)),
                    Err(error) => self
                        .report
                        .warnings
                        .push(format!("wariant {}: {error:#}", variation.offer.id)),
                }
            }
        }
        found
    }

    async fn placeholder(&mut self) -> Result<String> {
        if let Some(asset) = &self.state.placeholder {
            return Ok(asset.clone());
        }
        let asset = upload_bytes(
            self.client,
            &self.site,
            PLACEHOLDER.to_vec(),
            "produkt-zdjecie-wkrotce.png",
            Some("Zdjęcie produktu wkrótce"),
        )
        .await
        .context("zdjęcie zastępcze")?;
        let id = text(&asset["id"]);
        self.state.placeholder = Some(id.clone());
        self.state.save(&self.state_path)?;
        Ok(id)
    }
}

struct Terms {
    categories: HashMap<u64, String>,
    /// (attribute id, value name) → term id
    values: HashMap<(u64, String), String>,
}

fn photos_name(photos: WooPhotos) -> &'static str {
    match photos {
        WooPhotos::All => "all",
        WooPhotos::Placeholder => "placeholder",
        WooPhotos::None => "none",
    }
}

// --- conversions -------------------------------------------------------------

fn clip(value: &str, max: usize) -> String {
    value
        .chars()
        .take(max)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `kanapa-%c5%82adna` → `kanapa-ladna`: what the CMS accepts in addresses.
fn slugify(value: &str) -> String {
    let mut slug = String::new();
    for c in percent_decode(value).to_lowercase().chars() {
        let ascii = match c {
            'ą' | 'á' | 'à' | 'â' | 'ä' => 'a',
            'ć' | 'č' | 'ç' => 'c',
            'ę' | 'é' | 'è' | 'ê' | 'ë' | 'ě' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ł' => 'l',
            'ń' | 'ň' | 'ñ' => 'n',
            'ó' | 'ò' | 'ô' | 'ö' => 'o',
            'ś' | 'š' => 's',
            'ú' | 'ù' | 'û' | 'ü' | 'ů' => 'u',
            'ý' => 'y',
            'ź' | 'ż' | 'ž' => 'z',
            'ř' => 'r',
            c if c.is_ascii_alphanumeric() => c,
            _ => '-',
        };
        if ascii != '-' || !slug.ends_with('-') {
            slug.push(ascii);
        }
    }
    slug.trim_matches('-')
        .chars()
        .take(96)
        .collect::<String>()
        .trim_end_matches('-')
        .to_owned()
}

/// `4550.90` → 455090 grosze.
fn grosze(value: &str) -> Option<i64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let (whole, fraction) = value.split_once('.').unwrap_or((value, "0"));
    let whole: i64 = whole.parse().ok()?;
    let fraction: String = fraction.chars().chain("00".chars()).take(2).collect();
    Some(whole * 100 + fraction.parse::<i64>().ok()?)
}

/// `171.5` (cm or kg) × factor, rounded.
fn scaled(value: &str, factor: f64) -> Option<i64> {
    let number: f64 = value.trim().replace(',', ".").parse().ok()?;
    let scaled = (number * factor).round();
    (scaled >= 0.0 && scaled < f64::from(i32::MAX)).then_some(scaled as i64)
}

/// WooCommerce's GMT time (`2026-05-01T00:00:00`) as RFC 3339.
fn woo_time(value: Option<&String>) -> Option<String> {
    let value = value?.trim();
    chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S")
        .ok()
        .map(|t| t.and_utc().to_rfc3339())
}

fn shipping_class(slug: &str) -> Option<String> {
    let slug = clip(&slugify(slug), 40);
    let slug = slug.trim_end_matches('-').to_owned();
    (!slug.is_empty()).then_some(slug)
}

/// One variant of the shop from a product or a variation.
fn variant(offer: &Offer, sku: String, stock_of_parent: Option<i64>) -> Value {
    let price = grosze(&offer.regular_price)
        .or_else(|| grosze(&offer.price))
        .unwrap_or(0);
    let sale_price = grosze(&offer.sale_price).filter(|sale| *sale < price);
    let managed = match &offer.manage_stock {
        Value::Bool(managed) => Some(*managed),
        Value::String(s) if s == "parent" => None,
        _ => Some(false),
    };
    let stock = match managed {
        Some(true) => offer.stock_quantity,
        None => stock_of_parent,
        Some(false) if offer.stock_status == "outofstock" => Some(0),
        Some(false) => None,
    }
    .map(|q| q.clamp(0, 1_000_000));
    let mut variant = json!({
        "sku": sku,
        "price": price,
        "active": price > 0 || offer.status.is_empty() || offer.status == "publish",
    });
    let mut put = |key: &str, value: Option<Value>| {
        if let Some(value) = value {
            variant[key] = value;
        }
    };
    put("sale_price", sale_price.map(|s| json!(s)));
    put(
        "sale_from",
        sale_price
            .and(woo_time(offer.date_on_sale_from_gmt.as_ref()))
            .map(Value::from),
    );
    put(
        "sale_to",
        sale_price
            .and(woo_time(offer.date_on_sale_to_gmt.as_ref()))
            .map(Value::from),
    );
    put("stock", stock.map(|s| json!(s)));
    put("weight_g", scaled(&offer.weight, 1000.0).map(|v| json!(v)));
    put(
        "length_mm",
        scaled(&offer.dimensions.length, 10.0).map(|v| json!(v)),
    );
    put(
        "width_mm",
        scaled(&offer.dimensions.width, 10.0).map(|v| json!(v)),
    );
    put(
        "height_mm",
        scaled(&offer.dimensions.height, 10.0).map(|v| json!(v)),
    );
    variant
}

/// The product's variants: one for a simple product, one per variation of a
/// variable one. Variants already in the shop keep their id, gallery and
/// parcels (matched by SKU).
fn variants_of(
    product: &Product,
    variations: &[Variation],
    current: &BTreeMap<String, Value>,
    images: &[(u64, String)],
) -> Vec<Value> {
    let parent_sku = product.offer.sku.trim();
    let mut used = HashSet::new();
    let mut unique = |sku: String, id: u64| {
        let sku = clip(&sku, 64);
        if used.insert(sku.to_lowercase()) {
            sku
        } else {
            let other = clip(&format!("{sku}-{id}"), 64);
            used.insert(other.to_lowercase());
            other
        }
    };
    let mut variants: Vec<Value> = if variations.is_empty() {
        let sku = if parent_sku.is_empty() {
            format!("WOO-{}", product.offer.id)
        } else {
            parent_sku.to_owned()
        };
        vec![variant(&product.offer, unique(sku, product.offer.id), None)]
    } else {
        variations
            .iter()
            .take(MAX_VARIANTS)
            .map(|v| {
                let own = v.offer.sku.trim();
                let sku = if !own.is_empty() && own != parent_sku {
                    own.to_owned()
                } else if !parent_sku.is_empty() {
                    format!("{parent_sku}-{}", v.offer.id)
                } else {
                    format!("WOO-{}", v.offer.id)
                };
                let mut variant = variant(
                    &v.offer,
                    unique(sku, v.offer.id),
                    product.offer.stock_quantity,
                );
                let options: BTreeMap<String, String> = v
                    .attributes
                    .iter()
                    .filter(|a| !a.name.trim().is_empty() && !a.option.trim().is_empty())
                    .map(|a| {
                        (
                            clip(&html::text(&a.name), 60),
                            clip(&html::text(&a.option), 120),
                        )
                    })
                    .collect();
                if !options.is_empty() {
                    variant["options"] = json!(options);
                }
                if let Some((_, asset)) = images.iter().find(|(id, _)| *id == v.offer.id) {
                    variant["gallery"] = json!([asset]);
                }
                variant
            })
            .collect()
    };
    for variant in &mut variants {
        if let Some(existing) = current.get(&text(&variant["sku"]).to_lowercase()) {
            for key in ["id", "parcels"] {
                if !existing[key].is_null() {
                    variant[key] = existing[key].clone();
                }
            }
            if variant["gallery"].is_null() && !existing["gallery"].is_null() {
                variant["gallery"] = existing["gallery"].clone();
            }
        }
    }
    variants
}

/// What the shop would refuse, said before sending.
fn check(variants: &[Value]) -> std::result::Result<(), String> {
    if variants.is_empty() {
        return Err("brak wariantów".into());
    }
    for variant in variants {
        let sku = text(&variant["sku"]);
        let price = variant["price"].as_i64().unwrap_or(0);
        if !(0..=100_000_000).contains(&price) {
            return Err(format!("{sku}: cena poza zakresem"));
        }
        for (key, max) in [
            ("length_mm", 100_000),
            ("width_mm", 100_000),
            ("height_mm", 100_000),
            ("weight_g", 10_000_000),
        ] {
            if variant[key].as_i64().is_some_and(|v| v > max) {
                return Err(format!("{sku}: {key} poza zakresem"));
            }
        }
    }
    Ok(())
}

/// Tabs kept in a meta field: a list of `{title, content, enabled}`.
fn tabs_of(product: &Product, key: &str, hosts: &[String]) -> Vec<Value> {
    product
        .meta(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|tab| tab["enabled"].as_bool() != Some(false))
        .filter_map(|tab| {
            let title = html::text(tab["title"].as_str()?);
            let content = html::to_doc(tab["content"].as_str()?, hosts)?;
            (!title.is_empty()).then(|| json!({ "title": clip(&title, 80), "content": content }))
        })
        .take(MAX_TABS)
        .collect()
}

/// Yoast's title without the site name the CMS adds itself.
fn seo_title(product: &Product, title: &str, suffixes: &[String]) -> Option<String> {
    let yoast = product.yoast_head_json.as_ref()?.get("title")?.as_str()?;
    let bare = suffixes.iter().fold(html::text(yoast), |text, suffix| {
        text.strip_suffix(suffix.as_str())
            .map(str::to_owned)
            .unwrap_or(text)
    });
    (bare != title && !bare.is_empty()).then(|| clip(&bare, 120))
}

fn seo_description(product: &Product) -> Option<String> {
    let text = html::text(
        product
            .yoast_head_json
            .as_ref()?
            .get("description")?
            .as_str()?,
    );
    (!text.is_empty()).then(|| clip(&text, 320))
}

/// A trial import: `limit` products taken in turn from every category, so a
/// small sample still fills every listing.
fn sample(products: Vec<Product>, limit: usize) -> Vec<Product> {
    let mut by_category: BTreeMap<u64, VecDeque<Product>> = BTreeMap::new();
    for product in products {
        let category = product.categories.first().map_or(0, |c| c.id);
        by_category.entry(category).or_default().push_back(product);
    }
    let mut picked = Vec::new();
    while picked.len() < limit && by_category.values().any(|q| !q.is_empty()) {
        for queue in by_category.values_mut() {
            if picked.len() == limit {
                break;
            }
            if let Some(product) = queue.pop_front() {
                picked.push(product);
            }
        }
    }
    picked
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn product(value: Value) -> Product {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn slugs_prices_and_sizes() {
        assert_eq!(slugify("kanapa-%c5%82adna"), "kanapa-ladna");
        assert_eq!(slugify("Sofa Ćma -- XL!"), "sofa-cma-xl");
        assert_eq!(grosze("4550.90"), Some(455_090));
        assert_eq!(grosze("0.9"), Some(90));
        assert_eq!(grosze("12"), Some(1_200));
        assert_eq!(grosze(""), None);
        assert_eq!(scaled("171", 10.0), Some(1_710));
        assert_eq!(scaled("28,4", 1000.0), Some(28_400));
        assert_eq!(scaled("", 10.0), None);
        assert_eq!(shipping_class("1/2 palety"), Some("1-2-palety".into()));
        assert_eq!(
            woo_time(Some(&"2026-05-01T10:00:00".to_owned())).as_deref(),
            Some("2026-05-01T10:00:00+00:00")
        );
    }

    #[test]
    fn simple_product_is_one_variant() {
        let p = product(json!({
            "id": 7, "name": "Sofa", "slug": "sofa", "type": "simple", "sku": "SOF-1",
            "regular_price": "1999.00", "sale_price": "1499", "manage_stock": true,
            "stock_quantity": 3, "weight": "41.5", "dimensions": { "length": "210", "width": "90", "height": "" },
            "date_on_sale_to_gmt": "2026-12-31T23:59:59"
        }));
        let current = BTreeMap::from([(
            "sof-1".to_owned(),
            json!({ "id": "var_1", "sku": "SOF-1", "parcels": 2, "gallery": ["asset_1"] }),
        )]);
        let variants = variants_of(&p, &[], &current, &[]);
        assert_eq!(variants.len(), 1);
        let v = &variants[0];
        assert_eq!(v["id"], "var_1");
        assert_eq!(v["price"], 199_900);
        assert_eq!(v["sale_price"], 149_900);
        assert_eq!(v["sale_to"], "2026-12-31T23:59:59+00:00");
        assert_eq!(v["stock"], 3);
        assert_eq!(v["weight_g"], 41_500);
        assert_eq!(v["length_mm"], 2_100);
        assert!(v.get("height_mm").is_none());
        assert_eq!(v["parcels"], 2);
        assert_eq!(v["gallery"], json!(["asset_1"]));
        assert!(check(&variants).is_ok());
    }

    #[test]
    fn variations_become_variants_with_options() {
        let p = product(json!({
            "id": 9, "name": "Krzesło", "slug": "krzeslo", "type": "variable", "sku": "KRZ",
            "manage_stock": true, "stock_quantity": 5
        }));
        let variations: Vec<Variation> = serde_json::from_value(json!([
            { "id": 11, "sku": "KRZ", "regular_price": "300", "manage_stock": "parent",
              "attributes": [{ "name": "Kolor", "option": "Czarny" }] },
            { "id": 12, "sku": "KRZ-B", "regular_price": "320", "manage_stock": false,
              "stock_status": "outofstock", "attributes": [{ "name": "Kolor", "option": "Biały" }] }
        ]))
        .unwrap();
        let variants = variants_of(&p, &variations, &BTreeMap::new(), &[(12, "asset_9".into())]);
        assert_eq!(
            variants[0]["sku"], "KRZ-11",
            "a variation inheriting the SKU gets its own"
        );
        assert_eq!(variants[0]["stock"], 5, "stock of the parent");
        assert_eq!(variants[0]["options"], json!({ "Kolor": "Czarny" }));
        assert_eq!(variants[1]["sku"], "KRZ-B");
        assert_eq!(variants[1]["stock"], 0);
        assert_eq!(variants[1]["gallery"], json!(["asset_9"]));
    }

    #[test]
    fn seo_and_tabs() {
        let p = product(json!({
            "id": 1, "name": "Sofa", "slug": "sofa",
            "yoast_head_json": { "title": "Sofa nowoczesna | Mój sklep", "description": "Opis &amp; więcej" },
            "meta_data": [{ "key": "_tabs", "value": [
                { "title": "Wymiary", "content": "<p>210 cm</p>" },
                { "title": "Ukryta", "content": "<p>x</p>", "enabled": false }
            ] }]
        }));
        assert_eq!(
            seo_title(&p, "Sofa", &[" | Mój sklep".into()]).as_deref(),
            Some("Sofa nowoczesna")
        );
        assert_eq!(seo_description(&p).as_deref(), Some("Opis & więcej"));
        let tabs = tabs_of(&p, "_tabs", &[]);
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0]["title"], "Wymiary");
    }
}
