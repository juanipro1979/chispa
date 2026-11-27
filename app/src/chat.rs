//! Chat unificado: Twitch (IRC anónimo, sin login) y Kick (websocket público de Pusher).
//! También saca de ahí las alertas: subs, regalos, raids y bits.
//! YouTube necesita una API key de Google para leer el chat; queda para cuando haya OAuth de YouTube.

use std::{
    collections::HashMap,
    io::ErrorKind,
    net::TcpStream,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use serde::Serialize;
use serde_json::{json, Value};
use tungstenite::{stream::MaybeTlsStream, Message, WebSocket};

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ChatEvent {
    pub platform: &'static str,
    pub user: String,
    pub text: String,
    pub color: Option<String>,
    /// "msg" | "alert" | "info"
    pub kind: &'static str,
}

pub type Sink = Arc<dyn Fn(ChatEvent) + Send + Sync>;

/// Maneja las conexiones de chat. Al soltarlo, se cortan.
pub struct Chat {
    stop: Arc<AtomicBool>,
}

impl Drop for Chat {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Chat {
    pub fn start(twitch: &str, kick: &str, sink: Sink) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let twitch = twitch.trim().trim_start_matches('#').to_lowercase();
        if !twitch.is_empty() {
            let (s, k) = (stop.clone(), sink.clone());
            std::thread::spawn(move || run_forever("Twitch", &s, &k, || twitch_session(&twitch, &s, &k)));
        }
        let kick = kick.trim().to_lowercase();
        if !kick.is_empty() {
            let (s, k) = (stop.clone(), sink);
            std::thread::spawn(move || run_forever("Kick", &s, &k, || kick_session(&kick, &s, &k)));
        }
        Self { stop }
    }
}

/// Reconecta con espera creciente hasta que se pida parar.
fn run_forever(name: &'static str, stop: &AtomicBool, sink: &Sink, mut session: impl FnMut() -> Result<(), String>) {
    let mut wait = 2;
    while !stop.load(Ordering::Relaxed) {
        match session() {
            Ok(()) => wait = 2,
            Err(e) => {
                sink(ChatEvent { platform: name_id(name), user: String::new(), text: format!("Chat de {name}: {e}"), color: None, kind: "info" });
                wait = (wait * 2).min(60);
            }
        }
        for _ in 0..wait {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

fn name_id(name: &str) -> &'static str {
    if name == "Kick" { "kick" } else { "twitch" }
}

type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

fn connect(url: &str) -> Result<Ws, String> {
    let (ws, _) = tungstenite::connect(url).map_err(|e| format!("no pude conectar ({e})"))?;
    // Timeout de lectura para poder cortar cuando se pide
    let tcp = match ws.get_ref() {
        MaybeTlsStream::Plain(s) => Some(s),
        MaybeTlsStream::Rustls(s) => Some(&s.sock),
        _ => None,
    };
    if let Some(t) = tcp {
        let _ = t.set_read_timeout(Some(Duration::from_secs(1)));
    }
    Ok(ws)
}

/// Lee el próximo mensaje de texto; None si fue timeout (para revisar `stop`).
fn read_text(ws: &mut Ws) -> Result<Option<String>, String> {
    match ws.read() {
        Ok(Message::Text(t)) => Ok(Some(t.to_string())),
        Ok(Message::Close(_)) => Err("se cerró la conexión".into()),
        Ok(_) => Ok(None),
        Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

// ---------------- Twitch ----------------

fn twitch_session(channel: &str, stop: &AtomicBool, sink: &Sink) -> Result<(), String> {
    let mut ws = connect("wss://irc-ws.chat.twitch.tv:443")?;
    let nick = format!("justinfan{}", 10000 + std::process::id() % 80000);
    for line in ["CAP REQ :twitch.tv/tags twitch.tv/commands".to_string(), format!("NICK {nick}"), format!("JOIN #{channel}")] {
        ws.send(Message::text(line)).map_err(|e| e.to_string())?;
    }
    sink(ChatEvent { platform: "twitch", user: String::new(), text: format!("Conectado al chat de twitch.tv/{channel}"), color: None, kind: "info" });
    while !stop.load(Ordering::Relaxed) {
        let Some(text) = read_text(&mut ws)? else { continue };
        for line in text.split("\r\n").filter(|l| !l.is_empty()) {
            if line.starts_with("PING") {
                ws.send(Message::text(line.replacen("PING", "PONG", 1))).map_err(|e| e.to_string())?;
                continue;
            }
            if let Some(ev) = parse_twitch(line) {
                sink(ev);
            }
        }
    }
    let _ = ws.close(None);
    Ok(())
}

/// Convierte una línea IRC de Twitch en un evento de chat (mensaje o alerta).
pub fn parse_twitch(line: &str) -> Option<ChatEvent> {
    let (tags, rest) = match line.strip_prefix('@') {
        Some(r) => {
            let (t, rest) = r.split_once(' ')?;
            (parse_tags(t), rest)
        }
        None => (HashMap::new(), line),
    };
    // ":user!user@host COMMAND #chan :texto"
    let rest = rest.strip_prefix(':').unwrap_or(rest);
    let (prefix, rest) = rest.split_once(' ')?;
    let (command, rest) = rest.split_once(' ').unwrap_or((rest, ""));
    let trailing = rest.split_once(" :").map(|(_, t)| t).or_else(|| rest.strip_prefix(':')).unwrap_or("");
    let login = prefix.split('!').next().unwrap_or_default();
    let user = tags.get("display-name").filter(|d| !d.is_empty()).cloned().unwrap_or_else(|| login.to_string());
    let color = tags.get("color").filter(|c| !c.is_empty()).cloned();

    match command {
        "PRIVMSG" => {
            if let Some(bits) = tags.get("bits") {
                return Some(ChatEvent {
                    platform: "twitch",
                    user: user.clone(),
                    text: format!("{user} mandó {bits} bits: {trailing}"),
                    color,
                    kind: "alert",
                });
            }
            Some(ChatEvent { platform: "twitch", user, text: trailing.to_string(), color, kind: "msg" })
        }
        "USERNOTICE" => {
            let id = tags.get("msg-id").map(String::as_str).unwrap_or_default();
            if !matches!(id, "sub" | "resub" | "subgift" | "submysterygift" | "raid" | "giftpaidupgrade") {
                return None;
            }
            let sys = tags.get("system-msg").cloned().unwrap_or_default();
            let text = if trailing.is_empty() { sys } else { format!("{sys} «{trailing}»") };
            Some(ChatEvent { platform: "twitch", user, text, color, kind: "alert" })
        }
        _ => None,
    }
}

fn parse_tags(s: &str) -> HashMap<String, String> {
    s.split(';')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| {
            let v = v.replace("\\s", " ").replace("\\:", ";").replace("\\\\", "\\").replace("\\r", "").replace("\\n", " ");
            (k.to_string(), v)
        })
        .collect()
}

// ---------------- Kick ----------------

const KICK_PUSHER: &str = "wss://ws-us2.pusher.com/app/32cbd69e4b950bf97679?protocol=7&client=js&version=8.4.0&flash=false";

/// El chat de Kick se identifica por el número de chatroom. Si el usuario pegó un número, se usa
/// directo; si no, se busca por el nombre del canal (la API de Kick a veces bloquea esto).
fn kick_chatroom(channel: &str) -> Result<u64, String> {
    if let Ok(id) = channel.parse::<u64>() {
        return Ok(id);
    }
    let v: Value = ureq::get(&format!("https://kick.com/api/v2/channels/{channel}"))
        .set("Accept", "application/json")
        .set("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(|_| "Kick no dejó buscar el canal. Pegá el número de chatroom en Ajustes (ver README)".to_string())?
        .into_json()
        .map_err(|e| e.to_string())?;
    v["chatroom"]["id"].as_u64().ok_or_else(|| "no encontré el chatroom de ese canal".into())
}

fn kick_session(channel: &str, stop: &AtomicBool, sink: &Sink) -> Result<(), String> {
    let room = kick_chatroom(channel)?;
    let mut ws = connect(KICK_PUSHER)?;
    for ch in [format!("chatrooms.{room}.v2"), format!("chatroom_{room}")] {
        let sub = json!({ "event": "pusher:subscribe", "data": { "auth": "", "channel": ch } });
        ws.send(Message::text(sub.to_string())).map_err(|e| e.to_string())?;
    }
    sink(ChatEvent { platform: "kick", user: String::new(), text: format!("Conectado al chat de kick.com/{channel}"), color: None, kind: "info" });
    while !stop.load(Ordering::Relaxed) {
        let Some(text) = read_text(&mut ws)? else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
        let event = v["event"].as_str().unwrap_or_default();
        if event == "pusher:ping" {
            ws.send(Message::text(json!({ "event": "pusher:pong", "data": {} }).to_string())).map_err(|e| e.to_string())?;
            continue;
        }
        // En Pusher, "data" viene como texto JSON
        let data: Value = v["data"].as_str().and_then(|d| serde_json::from_str(d).ok()).unwrap_or(Value::Null);
        if let Some(ev) = parse_kick(event, &data) {
            sink(ev);
        }
    }
    let _ = ws.close(None);
    Ok(())
}

pub fn parse_kick(event: &str, d: &Value) -> Option<ChatEvent> {
    match event {
        "App\\Events\\ChatMessageEvent" => Some(ChatEvent {
            platform: "kick",
            user: d["sender"]["username"].as_str()?.to_string(),
            text: strip_kick_emotes(d["content"].as_str().unwrap_or_default()),
            color: d["sender"]["identity"]["color"].as_str().map(String::from),
            kind: "msg",
        }),
        "App\\Events\\SubscriptionEvent" => {
            let u = d["username"].as_str()?.to_string();
            let months = d["months"].as_u64().unwrap_or(1);
            Some(ChatEvent { platform: "kick", text: format!("{u} se suscribió ({months} meses)"), user: u, color: None, kind: "alert" })
        }
        "App\\Events\\GiftedSubscriptionsEvent" => {
            let u = d["gifter_username"].as_str()?.to_string();
            let n = d["gifted_usernames"].as_array().map(|a| a.len()).unwrap_or(1);
            Some(ChatEvent { platform: "kick", text: format!("{u} regaló {n} suscripciones"), user: u, color: None, kind: "alert" })
        }
        "App\\Events\\StreamHostEvent" => {
            let u = d["host_username"].as_str()?.to_string();
            let n = d["number_viewers"].as_u64().unwrap_or(0);
            Some(ChatEvent { platform: "kick", text: format!("{u} hostea con {n} espectadores"), user: u, color: None, kind: "alert" })
        }
        _ => None,
    }
}

/// "[emote:123:Kappa] hola" → ":Kappa: hola"
fn strip_kick_emotes(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("[emote:") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 7..];
        match after.find(']') {
            Some(j) => {
                let name = after[..j].rsplit(':').next().unwrap_or_default();
                out.push_str(&format!(":{name}:"));
                rest = &after[j + 1..];
            }
            None => {
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twitch_privmsg() {
        let l = "@color=#FF0000;display-name=Juani;emotes= :juani!juani@juani.tmi.twitch.tv PRIVMSG #canal :hola chat";
        let e = parse_twitch(l).unwrap();
        assert_eq!((e.user.as_str(), e.text.as_str(), e.kind), ("Juani", "hola chat", "msg"));
        assert_eq!(e.color.as_deref(), Some("#FF0000"));
    }

    #[test]
    fn twitch_sub_alert() {
        let l = "@display-name=Pepe;msg-id=resub;system-msg=Pepe\\ssubscribed\\sfor\\s3\\smonths. :tmi.twitch.tv USERNOTICE #canal :vamos";
        let e = parse_twitch(l).unwrap();
        assert_eq!(e.kind, "alert");
        assert_eq!(e.text, "Pepe subscribed for 3 months. «vamos»");
    }

    #[test]
    fn twitch_bits() {
        let l = "@bits=100;display-name=Ana :ana!ana@ana.tmi.twitch.tv PRIVMSG #canal :cheer100 grande";
        assert_eq!(parse_twitch(l).unwrap().kind, "alert");
    }

    #[test]
    fn twitch_ignores_join() {
        assert!(parse_twitch(":juani!juani@juani.tmi.twitch.tv JOIN #canal").is_none());
    }

    #[test]
    fn kick_message() {
        let d = json!({ "content": "[emote:39:KEKW] jaja", "sender": { "username": "x", "identity": { "color": "#00FF00" } } });
        let e = parse_kick("App\\Events\\ChatMessageEvent", &d).unwrap();
        assert_eq!(e.text, ":KEKW: jaja");
    }

    #[test]
    fn kick_gift() {
        let d = json!({ "gifter_username": "rico", "gifted_usernames": ["a", "b"] });
        assert_eq!(parse_kick("App\\Events\\GiftedSubscriptionsEvent", &d).unwrap().text, "rico regaló 2 suscripciones");
    }
}
