//! Cuenta de Twitch por OAuth (Device Code Flow, sin servidor ni secreto: solo el Client ID).
//! Con la cuenta conectada Chispa obtiene la clave de stream sola, cambia título y categoría
//! y pone marcadores. Los tokens van al Administrador de credenciales de Windows.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config;

const SCOPES: &str = "channel:read:stream_key channel:manage:broadcast";
const TOKEN_SECRET: &str = "twitch-oauth";

#[derive(Serialize, Deserialize, Clone)]
struct Tokens {
    access: String,
    refresh: String,
    user_id: String,
    login: String,
}

/// Lo que se muestra al usuario para autorizar
#[derive(Serialize, Clone)]
pub struct DeviceStart {
    pub user_code: String,
    pub verification_uri: String,
    #[serde(skip)]
    device_code: String,
    #[serde(skip)]
    interval: u64,
    #[serde(skip)]
    expires_in: u64,
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).build()
}

fn err_text(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, r) => {
            let body = r.into_string().unwrap_or_default();
            let msg = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| v["message"].as_str().map(String::from))
                .unwrap_or(body);
            format!("Twitch respondió {code}: {msg}")
        }
        other => format!("No pude hablar con Twitch: {other}"),
    }
}

/// Paso 1: pedir un código para que el usuario lo autorice en twitch.tv/activate
pub fn device_start(client_id: &str) -> Result<DeviceStart, String> {
    if client_id.trim().is_empty() {
        return Err("Falta el Client ID de Twitch (creá una app en dev.twitch.tv/console, tipo \"Public\")".into());
    }
    let v: Value = agent()
        .post("https://id.twitch.tv/oauth2/device")
        .send_form(&[("client_id", client_id.trim()), ("scopes", SCOPES)])
        .map_err(err_text)?
        .into_json()
        .map_err(|e| e.to_string())?;
    Ok(DeviceStart {
        user_code: v["user_code"].as_str().unwrap_or_default().into(),
        verification_uri: v["verification_uri"].as_str().unwrap_or("https://www.twitch.tv/activate").into(),
        device_code: v["device_code"].as_str().unwrap_or_default().into(),
        interval: v["interval"].as_u64().unwrap_or(5),
        expires_in: v["expires_in"].as_u64().unwrap_or(1800),
    })
}

/// Paso 2: esperar a que el usuario autorice. Bloquea (llamar en un hilo). Devuelve el login del canal.
pub fn device_wait(client_id: &str, start: &DeviceStart) -> Result<String, String> {
    let deadline = Instant::now() + Duration::from_secs(start.expires_in);
    let mut interval = start.interval.max(1);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_secs(interval));
        let r = agent().post("https://id.twitch.tv/oauth2/token").send_form(&[
            ("client_id", client_id.trim()),
            ("scopes", SCOPES),
            ("device_code", start.device_code.as_str()),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ]);
        match r {
            Ok(resp) => {
                let v: Value = resp.into_json().map_err(|e| e.to_string())?;
                let access = v["access_token"].as_str().unwrap_or_default().to_string();
                let refresh = v["refresh_token"].as_str().unwrap_or_default().to_string();
                let (user_id, login) = whoami(client_id, &access)?;
                save_tokens(&Tokens { access, refresh, user_id, login: login.clone() })?;
                return Ok(login);
            }
            Err(ureq::Error::Status(400, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                if body.contains("authorization_pending") {
                    continue;
                }
                if body.contains("slow_down") {
                    interval += 5;
                    continue;
                }
                return Err(format!("Twitch rechazó la autorización: {body}"));
            }
            Err(e) => return Err(err_text(e)),
        }
    }
    Err("Se venció el código. Probá conectar de nuevo.".into())
}

fn whoami(client_id: &str, access: &str) -> Result<(String, String), String> {
    let v: Value = agent()
        .get("https://api.twitch.tv/helix/users")
        .set("Client-Id", client_id.trim())
        .set("Authorization", &format!("Bearer {access}"))
        .call()
        .map_err(err_text)?
        .into_json()
        .map_err(|e| e.to_string())?;
    let u = &v["data"][0];
    Ok((u["id"].as_str().unwrap_or_default().into(), u["login"].as_str().unwrap_or_default().into()))
}

fn save_tokens(t: &Tokens) -> Result<(), String> {
    config::set_secret(TOKEN_SECRET, &serde_json::to_string(t).map_err(|e| e.to_string())?)
}

fn load_tokens() -> Option<Tokens> {
    config::get_secret(TOKEN_SECRET).and_then(|s| serde_json::from_str(&s).ok())
}

pub fn connected_login() -> Option<String> {
    load_tokens().map(|t| t.login)
}

pub fn disconnect() -> Result<(), String> {
    config::set_secret(TOKEN_SECRET, "")
}

fn refresh(client_id: &str, t: &Tokens) -> Result<Tokens, String> {
    let v: Value = agent()
        .post("https://id.twitch.tv/oauth2/token")
        .send_form(&[
            ("client_id", client_id.trim()),
            ("grant_type", "refresh_token"),
            ("refresh_token", t.refresh.as_str()),
        ])
        .map_err(err_text)?
        .into_json()
        .map_err(|e| e.to_string())?;
    let nt = Tokens {
        access: v["access_token"].as_str().unwrap_or_default().into(),
        refresh: v["refresh_token"].as_str().unwrap_or(&t.refresh).into(),
        ..t.clone()
    };
    save_tokens(&nt)?;
    Ok(nt)
}

/// Llama a la API de Twitch con el token; si venció, lo renueva una vez y reintenta.
fn helix(client_id: &str, method: &str, path: &str, body: Option<Value>) -> Result<(Value, Tokens), String> {
    let mut t = load_tokens().ok_or("Conectá tu cuenta de Twitch en Ajustes")?;
    for attempt in 0..2 {
        let req = agent()
            .request(method, &format!("https://api.twitch.tv/helix/{path}"))
            .set("Client-Id", client_id.trim())
            .set("Authorization", &format!("Bearer {}", t.access));
        let r = match &body {
            Some(b) => req.send_json(b.clone()),
            None => req.call(),
        };
        match r {
            Ok(resp) => {
                let v = resp.into_json::<Value>().unwrap_or(Value::Null);
                return Ok((v, t));
            }
            Err(ureq::Error::Status(401, _)) if attempt == 0 => t = refresh(client_id, &t)?,
            Err(e) => return Err(err_text(e)),
        }
    }
    Err("Twitch no aceptó el token: volvé a conectar la cuenta".into())
}

/// Trae la clave de transmisión de la cuenta conectada y la guarda.
pub fn fetch_stream_key(client_id: &str) -> Result<(), String> {
    let t = load_tokens().ok_or("Conectá tu cuenta de Twitch en Ajustes")?;
    let (v, _) = helix(client_id, "GET", &format!("streams/key?broadcaster_id={}", t.user_id), None)?;
    let key = v["data"][0]["stream_key"].as_str().ok_or("Twitch no devolvió la clave")?;
    config::set_key("twitch", key)
}

/// Cambia título y/o categoría del canal. La categoría se busca por nombre.
pub fn update_channel(client_id: &str, title: &str, category: &str) -> Result<String, String> {
    let t = load_tokens().ok_or("Conectá tu cuenta de Twitch en Ajustes")?;
    let mut body = json!({});
    if !title.trim().is_empty() {
        body["title"] = json!(title.trim());
    }
    let mut cat_name = String::new();
    if !category.trim().is_empty() {
        let q = urlencode(category.trim());
        let (v, _) = helix(client_id, "GET", &format!("search/categories?first=1&query={q}"), None)?;
        let c = &v["data"][0];
        let id = c["id"].as_str().ok_or_else(|| format!("No encontré la categoría \"{}\"", category.trim()))?;
        cat_name = c["name"].as_str().unwrap_or_default().to_string();
        body["game_id"] = json!(id);
    }
    if body.as_object().is_some_and(|o| o.is_empty()) {
        return Err("Escribí un título o una categoría".into());
    }
    helix(client_id, "PATCH", &format!("channels?broadcaster_id={}", t.user_id), Some(body))?;
    Ok(if cat_name.is_empty() { "Título actualizado en Twitch".into() } else { format!("Twitch actualizado · {cat_name}") })
}

/// Marcador en el stream (para encontrar el momento después, en el VOD)
pub fn marker(client_id: &str) -> Result<String, String> {
    let t = load_tokens().ok_or("Para marcadores conectá tu cuenta de Twitch")?;
    helix(client_id, "POST", "streams/markers", Some(json!({ "user_id": t.user_id, "description": "Chispa" })))?;
    Ok("Marcador puesto".into())
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn urlencode_works() {
        assert_eq!(super::urlencode("Just Chatting"), "Just%20Chatting");
        assert_eq!(super::urlencode("Pokémon"), "Pok%C3%A9mon");
    }
}
