use anyhow::{Context, Result, bail};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

const APP_DIR_NAME: &str = "AssinadorLivre";
const CONFIG_FILE_NAME: &str = "config.json";
const LOG_DIR_NAME: &str = "logs";
const LOG_FILE_NAME: &str = "assinador.log";

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub base_dir: PathBuf,
    pub config_path: PathBuf,
    pub log_dir: PathBuf,
    pub log_file: PathBuf,
}

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: AppConfig,
    pub paths: AppPaths,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub ws_host: String,
    pub ws_port: u16,
    pub ws_path: String,
    pub ws_token: String,
    pub allowed_origins: Vec<String>,
    pub cert_override: CertOverride,
    pub startup_with_windows: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CertOverride {
    pub mode: String,
    pub thumbprint: Option<String>,
    pub index: Option<usize>,
}

impl Default for CertOverride {
    fn default() -> Self {
        Self {
            mode: "auto".to_string(),
            thumbprint: None,
            index: None,
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            ws_host: "127.0.0.1".to_string(),
            ws_port: 45890,
            ws_path: "/ws".to_string(),
            ws_token: "troque-este-token".to_string(),
            allowed_origins: vec![
                "http://localhost:3000".to_string(),
                "https://seu-dominio.com".to_string(),
            ],
            cert_override: CertOverride::default(),
            startup_with_windows: true,
        }
    }
}

impl AppConfig {
    pub fn endpoint(&self) -> String {
        format!("ws://{}:{}{}", self.ws_host, self.ws_port, self.ws_path)
    }

    pub fn normalized_ws_path(&self) -> String {
        let trimmed = self.ws_path.trim();
        if trimmed.starts_with('/') {
            trimmed.to_string()
        } else {
            format!("/{trimmed}")
        }
    }

    pub fn normalized_allowed_origins(&self) -> Vec<String> {
        self.allowed_origins
            .iter()
            .map(|origin| origin.trim().to_ascii_lowercase())
            .filter(|origin| !origin.is_empty())
            .collect()
    }
}

pub fn load_or_create() -> Result<LoadedConfig> {
    let paths = discover_paths()?;
    fs::create_dir_all(&paths.base_dir)
        .with_context(|| format!("Falha ao criar {}", paths.base_dir.display()))?;
    fs::create_dir_all(&paths.log_dir)
        .with_context(|| format!("Falha ao criar {}", paths.log_dir.display()))?;

    let mut config = if paths.config_path.exists() {
        let raw = fs::read_to_string(&paths.config_path)
            .with_context(|| format!("Falha ao ler {}", paths.config_path.display()))?;
        serde_json::from_str::<AppConfig>(&raw).with_context(|| {
            format!(
                "JSON de configuracao invalido em {}",
                paths.config_path.display()
            )
        })?
    } else {
        let default_cfg = AppConfig::default();
        save_config(&paths.config_path, &default_cfg)?;
        default_cfg
    };

    normalize_and_validate(&mut config)?;

    Ok(LoadedConfig { config, paths })
}

pub fn discover_paths() -> Result<AppPaths> {
    let base_dirs =
        BaseDirs::new().context("Nao foi possivel localizar diretorio base do usuario")?;
    let base_dir = base_dirs.data_dir().join(APP_DIR_NAME);
    let config_path = base_dir.join(CONFIG_FILE_NAME);
    let log_dir = base_dir.join(LOG_DIR_NAME);
    let log_file = log_dir.join(LOG_FILE_NAME);

    Ok(AppPaths {
        base_dir,
        config_path,
        log_dir,
        log_file,
    })
}

fn save_config(path: &PathBuf, config: &AppConfig) -> Result<()> {
    let content = serde_json::to_string_pretty(config)?;
    fs::write(path, content).with_context(|| format!("Falha ao escrever {}", path.display()))
}

fn normalize_and_validate(config: &mut AppConfig) -> Result<()> {
    config.ws_host = config.ws_host.trim().to_string();
    config.ws_path = config.normalized_ws_path();
    config.ws_token = config.ws_token.trim().to_string();
    config.cert_override.mode = config.cert_override.mode.trim().to_ascii_lowercase();

    if config.ws_host.is_empty() {
        bail!("ws_host nao pode ser vazio");
    }
    if config.ws_port == 0 {
        bail!("ws_port invalido: {}", config.ws_port);
    }
    if config.ws_token.is_empty() {
        bail!("ws_token nao pode ser vazio");
    }
    if config.ws_path == "/" {
        bail!("ws_path deve conter um path especifico (ex.: /ws)");
    }
    if config.cert_override.mode.is_empty() {
        config.cert_override.mode = "auto".to_string();
    }
    if config.cert_override.mode != "auto" && config.cert_override.mode != "token_only" {
        bail!(
            "cert_override.mode invalido: '{}'. Valores aceitos: auto, token_only",
            config.cert_override.mode
        );
    }

    if let Some(index) = config.cert_override.index {
        if index == 0 {
            bail!("cert_override.index deve ser >= 1");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_ws_path_adds_slash() {
        let mut cfg = AppConfig::default();
        cfg.ws_path = "ws".to_string();
        normalize_and_validate(&mut cfg).expect("config should be valid");
        assert_eq!(cfg.ws_path, "/ws");
    }

    #[test]
    fn rejects_empty_token() {
        let mut cfg = AppConfig::default();
        cfg.ws_token = "   ".to_string();
        assert!(normalize_and_validate(&mut cfg).is_err());
    }

    #[test]
    fn accepts_token_only_mode() {
        let mut cfg = AppConfig::default();
        cfg.cert_override.mode = "token_only".to_string();
        assert!(normalize_and_validate(&mut cfg).is_ok());
    }
}
