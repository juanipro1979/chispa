//! Bus de comandos. El motor vive en un hilo propio; UI, atajos y voz le mandan `Command`s.
//! El estado se publica en un `Status` compartido, así la UI lo lee sin esperar en la cola.

use std::{
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};

use serde::Serialize;

use crate::engine::{Engine, EngineOptions, SceneMode, StreamTarget};

#[derive(Debug, Clone)]
pub enum Command {
    Clip,
    /// Guarda un clip y además arma la versión vertical 9:16 (en segundo plano)
    ClipVertical,
    SetScene(SceneMode),
    /// Muestra una alerta en el stream (sin respuesta para el usuario)
    ShowAlert(String),
    /// Vista previa: Some((hwnd, x, y, ancho, alto)) la abre o la mueve; None la cierra
    Preview(Option<(isize, i32, i32, u32, u32)>),
    ToggleCamera,
    ToggleRecord,
    /// Sale en vivo a uno o varios destinos (un solo encode)
    StartStream(Vec<StreamTarget>),
    StopStream,
    /// Corta un solo destino (por nombre) y deja el resto
    StopStreamTo(String),
    ToggleMic,
    ToggleReplay,
    Shutdown,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Status {
    pub ready: bool,
    pub error: Option<String>,
    pub tier: String,
    pub resolution: String,
    pub fps: u32,
    pub video_kbps: u32,
    pub encoder: String,
    pub threads: usize,
    pub ram_gb: f64,
    pub replay: bool,
    pub replay_seconds: i64,
    pub recording: bool,
    pub rec_seconds: u64,
    pub streaming: bool,
    /// Destinos en vivo, unidos: "Twitch + Kick"
    pub stream_target: Option<String>,
    pub stream_targets: Vec<String>,
    /// Salud de cada destino (guardián)
    pub stream_health: Vec<crate::live::DestHealth>,
    pub stream_seconds: u64,
    pub scene: Option<SceneMode>,
    pub camera_on: bool,
    pub camera_name: Option<String>,
    pub has_mic: bool,
    pub mic_name: Option<String>,
    pub mic_muted: bool,
    pub last_clip: Option<String>,
    pub clips_dir: String,
    pub rec_dir: String,
    /// Último aviso del motor (por ejemplo "se cortó el stream"). Lo consume la UI.
    pub notice: Option<String>,
}

type Reply = mpsc::Sender<Result<String, String>>;

#[derive(Clone)]
pub struct EngineHandle {
    tx: mpsc::Sender<(Command, Option<Reply>)>,
    status: Arc<Mutex<Status>>,
}

impl EngineHandle {
    /// Arranca el motor en un hilo propio. Vuelve enseguida; `status().ready` pasa a true cuando termina de cargar.
    pub fn spawn(opts: EngineOptions) -> Self {
        let (tx, rx) = mpsc::channel::<(Command, Option<Reply>)>();
        let status = Arc::new(Mutex::new(Status::default()));
        let st = status.clone();

        std::thread::Builder::new()
            .name("chispa-engine".into())
            .spawn(move || {
                let mut engine = match Engine::new(opts) {
                    Ok(e) => e,
                    Err(e) => {
                        st.lock().unwrap().error = Some(format!("No pude arrancar el motor: {e:#}"));
                        // Responder error a todo lo que llegue
                        for (_, reply) in rx {
                            if let Some(r) = reply {
                                let _ = r.send(Err("El motor no arrancó".into()));
                            }
                        }
                        return;
                    }
                };
                let mut last_clip: Option<String> = None;
                let first_notice = engine.mic_note.take();
                publish(&st, &engine, &last_clip, first_notice);

                loop {
                    match rx.recv_timeout(Duration::from_millis(500)) {
                        Ok((Command::Shutdown, reply)) => {
                            engine.shutdown();
                            if let Some(r) = reply {
                                let _ = r.send(Ok("Chau".into()));
                            }
                            break;
                        }
                        Ok((Command::ClipVertical, reply)) => {
                            let res = handle(&mut engine, Command::Clip, &mut last_clip);
                            if let (Ok(_), Some(clip)) = (&res, last_clip.clone()) {
                                let st2 = st.clone();
                                std::thread::spawn(move || {
                                    let msg = match crate::vertical::make_vertical(std::path::Path::new(&clip)) {
                                        Ok(p) => format!("Clip vertical listo: {}", p.file_name().unwrap_or_default().to_string_lossy()),
                                        Err(e) => format!("{e:#}"),
                                    };
                                    st2.lock().unwrap().notice = Some(msg);
                                });
                            }
                            publish(&st, &engine, &last_clip, None);
                            if let Some(r) = reply {
                                let _ = r.send(res.map(|m| format!("{m} · armando la versión vertical…")));
                            }
                        }
                        Ok((cmd, reply)) => {
                            let res = handle(&mut engine, cmd, &mut last_clip);
                            publish(&st, &engine, &last_clip, None);
                            if let Some(r) = reply {
                                let _ = r.send(res);
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            let notes = engine.poll();
                            let notice = (!notes.is_empty()).then(|| notes.join("
"));
                            publish(&st, &engine, &last_clip, notice);
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            engine.shutdown();
                            break;
                        }
                    }
                }
            })
            .expect("no pude crear el hilo del motor");

        Self { tx, status }
    }

    /// Manda un comando y espera la respuesta (texto para mostrar al usuario).
    pub fn send(&self, cmd: Command) -> Result<String, String> {
        let (rtx, rrx) = mpsc::channel();
        self.tx.send((cmd, Some(rtx))).map_err(|_| "El motor no está corriendo".to_string())?;
        rrx.recv_timeout(Duration::from_secs(30))
            .map_err(|_| "El motor tardó demasiado en responder".to_string())?
    }

    /// Manda un comando sin esperar respuesta
    pub fn post(&self, cmd: Command) {
        let _ = self.tx.send((cmd, None));
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    /// Devuelve y borra el último aviso, para mostrarlo una sola vez.
    pub fn take_notice(&self) -> Option<String> {
        self.status.lock().unwrap().notice.take()
    }
}

fn handle(engine: &mut Engine, cmd: Command, last_clip: &mut Option<String>) -> Result<String, String> {
    let r: anyhow::Result<String> = (|| match cmd {
        Command::Clip => {
            let p = engine.save_clip()?;
            let s = p.display().to_string();
            *last_clip = Some(s.clone());
            Ok(format!("Clip guardado ({} s)", engine.opts.replay_seconds))
        }
        Command::ToggleRecord => {
            if engine.recording().is_some() {
                engine.stop_record()?;
                Ok("Grabación guardada".into())
            } else {
                engine.start_record()?;
                Ok("Grabando".into())
            }
        }
        Command::StartStream(targets) => {
            let failed = engine.start_stream(&targets)?;
            let live = engine.streaming().map(|s| s.1.join(" + ")).unwrap_or_default();
            if failed.is_empty() {
                Ok(format!("En vivo en {live}"))
            } else {
                Ok(format!("En vivo en {live}. No pude conectar: {}", failed.join(" · ")))
            }
        }
        Command::StopStreamTo(name) => {
            engine.stop_stream_one(&name)?;
            Ok(format!("Cortado en {name}"))
        }
        Command::StopStream => {
            if engine.stop_stream()? {
                Ok("Transmisión terminada".into())
            } else {
                Ok("No estabas en vivo".into())
            }
        }
        Command::SetScene(m) => {
            let m = engine.set_scene(m)?;
            Ok(match m {
                SceneMode::Juego => "Escena: Juego".into(),
                SceneMode::Pausa => "Escena: Ya vuelvo".into(),
                SceneMode::Charla => "Escena: Charla".into(),
            })
        }
        Command::ToggleCamera => {
            let on = engine.set_camera(!engine.camera_on())?;
            Ok(if on {
                format!("Cámara prendida: {}", engine.camera_name.clone().unwrap_or_default())
            } else {
                "Cámara apagada".into()
            })
        }
        Command::ClipVertical => Ok(String::new()),
        Command::Preview(Some((hwnd, x, y, w, h))) => {
            engine.set_preview(hwnd, x, y, w, h)?;
            Ok(String::new())
        }
        Command::Preview(None) => {
            engine.close_preview();
            Ok(String::new())
        }
        Command::ShowAlert(t) => {
            engine.show_alert(&t)?;
            Ok(String::new())
        }
        Command::ToggleMic => {
            let muted = engine.set_mic_muted(!engine.mic_muted())?;
            Ok(if muted { "Micrófono muteado".into() } else { "Micrófono activo".into() })
        }
        Command::ToggleReplay => {
            let on = !engine.replay_active();
            engine.set_replay(on)?;
            Ok(if on { "Clips activados".into() } else { "Clips desactivados".into() })
        }
        Command::Shutdown => Ok(String::new()),
    })();
    r.map_err(|e| format!("{e:#}"))
}

fn publish(st: &Arc<Mutex<Status>>, e: &Engine, last_clip: &Option<String>, notice: Option<String>) {
    let mut s = st.lock().unwrap();
    let prev_notice = s.notice.take();
    let rec = e.recording();
    let stream = e.streaming();
    *s = Status {
        ready: true,
        error: None,
        tier: format!("{:?}", e.profile.tier),
        resolution: format!("{}x{}", e.out_size.0, e.out_size.1),
        fps: e.profile.fps,
        video_kbps: e.profile.video_kbps,
        encoder: if e.gpu_encoder { "GPU".into() } else { "CPU (x264)".into() },
        threads: e.machine.threads,
        ram_gb: (e.machine.ram_gb * 10.0).round() / 10.0,
        replay: e.replay_active(),
        replay_seconds: e.opts.replay_seconds,
        recording: rec.is_some(),
        rec_seconds: rec.map(|r| r.0).unwrap_or(0),
        streaming: stream.is_some(),
        stream_target: stream.as_ref().map(|s| s.1.join(" + ")),
        stream_seconds: stream.as_ref().map(|s| s.0).unwrap_or(0),
        stream_targets: stream.map(|s| s.1).unwrap_or_default(),
        stream_health: e.stream_health(),
        scene: Some(e.mode),
        camera_on: e.camera_on(),
        camera_name: e.camera_name.clone(),
        has_mic: e.has_mic(),
        mic_name: e.mic_name.clone(),
        mic_muted: e.mic_muted(),
        last_clip: last_clip.clone(),
        clips_dir: e.opts.clips_dir.display().to_string(),
        rec_dir: e.opts.rec_dir.display().to_string(),
        notice: notice.or(prev_notice),
    };
}
