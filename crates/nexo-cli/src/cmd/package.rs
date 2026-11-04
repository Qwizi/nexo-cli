//! `nexo package`: a package of Nexo CMS (an extension or a theme) from its
//! scaffold to the archive the panel installs.
//!
//! - `new`: the manifest, a renderer, a backend (Rust or TypeScript) and a
//!   README, in a folder named after the package's id.
//! - `check`: the layout, the manifest, the renderer's imports, the backend
//!   and the version's tag here; with `--server` the CMS checks the whole
//!   package too (manifest rules, the component against its WIT) without
//!   installing it.
//! - `build`: the backend as `backend.wasm` (Rust: cargo for wasm32-wasip2;
//!   TypeScript: the backend's `npm run build`, jco).
//! - `pack`: the `.zip` for Administracja › Rozszerzenia › Wgraj.
//! - `wit`: the WIT of the core and of the packages it requires, from a
//!   Nexo CMS tree, into `wit/deps`.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use nexo_core::client::{Body, FilePart};
use reqwest::Method;
use serde_json::{Value, json};

use crate::cli::{PackageBackend, PackageCommand, PackageKind};
use crate::ctx::{Ctx, text};
use crate::out;

const MANIFEST: &str = "nexo.json";
/// File types the CMS takes in a package (its upload check).
const ALLOWED: &[&str] = &[
    "json", "md", "txt", "svelte", "ts", "js", "css", "svg", "png", "jpg", "jpeg", "webp", "avif",
    "gif", "ico", "woff", "woff2", "wasm", "sql", "wit",
];
/// Build output and installed modules, never part of the archive (nor
/// `backend/` at the package's top, its sources, or `wit/deps`).
const SKIPPED: &[&str] = &["node_modules", "target", "dist"];
/// Modules a renderer may import besides its own files and the packages it
/// requires (`@ext/<id>`).
const RENDERER_IMPORTS: &[&str] = &[
    "svelte",
    "$app",
    "@nexo-cms/theme-sdk",
    "@nexo-cms/contract",
];

pub async fn run(ctx: &Ctx, command: PackageCommand) -> Result<()> {
    match command {
        PackageCommand::New {
            kind,
            id,
            name,
            backend,
            dir,
        } => new(&kind, &id, name, backend, dir),
        PackageCommand::Check { dir, server } => check(ctx, &dir, server).await,
        PackageCommand::Build { dir } => build(&dir).map(|path| {
            println!("Zbudowano {}", path.display());
        }),
        PackageCommand::Pack { dir, output } => pack(&dir, output).map(|(path, files)| {
            println!("Archiwum {} ({files} plików)", path.display());
        }),
        PackageCommand::Wit { dir, core } => wit(&dir, core),
    }
}

/// `com.acme.hello` → `hello`: what the manifest's `key` becomes.
fn key_of(id: &str) -> String {
    let last = id.rsplit('.').next().unwrap_or(id);
    let mut key: String = last
        .chars()
        .map(|c| {
            if c == '-' {
                '_'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_')
        .collect();
    if !key.starts_with(|c: char| c.is_ascii_lowercase()) {
        key.insert(0, 'x');
    }
    key.truncate(40);
    key
}

/// Reverse DNS, at least two segments of lowercase letters, digits and `-`.
fn valid_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('.').collect();
    parts.len() >= 2
        && id.len() <= 100
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

/// The Nexo CMS tree `dir` is in (it has `sdk/extension`), if any.
fn core_tree(dir: &Path) -> Option<PathBuf> {
    let start = std::fs::canonicalize(dir).ok()?;
    start
        .ancestors()
        .find(|d| d.join("sdk/extension/Cargo.toml").is_file() && d.join("sdk/wit").is_dir())
        .map(Path::to_path_buf)
}

/// `to` relative to `from` (both absolute), with `/` between parts.
fn relative(from: &Path, to: &Path) -> String {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = vec!["..".to_owned(); from.len() - common];
    parts.extend(
        to[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    parts.join("/")
}

fn write(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text).with_context(|| path.display().to_string())
}

fn fill(template: &str, id: &str, key: &str, name: &str) -> String {
    template
        .replace("{{id}}", id)
        .replace("{{key}}", key)
        .replace("{{name}}", name)
}

fn new(
    kind: &PackageKind,
    id: &str,
    name: Option<String>,
    backend: PackageBackend,
    dir: Option<PathBuf>,
) -> Result<()> {
    if !valid_id(id) {
        bail!("identyfikator paczki to odwrócona domena, np. com.twoja-firma.nazwa");
    }
    let root = dir.unwrap_or_else(|| PathBuf::from(".")).join(id);
    if root.exists() {
        bail!("{} już istnieje", root.display());
    }
    let key = key_of(id);
    let name = name.unwrap_or_else(|| {
        let mut chars = key.replace('_', " ").chars().collect::<Vec<_>>();
        if let Some(first) = chars.first_mut() {
            *first = first.to_ascii_uppercase();
        }
        chars.into_iter().collect()
    });
    let files: Vec<(&str, String)> = match kind {
        PackageKind::Theme => vec![
            (MANIFEST, fill(templates::THEME_MANIFEST, id, &key, &name)),
            (
                "renderer/index.ts",
                fill(templates::THEME_RENDERER, id, &key, &name),
            ),
            (
                "renderer/theme.css",
                fill(templates::THEME_CSS, id, &key, &name),
            ),
            (
                "package.json",
                fill(templates::PACKAGE_JSON, id, &key, &name),
            ),
            ("README.md", fill(templates::THEME_README, id, &key, &name)),
            (".gitignore", templates::GITIGNORE.to_owned()),
        ],
        PackageKind::Extension => {
            let mut files = vec![
                (MANIFEST, extension_manifest(id, &key, &name, backend)),
                (
                    "renderer/index.ts",
                    fill(templates::RENDERER, id, &key, &name),
                ),
                (
                    "package.json",
                    fill(templates::PACKAGE_JSON, id, &key, &name),
                ),
                (".gitignore", templates::GITIGNORE.to_owned()),
            ];
            match backend {
                PackageBackend::Rust => {
                    std::fs::create_dir_all(root.join("backend"))?;
                    let sdk = sdk_dependency(&root.join("backend"))?;
                    files.extend([
                        (
                            "backend/Cargo.toml",
                            fill(templates::RUST_CARGO, id, &key, &name).replace("{{sdk}}", &sdk),
                        ),
                        (
                            "backend/src/lib.rs",
                            fill(templates::RUST_LIB, id, &key, &name),
                        ),
                        (
                            "backend/rust-toolchain.toml",
                            templates::RUST_TOOLCHAIN.to_owned(),
                        ),
                        ("README.md", fill(templates::README_RUST, id, &key, &name)),
                    ]);
                }
                PackageBackend::Typescript => files.extend([
                    (
                        "backend/package.json",
                        fill(templates::TS_PACKAGE, id, &key, &name),
                    ),
                    ("backend/tsconfig.json", templates::TS_CONFIG.to_owned()),
                    (
                        "backend/src/index.ts",
                        fill(templates::TS_INDEX, id, &key, &name),
                    ),
                    ("README.md", fill(templates::README_TS, id, &key, &name)),
                ]),
                PackageBackend::None => {
                    files.push(("README.md", fill(templates::README_NONE, id, &key, &name)));
                }
            }
            files
        }
    };
    for (path, text) in &files {
        write(&root.join(path), text)?;
    }
    if matches!(kind, PackageKind::Extension) && backend == PackageBackend::Typescript {
        // The backend's types come from the core's WIT.
        if let Err(error) = wit(&root, None) {
            println!(
                "Uwaga: {error:#}; uruchom później `nexo package wit --core <drzewo Nexo CMS>`."
            );
        }
    }
    println!("Utworzono {} ({} plików).", root.display(), files.len());
    println!(
        "Dalej: nexo package build {0} && nexo package pack {0}",
        root.display()
    );
    Ok(())
}

fn extension_manifest(id: &str, key: &str, name: &str, backend: PackageBackend) -> String {
    let mut manifest = json!({
        "id": id,
        "type": "extension",
        "key": key,
        "name": name,
        "version": "0.1.0",
        "description": format!("{name}: rozszerzenie Nexo CMS."),
        "publisher": { "name": "Twoja firma" },
        "core": "^2.0",
    });
    if backend != PackageBackend::None {
        manifest["permissions"] = json!([{
            "key": format!("{key}.page.read"),
            "label": name,
            "description": "Widzi stronę rozszerzenia w panelu.",
        }]);
        manifest["pages"] = json!([{
            "key": "start",
            "label": name,
            "description": "Strona rozszerzenia w panelu.",
            "icon": "boxes",
            "permission": format!("{key}.page.read"),
            "path": "/admin/start",
        }]);
        manifest["backend"] = json!({});
    }
    let mut text = serde_json::to_string_pretty(&manifest).unwrap_or_default();
    text.push('\n');
    text
}

/// The SDK as the backend depends on it: by path inside a Nexo CMS tree,
/// else from the core's repository.
fn sdk_dependency(backend: &Path) -> Result<String> {
    Ok(match core_tree(backend) {
        Some(core) => {
            let from = std::fs::canonicalize(backend)?;
            format!(
                "{{ path = \"{}\" }}",
                relative(&from, &core.join("sdk/extension"))
            )
        }
        None => "{ git = \"https://github.com/Qwizi/nexo-cms\", branch = \"main\" }".to_owned(),
    })
}

fn read_manifest(dir: &Path) -> Result<Value> {
    let path = dir.join(MANIFEST);
    let bytes = std::fs::read(&path)
        .with_context(|| format!("brak {} (to folder paczki?)", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("{} nie jest poprawnym JSON-em", path.display()))
}

/// What the local checks found: errors stop a pack, warnings do not.
#[derive(Debug, Default)]
struct Findings {
    errors: Vec<String>,
    warnings: Vec<String>,
}

fn semver(version: &str) -> bool {
    let core = version.split(['-', '+']).next().unwrap_or_default();
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Import specifiers of a renderer file (`from '…'` and `import '…'`).
fn imports(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    for marker in [
        "from '",
        "from \"",
        "import '",
        "import \"",
        "import('",
        "import(\"",
    ] {
        let quote = marker.chars().last().unwrap_or('\'');
        let mut rest = source;
        while let Some(at) = rest.find(marker) {
            rest = &rest[at + marker.len()..];
            if let Some(end) = rest.find(quote) {
                found.push(rest[..end].to_owned());
            }
        }
    }
    found
}

/// The package's files under `dir` (its root or a folder in it).
fn files_under(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            let sources = dir == root && name == "backend";
            let deps = name == "deps" && path.parent() == Some(root.join("wit").as_path());
            if !sources && !deps && !SKIPPED.contains(&name.as_str()) {
                files_under(root, &path, out);
            }
        } else {
            out.push(path);
        }
    }
}

fn local_findings(dir: &Path) -> Result<(Value, Findings)> {
    let manifest = read_manifest(dir)?;
    let mut f = Findings::default();
    let id = text(&manifest["id"]);
    if !valid_id(&id) {
        f.errors
            .push("`id`: odwrócona domena, np. com.twoja-firma.nazwa".to_owned());
    }
    if let Some(folder) = std::fs::canonicalize(dir)
        .ok()
        .and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
        && folder != id
    {
        f.warnings
            .push(format!("folder nazywa się {folder}, a paczka {id}: repozytoria trzymają paczkę w folderze jej id"));
    }
    let kind = text(&manifest["type"]);
    if kind != "extension" && kind != "theme" {
        f.errors.push("`type`: extension albo theme".to_owned());
    }
    if kind == "extension" {
        let key = text(&manifest["key"]);
        let ok = key.len() <= 40
            && key.starts_with(|c: char| c.is_ascii_lowercase())
            && key
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        if !ok {
            f.errors
                .push("`key`: małe litery, cyfry i _, od litery, do 40 znaków".to_owned());
        }
    }
    for field in ["name", "core"] {
        if text(&manifest[field]).trim().is_empty() {
            f.errors.push(format!("brak `{field}`"));
        }
    }
    if text(&manifest["publisher"]["name"]).trim().is_empty() {
        f.errors.push("brak `publisher.name`".to_owned());
    }
    let version = text(&manifest["version"]);
    if !semver(&version) {
        f.errors.push(format!(
            "`version`: wersja semver (1.2.3), jest „{version}”"
        ));
    }
    if let Some(tag) = released_elsewhere(dir, &version) {
        f.warnings.push(format!(
            "wersja {version} jest już wydana (tag {tag} wskazuje inny commit): podnieś `version`"
        ));
    }

    // Renderer: only the modules a package may import.
    let requires: Vec<String> = manifest["requires"]
        .as_array()
        .map(|r| r.iter().map(text).collect())
        .unwrap_or_default();
    let mut files = Vec::new();
    files_under(dir, &dir.join("renderer"), &mut files);
    for file in &files {
        let ext = file
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        if !matches!(ext, "ts" | "js" | "svelte") {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(file) else {
            continue;
        };
        for module in imports(&source) {
            let allowed = module.starts_with('.')
                || RENDERER_IMPORTS
                    .iter()
                    .any(|m| module == *m || module.starts_with(&format!("{m}/")))
                || module
                    .strip_prefix("@ext/")
                    .is_some_and(|other| requires.iter().any(|r| r == other));
            if !allowed {
                let shown = file.strip_prefix(dir).unwrap_or(file).display().to_string();
                f.errors.push(format!(
                    "{shown}: import „{module}” (renderer importuje svelte, $app/*, @nexo-cms/theme-sdk, @nexo-cms/contract, @ext/<wymagana paczka> i własne pliki)"
                ));
            }
        }
    }

    // Backend: the component where the manifest says, migrations, WIT.
    if let Some(backend) = manifest["backend"].as_object() {
        let wasm = backend
            .get("wasm")
            .and_then(Value::as_str)
            .unwrap_or("backend.wasm");
        match std::fs::read(dir.join(wasm)) {
            Ok(bytes) if bytes.starts_with(b"\0asm") && bytes.get(6..8) == Some(&[1, 0][..]) => {}
            Ok(bytes) if bytes.starts_with(b"\0asm") => f.errors.push(format!(
                "{wasm} to moduł WebAssembly, a nie komponent: zbuduj go dla wasm32-wasip2 (`nexo package build`)"
            )),
            Ok(_) => f.errors.push(format!("{wasm} nie jest plikiem WebAssembly")),
            Err(_) => f.errors.push(format!("brak {wasm}: zbuduj backend (`nexo package build`)")),
        }
        let database = backend
            .get("database")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut versions = std::collections::BTreeSet::new();
        if let Ok(entries) = std::fs::read_dir(dir.join("migrations")) {
            if !database {
                f.errors
                    .push("migrations/ bez `backend.database: true`".to_owned());
            }
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let version = name
                    .strip_suffix(".sql")
                    .and_then(|stem| stem.split_once('_'))
                    .filter(|(_, rest)| !rest.is_empty())
                    .and_then(|(v, _)| v.parse::<i64>().ok());
                match version {
                    Some(v) if !versions.insert(v) => {
                        f.errors
                            .push(format!("migrations/{name}: wersja {v} powtarza się"));
                    }
                    Some(_) => {}
                    None => f
                        .errors
                        .push(format!("migrations/{name}: nazwa <wersja>_<opis>.sql")),
                }
            }
        }
    }
    if let Some(wit) = manifest["defines"]["wit"].as_str()
        && !dir.join(wit).is_file()
    {
        f.errors.push(format!("brak {wit} (defines.wit)"));
    }
    Ok((manifest, f))
}

/// The tag `v<version>` when it exists and is not this commit.
fn released_elsewhere(dir: &Path, version: &str) -> Option<String> {
    let tag = format!("v{version}");
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
    };
    let tagged = git(&["rev-list", "-n", "1", &tag])?;
    let head = git(&["rev-parse", "HEAD"])?;
    (tagged != head).then_some(tag)
}

async fn check(ctx: &Ctx, dir: &Path, server: bool) -> Result<()> {
    let (manifest, findings) = local_findings(dir)?;
    for warning in &findings.warnings {
        println!("uwaga: {warning}");
    }
    for error in &findings.errors {
        println!("błąd: {error}");
    }
    if !findings.errors.is_empty() {
        bail!(
            "{} {}: {} błędów",
            text(&manifest["id"]),
            text(&manifest["version"]),
            findings.errors.len()
        );
    }
    println!(
        "{} {}: układ, manifest, importy i backend w porządku.",
        text(&manifest["id"]),
        text(&manifest["version"])
    );
    if !server {
        return Ok(());
    }
    // The CMS checks what only it can (manifest rules, the component
    // against its WIT and the installed packages), then the upload goes.
    let (bytes, _) = archive(dir)?;
    let client = ctx.client()?;
    let id = text(&manifest["id"]);
    let answer = client
        .json(
            Method::POST,
            "/extensions/uploads",
            &[],
            Body::Multipart {
                fields: Vec::new(),
                file: FilePart {
                    field: "file".to_owned(),
                    filename: format!("{id}.zip"),
                    mime: "application/zip".to_owned(),
                    bytes,
                },
            },
        )
        .await?;
    let upload = &answer["data"];
    let summary = &upload["summary"];
    if let Some(upload_id) = upload["id"].as_str() {
        client
            .json(
                Method::DELETE,
                &format!("/extensions/uploads/{upload_id}"),
                &[],
                Body::None,
            )
            .await
            .ok();
    }
    if ctx.json {
        out::value(upload);
    } else {
        println!("CMS przyjąłby paczkę (nie zainstalowano jej):");
        out::value(summary);
    }
    Ok(())
}

fn build(dir: &Path) -> Result<PathBuf> {
    let manifest = read_manifest(dir)?;
    let wasm = manifest["backend"]["wasm"]
        .as_str()
        .unwrap_or("backend.wasm")
        .to_owned();
    if manifest["backend"].is_null() {
        bail!("ta paczka nie ma backendu (`backend` w nexo.json)");
    }
    let backend = dir.join("backend");
    let target = dir.join(&wasm);
    if backend.join("Cargo.toml").is_file() {
        let output = Command::new("cargo")
            .args([
                "build",
                "--release",
                "--target",
                "wasm32-wasip2",
                "--message-format=json-render-diagnostics",
            ])
            .current_dir(&backend)
            .stderr(std::process::Stdio::inherit())
            .output()
            .context("nie da się uruchomić cargo (zainstaluj Rusta: https://rustup.rs)")?;
        if !output.status.success() {
            bail!("cargo nie zbudowało backendu (rustup target add wasm32-wasip2?)");
        }
        let built = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|m| m["reason"] == "compiler-artifact")
            .flat_map(|m| {
                m["filenames"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|f| f.as_str().map(PathBuf::from))
                    .collect::<Vec<_>>()
            })
            .rfind(|f| f.extension().is_some_and(|e| e == "wasm"))
            .context("cargo nie zgłosiło pliku .wasm (crate-type = [\"cdylib\"]?)")?;
        std::fs::copy(&built, &target)?;
    } else if backend.join("package.json").is_file() {
        let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
        let status = Command::new(npm)
            .args(["run", "build"])
            .current_dir(&backend)
            .status()
            .context("nie da się uruchomić npm (zainstaluj Node.js)")?;
        if !status.success() {
            bail!("`npm run build` w backend/ się nie udało (npm install? nexo package wit?)");
        }
        if !target.is_file() {
            bail!("`npm run build` nie zapisał {wasm}");
        }
    } else {
        bail!("brak backend/Cargo.toml ani backend/package.json");
    }
    Ok(target)
}

/// The archive's files: everything the CMS takes, without sources and
/// build output (`backend/`, `node_modules`, `target`, `wit/deps`).
fn package_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    files_under(dir, dir, &mut files);
    files
        .into_iter()
        .filter(|path| {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase);
            matches!(name, "LICENSE" | "README")
                || ext.is_some_and(|e| ALLOWED.contains(&e.as_str()))
        })
        .collect()
}

fn archive(dir: &Path) -> Result<(Vec<u8>, usize)> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    let files = package_files(dir);
    {
        let mut zip = zip::ZipWriter::new(&mut buffer);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        // The manifest first: the CMS reads it before anything else.
        zip.start_file(MANIFEST, options)?;
        zip.write_all(&std::fs::read(dir.join(MANIFEST))?)?;
        for file in &files {
            let relative = file
                .strip_prefix(dir)
                .unwrap_or(file)
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            if relative == MANIFEST {
                continue;
            }
            zip.start_file(relative, options)?;
            zip.write_all(&std::fs::read(file)?)?;
        }
        zip.finish()?;
    }
    Ok((buffer.into_inner(), files.len()))
}

fn pack(dir: &Path, output: Option<PathBuf>) -> Result<(PathBuf, usize)> {
    let (manifest, findings) = local_findings(dir)?;
    if !findings.errors.is_empty() {
        for error in &findings.errors {
            println!("błąd: {error}");
        }
        bail!("paczka ma błędy: popraw je (`nexo package check`)");
    }
    let (bytes, files) = archive(dir)?;
    let path = output.unwrap_or_else(|| {
        PathBuf::from(format!(
            "{}-{}.zip",
            text(&manifest["id"]),
            text(&manifest["version"])
        ))
    });
    std::fs::write(&path, bytes)?;
    Ok((path, files))
}

/// The core's WIT and that of the packages the package requires, into
/// `wit/deps/<package>/`, from a Nexo CMS tree (`--core`, or the tree the
/// package is in).
fn wit(dir: &Path, core: Option<PathBuf>) -> Result<()> {
    let manifest = read_manifest(dir)?;
    let core = match core {
        Some(core) => core,
        None => core_tree(dir).context(
            "paczka nie leży w drzewie Nexo CMS: podaj je w --core (git clone git@github.com:Qwizi/nexo-cms.git)",
        )?,
    };
    let deps = dir.join("wit/deps");
    let mut copied = Vec::new();
    let copy = |from: &Path, to: &Path, copied: &mut Vec<String>| -> Result<()> {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)?.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "wit") {
                std::fs::copy(&path, to.join(entry.file_name()))?;
                copied.push(path.display().to_string());
            }
        }
        Ok(())
    };
    copy(
        &core.join("sdk/wit"),
        &deps.join("nexo-extension"),
        &mut copied,
    )?;
    let requires: Vec<String> = manifest["requires"]
        .as_array()
        .map(|r| r.iter().map(text).collect())
        .unwrap_or_default();
    for required in &requires {
        let found = [core.join("extensions").join(required)]
            .into_iter()
            .chain(
                std::fs::read_dir(core.join("extensions"))
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|repo| repo.path().join(required)),
            )
            .find(|p| p.join("wit").is_dir());
        match found {
            Some(package) => copy(&package.join("wit"), &deps.join(required), &mut copied)?,
            None => println!(
                "uwaga: {required} nie ma WIT w {} (nie ma tej paczki albo nie definiuje haków)",
                core.join("extensions").display()
            ),
        }
    }
    println!("WIT w {}: {} plików", deps.display(), copied.len());
    Ok(())
}

mod templates {
    pub const GITIGNORE: &str =
        "node_modules/\n/backend/target/\n/backend/dist/\nbackend.wasm\n*.zip\n";

    pub const PACKAGE_JSON: &str = r#"{
	"name": "@nexo-ext/{{key}}",
	"private": true,
	"version": "0.1.0",
	"type": "module",
	"description": "Renderer of {{id}}.",
	"peerDependencies": {
		"svelte": "^5.0.0"
	},
	"dependencies": {
		"@nexo-cms/theme-sdk": "workspace:*"
	}
}
"#;

    pub const RENDERER: &str = r#"import type { ExtensionRenderer } from '@nexo-cms/theme-sdk';

/**
 * What {{name}} puts on the site: blocks (`blocks['ext:{{id}}/<block>']`),
 * fills of slots, parts. Nothing yet.
 */
export default {} satisfies ExtensionRenderer;
"#;

    pub const RUST_CARGO: &str = r#"[package]
name = "{{key}}-backend"
description = "Backend of {{id}}: a WebAssembly component of the nexo:extension world."
version = "0.1.0"
edition = "2024"
publish = false

# A component for the CMS (cdylib) and a library for native tests.
[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
nexo-extension-sdk = {{sdk}}
serde_json = "1"

# A package of its own, not a member of the tree it sits in.
[workspace]

# The component: small and fast (it is compiled once at install).
[profile.release]
opt-level = "s"
lto = true
strip = true
"#;

    pub const RUST_TOOLCHAIN: &str = r#"[toolchain]
channel = "stable"
targets = ["wasm32-wasip2"]
"#;

    pub const RUST_LIB: &str = r#"//! The backend of {{name}} (`{{id}}`): its panel page and what the site may
//! ask for. Build it with `nexo package build`; test it with `cargo test`.

use nexo_extension_sdk::panel::{Content, Record, Section};
use nexo_extension_sdk::{Ctx, Error, Json, Router};
use serde_json::{Value, json};

/// The page in the panel (`pages` in nexo.json).
fn start(ctx: Ctx) -> Result<Json<Record>, Error> {
    ctx.require("{{key}}.page.read")?;
    let mut record = Record::new("{{name}}");
    record.sections.push(Section::main(
        "Działa",
        Content::Text {
            text: format!("Rozszerzenie działa w witrynie {}.", ctx.site),
        },
    ));
    Ok(Json(record))
}

/// What the site and browsers reach at `/_ext/{{key}}/hello`.
fn hello(_ctx: Ctx) -> Json<Value> {
    Json(json!({ "message": "Cześć!" }))
}

pub fn router() -> Router {
    Router::new()
        .get("/admin/start", start)
        .get("/render/hello", hello)
}

#[cfg(target_arch = "wasm32")]
nexo_extension_sdk::backend!(router);

#[cfg(test)]
mod tests {
    use nexo_extension_sdk::testing;

    #[test]
    fn says_hello() {
        let response = testing::call(&super::router(), testing::get("/render/hello"));
        assert_eq!(response.status, 200);
    }

    #[test]
    fn the_page_needs_its_permission() {
        let router = super::router();
        assert_eq!(testing::call(&router, testing::get("/admin/start")).status, 403);
        let allowed = testing::get("/admin/start").as_user("usr_1", &["{{key}}.page.read"]);
        assert_eq!(testing::call(&router, allowed).status, 200);
    }
}
"#;

    pub const TS_PACKAGE: &str = r#"{
	"name": "{{key}}-backend",
	"private": true,
	"type": "module",
	"description": "Backend of {{id}}: a WebAssembly component (jco) of the nexo:extension world.",
	"scripts": {
		"types": "jco guest-types ../wit/deps/nexo-extension --world-name backend -o src/generated",
		"build": "npm run types && tsc && jco componentize dist/index.js --wit ../wit/deps/nexo-extension --world-name backend --disable http --disable fetch-event --out ../backend.wasm"
	},
	"devDependencies": {
		"@bytecodealliance/componentize-js": "*",
		"@bytecodealliance/jco": "*",
		"typescript": "*"
	}
}
"#;

    pub const TS_CONFIG: &str = r#"{
	"compilerOptions": {
		"target": "ES2022",
		"module": "ES2022",
		"moduleResolution": "bundler",
		"strict": true,
		"outDir": "dist",
		"rootDir": "src",
		"skipLibCheck": true
	},
	"include": ["src"]
}
"#;

    pub const TS_INDEX: &str = r#"/**
 * The backend of {{name}} (`{{id}}`): the exports of the nexo:extension
 * world (`handler`, `worker`, `subscriber`). Build it with
 * `nexo package build` (npm install first).
 */

interface Request {
	method: string;
	path: string;
	query: string;
	headers: [string, string][];
	body: Uint8Array;
}

interface Response {
	status: number;
	headers: [string, string][];
	body: Uint8Array;
}

function json(status: number, value: unknown): Response {
	return {
		status,
		headers: [['content-type', 'application/json']],
		body: new TextEncoder().encode(JSON.stringify(value))
	};
}

export const handler = {
	handle(request: Request): Response {
		if (request.method === 'GET' && request.path === '/render/hello') {
			return json(200, { message: 'Cześć!' });
		}
		return json(404, {
			error: { type: 'invalid_request_error', code: 'not_found', message: 'Nie ma takiej ścieżki.' }
		});
	}
};

export const worker = {
	run(_name: string, _payload: string): void {}
};

export const subscriber = {
	onEvent(_event: unknown): void {}
};
"#;

    pub const README_RUST: &str = r#"# {{name}}

`{{id}}` (key `{{key}}`): an extension of Nexo CMS with a backend in Rust
(`backend/`, a WebAssembly component of the `nexo:extension` world).

```bash
nexo package build      # backend.wasm (cargo, wasm32-wasip2)
nexo package check      # layout, manifest, imports, backend (--server: on the CMS too)
nexo package pack       # {{id}}-<version>.zip for Administracja › Rozszerzenia › Wgraj
```

Tests run natively on the SDK's doubles: `cargo test` in `backend/`.
"#;

    pub const README_TS: &str = r#"# {{name}}

`{{id}}` (key `{{key}}`): an extension of Nexo CMS with a backend in
TypeScript (`backend/`, a component built with jco from the core's WIT in
`wit/deps`).

```bash
nexo package wit        # the core's WIT into wit/deps (from a Nexo CMS tree)
(cd backend && npm install)
nexo package build      # backend.wasm (npm run build: jco componentize)
nexo package pack       # {{id}}-<version>.zip for Administracja › Rozszerzenia › Wgraj
```
"#;

    pub const README_NONE: &str = r#"# {{name}}

`{{id}}` (key `{{key}}`): an extension of Nexo CMS without a backend:
blocks, slots and parts in `renderer/`.

```bash
nexo package check
nexo package pack       # {{id}}-<version>.zip for Administracja › Rozszerzenia › Wgraj
```
"#;

    pub const THEME_MANIFEST: &str = r#"{
  "id": "{{id}}",
  "type": "theme",
  "name": "{{name}}",
  "version": "0.1.0",
  "description": "{{name}}: motyw Nexo CMS.",
  "publisher": {
    "name": "Twoja firma"
  },
  "core": "^2.0",
  "themes": [
    {
      "key": "{{key}}",
      "name": "{{name}}",
      "description": "Wygląd witryny."
    }
  ]
}
"#;

    pub const THEME_RENDERER: &str = r#"import type { ExtensionRenderer, ThemeModule } from '@nexo-cms/theme-sdk';

import './theme.css';

/** {{name}}: the base theme with its own colours (`theme.css`). */
const {{key}}: Partial<ThemeModule> = {};

export default { themes: { {{key}} } } satisfies ExtensionRenderer;
"#;

    pub const THEME_CSS: &str = r#"/* {{name}}: colours and type on the theme SDK's tokens. */
[data-theme='{{key}}'] {
	--color-primary: oklch(0.45 0.12 260);
}
"#;

    pub const THEME_README: &str = r#"# {{name}}

`{{id}}`: a theme of Nexo CMS. It changes looks only: CSS on `data-slot`
hooks, whole blocks, widgets and parts (`renderer/index.ts`).

```bash
nexo package check
nexo package pack       # {{id}}-<version>.zip for Administracja › Motywy
```
"#;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_come_from_the_id() {
        assert_eq!(key_of("com.example.hello"), "hello");
        assert_eq!(key_of("pl.firma.stany-hurtowni"), "stany_hurtowni");
        assert_eq!(key_of("com.example.2fa"), "x2fa");
        assert!(valid_id("com.example.hello"));
        assert!(!valid_id("hello"));
        assert!(!valid_id("Com.Example"));
    }

    #[test]
    fn renderer_imports_are_found() {
        let source = "import A from './A.svelte';\nimport { x } from \"@nexo-cms/theme-sdk\";\nimport 'lodash';";
        assert_eq!(
            imports(source),
            ["./A.svelte", "@nexo-cms/theme-sdk", "lodash"]
        );
    }

    #[test]
    fn paths_are_relative() {
        assert_eq!(
            relative(
                Path::new("/a/b/ext/pkg/backend"),
                Path::new("/a/b/sdk/extension")
            ),
            "../../../sdk/extension"
        );
    }

    #[test]
    fn a_scaffold_checks_and_packs() {
        let root = tempfile::tempdir().unwrap();
        new(
            &PackageKind::Extension,
            "com.example.hello",
            None,
            PackageBackend::None,
            Some(root.path().to_owned()),
        )
        .unwrap();
        let dir = root.path().join("com.example.hello");
        let (manifest, findings) = local_findings(&dir).unwrap();
        assert_eq!(manifest["key"], "hello");
        assert!(findings.errors.is_empty(), "{:?}", findings.errors);
        let (bytes, _) = archive(&dir).unwrap();
        let zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let names: Vec<&str> = zip.file_names().collect();
        assert!(names.contains(&"nexo.json") && names.contains(&"renderer/index.ts"));
        assert!(!names.iter().any(|n| n.starts_with(".gitignore")));
    }

    #[test]
    fn a_backend_must_be_built_and_imports_kept_in_bounds() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("com.example.bad");
        write(
            &dir.join("nexo.json"),
            r#"{"id":"com.example.bad","type":"extension","key":"bad","name":"Bad","version":"1.0",
                "publisher":{"name":"X"},"core":"^2.0","backend":{}}"#,
        )
        .unwrap();
        write(&dir.join("renderer/index.ts"), "import x from 'lodash';").unwrap();
        let (_, findings) = local_findings(&dir).unwrap();
        let all = findings.errors.join("\n");
        assert!(all.contains("`version`"), "{all}");
        assert!(all.contains("brak backend.wasm"), "{all}");
        assert!(all.contains("„lodash”"), "{all}");
    }
}
