//! Configuración de la app: %APPDATA%\Chispa\config.json.
//! Las claves de transmisión y los tokens NO van acá: se guardan en el Administrador de credenciales de Windows.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const KEYRING_SERVICE: &str = "Chispa";
pub const PLATFORMS: [&str; 3] = ["twitch", "youtube", "kick"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Destinos activos al salir en vivo (uno o varios): "twitch" | "youtube" | "kick"
    pub targets: Vec<String>,
    /// Kick da una URL de ingest propia por canal (rtmps://...)
    pub kick_url: String,
    /// Client ID de una app registrada en dev.twitch.tv (para conectar la cuenta)
    pub twitch_client_id: String,
    /// Canal de Twitch (login). Se completa solo al conectar la cuenta; sirve para el chat.
    pub twitch_channel: String,
    /// Canal de Kick (como aparece en kick.com/<canal>), para el chat
    pub kick_channel: String,
    /// "es" | "en"
    pub lang: String,
    /// Comandos de voz ("Chispa, clipeá eso")
    pub voice: bool,
    /// Ver el chat unificado
    pub chat: bool,
    /// Mostrar alertas (subs, raids, bits) también en el stream
    pub alerts_on_stream: bool,

    // Compatibilidad con la config de la Fase 1
    #[serde(skip_serializing)]
    platform: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            targets: vec!["twitch".into()],
            kick_url: String::new(),
            twitch_client_id: String::new(),
            twitch_channel: String::new(),
            kick_channel: String::new(),
            lang: "es".into(),
            voice: false,
            chat: true,
            alerts_on_stream: true,
            platform: None,
        }
    }
}

fn path() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("Chispa").join("config.json")
}

pub fn load() -> Config {
    let mut cfg: Config =
        std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    if let Some(p) = cfg.platform.take() {
        if PLATFORMS.contains(&p.as_str()) && cfg.targets == vec!["twitch".to_string()] {
            cfg.targets = vec![p];
        }
    }
    cfg.targets.retain(|t| PLATFORMS.contains(&t.as_str()));
    cfg.targets.dedup();
    cfg
}

pub fn save(cfg: &Config) -> Result<(), String> {
    let p = path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(p, json).map_err(|e| e.to_string())
}

pub fn platform_name(id: &str) -> &'static str {
    match id {
        "youtube" => "YouTube",
        "kick" => "Kick",
        _ => "Twitch",
    }
}

/// Acepta "Twitch", "twitch", "kick"… y devuelve el id
pub fn platform_id(name: &str) -> Option<&'static str> {
    let n = name.trim().to_lowercase();
    PLATFORMS.into_iter().find(|p| *p == n)
}

/// URL del servidor RTMP de cada plataforma. Kick usa la URL que el usuario pega.
pub fn platform_url(id: &str, cfg: &Config) -> Result<String, String> {
    match id {
        "twitch" => Ok("rtmp://live.twitch.tv/app".into()),
        "youtube" => Ok("rtmp://a.rtmp.youtube.com/live2".into()),
        "kick" if !cfg.kick_url.trim().is_empty() => Ok(cfg.kick_url.trim().into()),
        "kick" => Err("Falta la URL del servidor de Kick (la encontrás en tu panel de Kick, sección Stream)".into()),
        other => Err(format!("Plataforma desconocida: {other}")),
    }
}

pub fn get_secret(name: &str) -> Option<String> {
    keyring::Entry::new(KEYRING_SERVICE, name).ok()?.get_password().ok().filter(|k| !k.is_empty())
}

pub fn set_secret(name: &str, value: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, name).map_err(|e| e.to_string())?;
    if value.is_empty() {
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    } else {
        entry.set_password(value).map_err(|e| e.to_string())
    }
}

/// Clave de transmisión de una plataforma
pub fn get_key(platform: &str) -> Option<String> {
    get_secret(platform)
}

pub fn set_key(platform: &str, key: &str) -> Result<(), String> {
    set_secret(platform, key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_phase1_platform() {
        let cfg: Config = serde_json::from_str(r#"{"platform":"kick","kick_url":"rtmps://x"}"#).unwrap();
        let mut cfg = cfg;
        if let Some(p) = cfg.platform.take() {
            cfg.targets = vec![p];
        }
        assert_eq!(cfg.targets, vec!["kick".to_string()]);
    }

    #[test]
    fn platform_ids() {
        assert_eq!(platform_id("Twitch"), Some("twitch"));
        assert_eq!(platform_id("otra"), None);
    }
}
