//! `config.toml`: the instances (profiles) the CLI talks to and image settings.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Where the CLI keeps its files: `$NEXO_HOME`, or the system's config folder
/// (`~/Library/Application Support/nexo`, `~/.config/nexo`, `%APPDATA%\nexo`).
#[derive(Debug, Clone)]
pub struct Paths {
    pub dir: PathBuf,
}

impl Paths {
    pub fn discover() -> Result<Self> {
        if let Some(home) = std::env::var_os("NEXO_HOME") {
            return Ok(Self { dir: home.into() });
        }
        let dirs = directories::ProjectDirs::from("", "", "nexo")
            .ok_or_else(|| Error::Config("brak folderu domowego na ustawienia".into()))?;
        Ok(Self {
            dir: dirs.config_dir().to_path_buf(),
        })
    }

    pub fn config_file(&self) -> PathBuf {
        self.dir.join("config.toml")
    }

    /// Serialises token refreshes of one profile across processes.
    pub fn lock_file(&self, profile: &str) -> PathBuf {
        self.dir.join("locks").join(format!("{profile}.lock"))
    }

    /// Fallback credential files when the OS keychain is not available.
    pub fn credentials_dir(&self) -> PathBuf {
        self.dir.join("credentials")
    }

    /// Prompt files that override the built-in image presets.
    pub fn prompts_dir(&self) -> PathBuf {
        self.dir.join("prompts")
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
    #[serde(default)]
    pub images: ImagesConfig,
}

/// One Nexo CMS installation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    /// Address of the panel (`https://cms.example.pl`); the API is reached
    /// through it.
    pub url: String,
    /// Site used when a command gets no `--site` (key or `site_…` id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImagesConfig {
    pub model: String,
    pub aspect_ratio: String,
    /// `1K`, `2K` or `4K`.
    pub image_size: String,
    /// JPEG quality of saved results.
    pub quality: u8,
}

impl Default for ImagesConfig {
    fn default() -> Self {
        Self {
            model: "gemini-3.1-flash-image".into(),
            aspect_ratio: "3:4".into(),
            image_size: "2K".into(),
            quality: 90,
        }
    }
}

impl Config {
    pub fn load(paths: &Paths) -> Result<Self> {
        let file = paths.config_file();
        match std::fs::read_to_string(&file) {
            Ok(text) => {
                toml::from_str(&text).map_err(|e| Error::Config(format!("{}: {e}", file.display())))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        let text = toml::to_string_pretty(self).map_err(|e| Error::Config(e.to_string()))?;
        write_private(&paths.config_file(), text.as_bytes())
    }

    /// The profile a command uses: `--profile`, `NEXO_PROFILE`, the default,
    /// or the only one there is.
    pub fn profile_name(&self, requested: Option<&str>) -> Result<String> {
        if let Some(name) = requested {
            return Ok(name.to_owned());
        }
        if let Some(name) = &self.default_profile {
            return Ok(name.clone());
        }
        match self.profiles.keys().next() {
            Some(name) if self.profiles.len() == 1 => Ok(name.clone()),
            Some(_) => Err(Error::Config(
                "jest kilka profili i żaden nie jest domyślny; podaj --profile albo `nexo profile use <nazwa>`"
                    .into(),
            )),
            None => Err(Error::Config(
                "nie ma jeszcze profilu; uruchom `nexo login --url https://cms.twoja-domena.pl`".into(),
            )),
        }
    }

    pub fn profile(&self, name: &str) -> Result<&Profile> {
        self.profiles.get(name).ok_or_else(|| {
            Error::Config(format!(
                "nie ma profilu `{name}`; uruchom `nexo login --profile {name} --url …`"
            ))
        })
    }
}

/// `https://cms.example.pl/` → `https://cms.example.pl`; anything that is not
/// an http(s) address is refused.
pub fn normalize_url(url: &str) -> Result<String> {
    let url = url.trim().trim_end_matches('/');
    let local = |rest: &str| {
        let host = rest.split(['/', ':']).next().unwrap_or_default();
        host == "localhost" || host == "127.0.0.1" || host.ends_with(".localhost")
    };
    let ok = url
        .strip_prefix("https://")
        .is_some_and(|rest| !rest.is_empty())
        || url.strip_prefix("http://").is_some_and(local);
    if !ok {
        return Err(Error::Config(format!(
            "`{url}` to nie adres panelu; użyj https:// (http:// tylko dla localhost)"
        )));
    }
    Ok(url.to_owned())
}

/// Writes a file readable only by its owner (settings and credentials).
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        restrict(parent, 0o700)?;
    }
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, bytes)?;
    restrict(&temp, 0o600)?;
    std::fs::rename(&temp, path)?;
    Ok(())
}

#[cfg(unix)]
fn restrict(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_panel_addresses() {
        assert_eq!(
            normalize_url("https://cms.comfyroom.pl/").unwrap(),
            "https://cms.comfyroom.pl"
        );
        assert_eq!(
            normalize_url("http://localhost:35173").unwrap(),
            "http://localhost:35173"
        );
        assert_eq!(
            normalize_url("http://admin.localhost").unwrap(),
            "http://admin.localhost"
        );
    }

    #[test]
    fn refuses_plain_http_elsewhere() {
        assert!(normalize_url("http://cms.comfyroom.pl").is_err());
        assert!(normalize_url("http://localhost.evil.pl").is_err());
        assert!(normalize_url("cms.comfyroom.pl").is_err());
    }

    #[test]
    fn picks_the_profile() {
        let mut config = Config::default();
        assert!(config.profile_name(None).is_err());
        config.profiles.insert(
            "comfyroom".into(),
            Profile {
                url: "https://cms.comfyroom.pl".into(),
                site: None,
            },
        );
        assert_eq!(config.profile_name(None).unwrap(), "comfyroom");
        config.profiles.insert(
            "local".into(),
            Profile {
                url: "http://localhost".into(),
                site: None,
            },
        );
        assert!(config.profile_name(None).is_err());
        assert_eq!(config.profile_name(Some("local")).unwrap(), "local");
        config.default_profile = Some("comfyroom".into());
        assert_eq!(config.profile_name(None).unwrap(), "comfyroom");
    }

    #[test]
    fn saves_privately() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            dir: dir.path().join("nexo"),
        };
        let config = Config {
            default_profile: Some("x".into()),
            ..Config::default()
        };
        config.save(&paths).unwrap();
        assert_eq!(
            Config::load(&paths).unwrap().default_profile.as_deref(),
            Some("x")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(paths.config_file())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
