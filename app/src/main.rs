// Sin consola en release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod chat;
mod config;
mod twitch;
mod voice;

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use chispa_core::{Command, EngineHandle, EngineOptions, SceneMode, Status, StreamTarget};
use serde::{Deserialize, Serialize};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, RunEvent,
};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

struct AppState {
    engine: EngineHandle,
    cfg: Mutex<config::Config>,
    chat: Mutex<Option<chat::Chat>>,
    chat_log: Mutex<VecDeque<chat::ChatEvent>>,
    voice: Mutex<Option<voice::Voice>>,
}

const CHAT_LOG_MAX: usize = 200;

/// Log de diagnóstico en %TEMP%\chispa-debug.log (solo con la variable CHISPA_DEBUG)
fn dlog(msg: &str) {
    if std::env::var_os("CHISPA_DEBUG").is_none() {
        return;
    }
    use std::io::Write;
    let p = std::env::temp_dir().join("chispa-debug.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let _ = writeln!(f, "{t} {msg}");
    }
}

/// Atajos globales. Alt+F-teclas porque Ctrl+Shift+letra choca con navegadores y con AMD Adrenalin.
const HOTKEYS: [(Code, &str, &str); 6] = [
    (Code::F10, "clip", "Alt+F10"),
    (Code::F11, "vertical", "Alt+F11"),
    (Code::F9, "record", "Alt+F9"),
    (Code::F8, "stream", "Alt+F8"),
    (Code::F7, "mic", "Alt+F7"),
    (Code::F6, "voice", "Alt+F6"),
];

fn targets_for(cfg: &config::Config, which: Option<&str>) -> Result<Vec<StreamTarget>, String> {
    let ids: Vec<String> = match which {
        None => cfg.targets.clone(),
        Some("all") => config::PLATFORMS.iter().filter(|p| config::get_key(p).is_some()).map(|p| p.to_string()).collect(),
        Some(p) => vec![config::platform_id(p).ok_or_else(|| format!("Plataforma desconocida: {p}"))?.to_string()],
    };
    if ids.is_empty() {
        return Err("Elegí al menos una plataforma en Ajustes".into());
    }
    ids.iter()
        .map(|id| {
            let key = config::get_key(id)
                .ok_or_else(|| format!("Falta la clave de {}: cargala en Ajustes", config::platform_name(id)))?;
            Ok(StreamTarget { name: config::platform_name(id).into(), url: config::platform_url(id, cfg)?, key })
        })
        .collect()
}

/// Ejecuta una acción. La usan los botones, los atajos, la bandeja y la voz.
fn do_action(app: &AppHandle, name: &str) -> Result<String, String> {
    let state = app.state::<AppState>();
    let st = state.engine.status();
    let (verb, arg) = name.split_once(':').map(|(a, b)| (a, Some(b))).unwrap_or((name, None));
    let cmd = match verb {
        "clip" => Command::Clip,
        "vertical" => Command::ClipVertical,
        "record" => Command::ToggleRecord,
        "record_start" if st.recording => return Ok("Ya estás grabando".into()),
        "record_stop" if !st.recording => return Ok("No estabas grabando".into()),
        "record_start" | "record_stop" => Command::ToggleRecord,
        "mic" => Command::ToggleMic,
        "mic_mute" if st.mic_muted => return Ok("El micrófono ya está muteado".into()),
        "mic_unmute" if !st.mic_muted => return Ok("El micrófono ya está activo".into()),
        "mic_mute" | "mic_unmute" => Command::ToggleMic,
        "replay" => Command::ToggleReplay,
        "camera" => Command::ToggleCamera,
        "camera_on" if st.camera_on => return Ok("La cámara ya está prendida".into()),
        "camera_off" if !st.camera_on => return Ok("La cámara ya está apagada".into()),
        "camera_on" | "camera_off" => Command::ToggleCamera,
        "scene" => Command::SetScene(SceneMode::parse(arg.unwrap_or_default()).ok_or("Escena desconocida")?),
        "stream" if st.streaming => Command::StopStream,
        "stream" | "stream_start" => {
            if st.streaming {
                return Ok("Ya estás en vivo".into());
            }
            let cfg = state.cfg.lock().unwrap().clone();
            Command::StartStream(targets_for(&cfg, arg)?)
        }
        "stream_stop" => match arg {
            Some(p) => Command::StopStreamTo(config::platform_name(config::platform_id(p).unwrap_or("twitch")).into()),
            None => Command::StopStream,
        },
        "marker" => {
            let id = state.cfg.lock().unwrap().twitch_client_id.clone();
            return twitch::marker(&id);
        }
        "voice" => return toggle_voice(app),
        other => return Err(format!("Acción desconocida: {other}")),
    };
    state.engine.send(cmd)
}

/// Igual que do_action, pero avisa en la UI (para atajos, bandeja y voz)
fn action_with_toast(app: AppHandle, name: &'static str) {
    std::thread::spawn(move || {
        let (ok, msg) = match do_action(&app, name) {
            Ok(m) => (true, m),
            Err(e) => (false, e),
        };
        toast(&app, ok, &msg);
        update_tray(&app);
    });
}

fn toast(app: &AppHandle, ok: bool, msg: &str) {
    let _ = app.emit("toast", serde_json::json!({ "ok": ok, "msg": msg }));
}

fn update_tray(app: &AppHandle) {
    let s = app.state::<AppState>().engine.status();
    let mut tip = String::from("Chispa");
    if s.streaming {
        tip.push_str(&format!(" · EN VIVO en {}", s.stream_target.unwrap_or_default()));
    }
    if s.recording {
        tip.push_str(" · grabando");
    }
    if s.replay {
        tip.push_str(" · clips listos (Alt+F10)");
    }
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(tip));
    }
}

// ---------- Voz ----------

fn toggle_voice(app: &AppHandle) -> Result<String, String> {
    let state = app.state::<AppState>();
    let on = state.voice.lock().unwrap().is_some();
    set_voice(app, !on)?;
    let mut cfg = state.cfg.lock().unwrap();
    cfg.voice = !on;
    let _ = config::save(&cfg);
    Ok(if on { "Voz apagada".into() } else { "Te escucho: decí \"Chispa, clipeá eso\"".into() })
}

fn set_voice(app: &AppHandle, on: bool) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut slot = state.voice.lock().unwrap();
    if !on {
        *slot = None;
        return Ok(());
    }
    if slot.is_some() {
        return Ok(());
    }
    let lang = state.cfg.lock().unwrap().lang.clone();
    let app2 = app.clone();
    let (v, _lang_name) = voice::Voice::start(&lang, move |action, heard| {
        let app3 = app2.clone();
        std::thread::spawn(move || {
            let (ok, msg) = match do_action(&app3, action) {
                Ok(m) => (true, format!("🎙 «{heard}» → {m}")),
                Err(e) => (false, format!("🎙 «{heard}» → {e}")),
            };
            toast(&app3, ok, &msg);
            update_tray(&app3);
        });
    })?;
    *slot = Some(v);
    Ok(())
}

// ---------- Chat y alertas ----------

fn restart_chat(app: &AppHandle) {
    let state = app.state::<AppState>();
    let cfg = state.cfg.lock().unwrap().clone();
    let mut slot = state.chat.lock().unwrap();
    *slot = None; // corta la anterior
    if !cfg.chat || (cfg.twitch_channel.trim().is_empty() && cfg.kick_channel.trim().is_empty()) {
        return;
    }
    let app2 = app.clone();
    let sink: chat::Sink = Arc::new(move |ev: chat::ChatEvent| {
        let st = app2.state::<AppState>();
        {
            let mut log = st.chat_log.lock().unwrap();
            log.push_back(ev.clone());
            while log.len() > CHAT_LOG_MAX {
                log.pop_front();
            }
        }
        if ev.kind == "alert" {
            toast(&app2, true, &format!("🎉 {}", ev.text));
            if st.cfg.lock().unwrap().alerts_on_stream && st.engine.status().streaming {
                st.engine.post(Command::ShowAlert(ev.text.clone()));
            }
        }
        let _ = app2.emit("chat", &ev);
    });
    *slot = Some(chat::Chat::start(&cfg.twitch_channel, &cfg.kick_channel, sink));
}

// ---------- Ventana ----------

/// Muestra la ventana. Si estaba destruida (Modo Stream / cerrada a la bandeja), la vuelve a crear.
fn show_window(app: &AppHandle) {
    let w = match app.get_webview_window("main") {
        Some(w) => w,
        None => match app.config().app.windows.first().map(|c| tauri::WebviewWindowBuilder::from_config(app, c)) {
            Some(Ok(b)) => match b.build() {
                Ok(w) => w,
                Err(e) => return eprintln!("[chispa] no pude abrir la ventana: {e}"),
            },
            _ => return,
        },
    };
    let _ = w.show();
    let _ = w.unminimize();
    let _ = w.set_focus();
}

/// Cierra la ventana del todo para liberar WebView2 (~300 MB). El motor sigue en la bandeja.
/// La vista previa vive dentro de la ventana: se cierra antes de destruirla. Se hace en otro hilo,
/// porque libobs necesita que el hilo principal siga atendiendo mensajes mientras saca la ventana hija.
fn close_window(app: &AppHandle) {
    dlog("close_window");
    let app = app.clone();
    std::thread::spawn(move || {
        let _ = app.state::<AppState>().engine.send(Command::Preview(None));
        let app2 = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(w) = app2.get_webview_window("main") {
                let _ = w.destroy();
            }
        });
    });
}

#[derive(Deserialize)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// Vista previa: `rect` en px CSS (la UI ya lo multiplica por devicePixelRatio). None la cierra.
#[tauri::command]
async fn preview(app: AppHandle, rect: Option<Rect>) -> Result<(), String> {
    dlog(&format!("preview {:?}", rect.as_ref().map(|r| (r.x, r.y, r.w, r.h))));
    let cmd = match rect {
        None => Command::Preview(None),
        Some(r) => {
            let w = app.get_webview_window("main").ok_or("sin ventana")?;
            let hwnd = w.hwnd().map_err(|e| e.to_string())?.0 as isize;
            Command::Preview(Some((hwnd, r.x.round() as i32, r.y.round() as i32, r.w.round() as u32, r.h.round() as u32)))
        }
    };
    let engine = app.state::<AppState>().engine.clone();
    tauri::async_runtime::spawn_blocking(move || engine.send(cmd)).await.map_err(|e| e.to_string())??;
    Ok(())
}

// ---------- Comandos para la UI ----------

#[tauri::command]
fn status(state: tauri::State<'_, AppState>) -> Status {
    let mut s = state.engine.status();
    s.notice = state.engine.take_notice();
    s
}

#[tauri::command]
async fn action(app: AppHandle, name: String) -> Result<String, String> {
    let app2 = app.clone();
    let r = tauri::async_runtime::spawn_blocking(move || do_action(&app2, &name))
        .await
        .map_err(|e| e.to_string())?;
    update_tray(&app);
    r
}

#[derive(Serialize)]
struct ConfigView {
    #[serde(flatten)]
    cfg: config::Config,
    keys: std::collections::HashMap<String, bool>,
    hotkeys: Vec<(String, String)>,
    hotkeys_failed: Vec<String>,
    twitch_login: Option<String>,
    voice_on: bool,
}

#[tauri::command]
fn get_config(state: tauri::State<'_, AppState>, failed: tauri::State<'_, HotkeysFailed>) -> ConfigView {
    let cfg = state.cfg.lock().unwrap().clone();
    let keys = config::PLATFORMS.iter().map(|p| (p.to_string(), config::get_key(p).is_some())).collect();
    ConfigView {
        cfg,
        keys,
        hotkeys: HOTKEYS.iter().map(|(_, a, k)| (a.to_string(), k.to_string())).collect(),
        hotkeys_failed: failed.0.clone(),
        twitch_login: twitch::connected_login(),
        voice_on: state.voice.lock().unwrap().is_some(),
    }
}

#[derive(Deserialize)]
struct SettingsIn {
    targets: Vec<String>,
    kick_url: String,
    twitch_client_id: String,
    twitch_channel: String,
    kick_channel: String,
    lang: String,
    chat: bool,
    alerts_on_stream: bool,
    /// Claves nuevas por plataforma. None = no tocar; "" = borrar.
    #[serde(default)]
    keys: std::collections::HashMap<String, Option<String>>,
}

#[tauri::command]
fn save_config(app: AppHandle, settings: SettingsIn) -> Result<String, String> {
    let state = app.state::<AppState>();
    for (p, k) in &settings.keys {
        let id = config::platform_id(p).ok_or("Plataforma desconocida")?;
        if let Some(k) = k {
            config::set_key(id, k.trim())?;
        }
    }
    let lang_changed;
    {
        let mut cfg = state.cfg.lock().unwrap();
        let mut targets: Vec<String> =
            settings.targets.iter().filter_map(|t| config::platform_id(t)).map(String::from).collect();
        targets.dedup();
        cfg.targets = targets;
        cfg.kick_url = settings.kick_url.trim().into();
        cfg.twitch_client_id = settings.twitch_client_id.trim().into();
        cfg.twitch_channel = settings.twitch_channel.trim().trim_start_matches('#').to_lowercase();
        cfg.kick_channel = settings.kick_channel.trim().to_lowercase();
        let lang = if settings.lang == "en" { "en" } else { "es" };
        lang_changed = cfg.lang != lang;
        cfg.lang = lang.into();
        cfg.chat = settings.chat;
        cfg.alerts_on_stream = settings.alerts_on_stream;
        config::save(&cfg)?;
    }
    restart_chat(&app);
    // Si cambió el idioma y la voz estaba prendida, se reinicia con las frases del idioma nuevo
    if lang_changed && state.voice.lock().unwrap().is_some() {
        set_voice(&app, false)?;
        set_voice(&app, true)?;
    }
    Ok("Ajustes guardados".into())
}

#[tauri::command]
fn chat_history(state: tauri::State<'_, AppState>) -> Vec<chat::ChatEvent> {
    state.chat_log.lock().unwrap().iter().cloned().collect()
}

/// Conectar Twitch: devuelve el código para escribir en twitch.tv/activate y espera en segundo plano.
#[tauri::command]
async fn twitch_connect(app: AppHandle) -> Result<twitch::DeviceStart, String> {
    let client_id = app.state::<AppState>().cfg.lock().unwrap().twitch_client_id.clone();
    let start = tauri::async_runtime::spawn_blocking({
        let id = client_id.clone();
        move || twitch::device_start(&id)
    })
    .await
    .map_err(|e| e.to_string())??;
    open_url(&start.verification_uri);
    let s2 = start.clone();
    std::thread::spawn(move || {
        let r = twitch::device_wait(&client_id, &s2).and_then(|login| {
            twitch::fetch_stream_key(&client_id)?;
            Ok(login)
        });
        match r {
            Ok(login) => {
                {
                    let st = app.state::<AppState>();
                    let mut cfg = st.cfg.lock().unwrap();
                    cfg.twitch_channel = login.clone();
                    if !cfg.targets.iter().any(|t| t == "twitch") {
                        cfg.targets.push("twitch".into());
                    }
                    let _ = config::save(&cfg);
                }
                restart_chat(&app);
                let _ = app.emit("twitch", serde_json::json!({ "ok": true, "login": login }));
                toast(&app, true, &format!("Twitch conectado: {login}. La clave se cargó sola."));
            }
            Err(e) => {
                let _ = app.emit("twitch", serde_json::json!({ "ok": false }));
                toast(&app, false, &e);
            }
        }
    });
    Ok(start)
}

#[tauri::command]
fn twitch_disconnect() -> Result<String, String> {
    twitch::disconnect()?;
    Ok("Cuenta de Twitch desconectada".into())
}

#[tauri::command]
async fn twitch_update(app: AppHandle, title: String, category: String) -> Result<String, String> {
    let id = app.state::<AppState>().cfg.lock().unwrap().twitch_client_id.clone();
    tauri::async_runtime::spawn_blocking(move || twitch::update_channel(&id, &title, &category))
        .await
        .map_err(|e| e.to_string())?
}

/// Abre solo URLs conocidas en el navegador del usuario
fn open_url(url: &str) {
    const ALLOWED: [&str; 4] =
        ["https://www.twitch.tv/", "https://id.twitch.tv/", "https://dev.twitch.tv/", "https://kick.com/"];
    if ALLOWED.iter().any(|a| url.starts_with(a)) {
        let _ = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn();
    }
}

#[tauri::command]
fn open_link(url: String) {
    open_url(&url);
}

#[tauri::command]
fn open_folder(state: tauri::State<'_, AppState>, which: String) -> Result<(), String> {
    let s = state.engine.status();
    let dir = if which == "rec" { s.rec_dir } else { s.clips_dir };
    std::process::Command::new("explorer").arg(dir).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Modo Stream: se cierra la UI (libera WebView2) y Chispa queda solo en la bandeja.
#[tauri::command]
fn stream_mode(app: AppHandle) {
    dlog("stream_mode (botón)");
    close_window(&app);
}

struct HotkeysFailed(Vec<String>);

fn main() {
    tauri::Builder::default()
        // Una sola instancia: si se abre otra vez, se muestra la ventana existente en vez de arrancar otro motor
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_window(app)))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    for (code, action, _) in HOTKEYS {
                        if shortcut.matches(Modifiers::ALT, code) {
                            action_with_toast(app.clone(), action);
                        }
                    }
                })
                .build(),
        )
        .setup(|app| {
            // El motor arranca acá, después del chequeo de instancia única
            let engine = EngineHandle::spawn(EngineOptions::default_dirs());
            let cfg = config::load();
            let voice_on = cfg.voice;
            app.manage(AppState {
                engine,
                cfg: Mutex::new(cfg),
                chat: Mutex::new(None),
                chat_log: Mutex::new(VecDeque::new()),
                voice: Mutex::new(None),
            });
            restart_chat(app.handle());
            if voice_on {
                if let Err(e) = set_voice(app.handle(), true) {
                    eprintln!("[chispa] {e}");
                }
            }

            // Atajos: si alguno ya lo usa otra app, se avisa en la UI en vez de romper
            let mut failed = Vec::new();
            for (code, _, label) in HOTKEYS {
                if app.global_shortcut().register(Shortcut::new(Some(Modifiers::ALT), code)).is_err() {
                    failed.push(label.to_string());
                }
            }
            app.manage(HotkeysFailed(failed));

            // Bandeja del sistema
            let show = MenuItem::with_id(app, "show", "Mostrar Chispa", true, None::<&str>)?;
            let clip = MenuItem::with_id(app, "clip", "Clipear (Alt+F10)", true, None::<&str>)?;
            let brb = MenuItem::with_id(app, "brb", "Escena: Ya vuelvo", true, None::<&str>)?;
            let game = MenuItem::with_id(app, "game", "Escena: Juego", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Salir", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &clip, &game, &brb, &quit])?;
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().cloned().expect("falta el ícono"))
                .tooltip("Chispa")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, ev| match ev.id.as_ref() {
                    "show" => show_window(app),
                    "clip" => action_with_toast(app.clone(), "clip"),
                    "brb" => action_with_toast(app.clone(), "scene:pausa"),
                    "game" => action_with_toast(app.clone(), "scene:juego"),
                    "quit" => {
                        let st = app.state::<AppState>();
                        *st.voice.lock().unwrap() = None;
                        *st.chat.lock().unwrap() = None;
                        let _ = st.engine.send(Command::Shutdown);
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, ev| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                        show_window(tray.app_handle());
                    }
                })
                .build(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            status,
            action,
            get_config,
            save_config,
            chat_history,
            twitch_connect,
            twitch_disconnect,
            twitch_update,
            open_link,
            open_folder,
            stream_mode,
            preview
        ])
        .on_window_event(|window, event| {
            dlog(&format!("window event: {event:?}"));
            // Cerrar con la X: primero se saca la vista previa de libobs y después se destruye la ventana
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                close_window(window.app_handle());
            }
        })
        .build(tauri::generate_context!())
        .expect("error al iniciar Chispa")
        .run(|_app, event| {
            // Cerrar la ventana no corta nada: Chispa sigue en la bandeja. Solo "Salir" (exit con código) termina.
            if let RunEvent::ExitRequested { api, code: None, .. } = event {
                api.prevent_exit();
            }
        });
}
