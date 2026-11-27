//! Motor: un contexto libobs con una escena (pantalla, audio del escritorio y micrófono)
//! y tres salidas: replay buffer (clips), grabación y stream RTMP.
//! Vive en un solo hilo (ver `bus.rs`); nadie más lo toca directamente.

use std::{
    path::PathBuf,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context, Result};
use libobs_simple::{
    output::{
        replay::ObsContextReplayExt,
        simple::{HardwareCodec, HardwarePreset, ObsContextSimpleExt},
    },
    sources::windows::{monitor_capture::MonitorCaptureSource, MonitorCaptureSourceBuilder},
    wrapper::{
        context::ObsContext,
        data::{
            output::{ObsOutputRef, ObsOutputTrait, ObsReplayBufferOutputRef},
            video::ObsVideoInfoBuilder,
            ObsDataSetters,
        },
        enums::{ObsAlignment, ObsBoundsType, ObsScaleType},
        graphics::Vec2,
        scenes::{ObsSceneItemRef, ObsSceneRef, ObsTransformInfoBuilder, SceneItemExtSceneTrait, SceneItemTrait},
        sources::{ObsSourceAudioExt, ObsSourceBuilder, ObsSourceRef},
        utils::{ObsPath, SourceInfo, StartupInfo},
    },
};

use libobs_simple::wrapper::data::{
    object::ObsObjectTrait,
    properties::{types::ObsListItemValue, ObsProperty, ObsPropertyObject},
};

use libobs_simple::wrapper::display::{ObsDisplayCreationData, ObsDisplayRef, ObsWindowHandle, WindowPositionTrait};

use crate::{
    audio::{self, MicChoice},
    live::{DestHealth, Live},
    profile::{self, Machine, Profile},
};

#[derive(Clone, Debug)]
pub struct StreamTarget {
    /// Nombre visible: "Twitch", "YouTube", "Kick"
    pub name: String,
    pub url: String,
    pub key: String,
}

#[derive(Clone, Debug)]
pub struct EngineOptions {
    pub clips_dir: PathBuf,
    pub rec_dir: PathBuf,
    pub replay_seconds: i64,
    pub replay_on_start: bool,
    pub capture_audio: bool,
}

impl EngineOptions {
    /// Carpetas por defecto: %USERPROFILE%\Videos\Chispa\{Clips,Grabaciones}
    pub fn default_dirs() -> Self {
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        let base = home.join("Videos").join("Chispa");
        Self {
            clips_dir: base.join("Clips"),
            rec_dir: base.join("Grabaciones"),
            replay_seconds: 30,
            replay_on_start: true,
            capture_audio: true,
        }
    }
}

/// Escenas listas para usar. Es una sola escena de libobs; cambia qué se ve y dónde.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SceneMode {
    /// Pantalla/juego, con la cámara chica en una esquina (si está prendida)
    Juego,
    /// Cartel "Ya vuelvo": no se ve la pantalla
    Pausa,
    /// Cámara a pantalla completa
    Charla,
}

impl SceneMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "juego" | "jugando" | "pantalla" | "game" => Some(Self::Juego),
            "pausa" | "ya vuelvo" | "yavuelvo" | "brb" => Some(Self::Pausa),
            "charla" | "charlando" | "camara" | "cámara" | "chat" => Some(Self::Charla),
            _ => None,
        }
    }
}

pub struct Engine {
    ctx: ObsContext,
    scene: ObsSceneRef,
    base: (u32, u32),
    screen: ObsSceneItemRef<MonitorCaptureSource>,
    pause_bg: Option<ObsSceneItemRef<ObsSourceRef>>,
    pause_text: Option<ObsSceneItemRef<ObsSourceRef>>,
    camera: Option<ObsSceneItemRef<ObsSourceRef>>,
    pub camera_name: Option<String>,
    cam_wanted: bool,
    alert: Option<ObsSceneItemRef<ObsSourceRef>>,
    alert_until: Option<Instant>,
    preview: Option<ObsDisplayRef>,
    pub mode: SceneMode,
    _desktop_audio: Option<ObsSceneItemRef<ObsSourceRef>>,
    mic: Option<ObsSceneItemRef<ObsSourceRef>>,
    pub mic_name: Option<String>,
    /// Aviso de autoconfiguración del micrófono, para mostrar una vez
    pub mic_note: Option<String>,
    pub machine: Machine,
    pub profile: Profile,
    pub out_size: (u32, u32),
    pub gpu_encoder: bool,
    pub opts: EngineOptions,
    replay: ObsReplayBufferOutputRef,
    replay_active: bool,
    record: Option<(ObsOutputRef, Instant, PathBuf)>,
    live: Option<Live>,
    cpu: CpuWatch,
}

impl Engine {
    pub fn new(opts: EngineOptions) -> Result<Self> {
        let machine = profile::detect_machine();
        let prof = profile::pick_profile(&machine);
        let monitors = MonitorCaptureSourceBuilder::get_monitors().context("no pude listar monitores")?;
        let monitor = monitors
            .iter()
            .find(|m| m.is_primary)
            .or(monitors.first())
            .context("no hay monitores")?
            .clone();
        let out_size = profile::output_size(monitor.width, monitor.height, prof.out_height);

        let scale = if prof.tier == profile::Tier::Potato { ObsScaleType::Bilinear } else { ObsScaleType::Bicubic };
        let video = ObsVideoInfoBuilder::new()
            .base_width(monitor.width)
            .base_height(monitor.height)
            .output_width(out_size.0)
            .output_height(out_size.1)
            .fps_num(prof.fps)
            .fps_den(1)
            .scale_type(scale)
            .build();
        let mut ctx = StartupInfo::new().set_video_info(video).start().context("libobs no arrancó")?;

        let mut scene = ctx.scene("main", Some(0))?;
        let screen = ctx
            .source_builder::<MonitorCaptureSourceBuilder, _>("Pantalla")?
            .set_monitor(&monitor)
            .add_to_scene(&mut scene)?;

        // Audio: dispositivos por defecto de Windows. Si falla alguno, seguimos sin él.
        let (desktop_audio, mut mic) = if opts.capture_audio {
            (
                add_audio(&ctx, &mut scene, "wasapi_output_capture", "Audio del escritorio"),
                add_audio(&ctx, &mut scene, "wasapi_input_capture", "Micrófono"),
            )
        } else {
            (None, None)
        };
        let (mic_name, mic_note) = match &mic {
            Some(m) => match setup_mic(&ctx, m.inner_source()) {
                Ok(MicChoice::Default(n)) => (Some(n), None),
                Ok(MicChoice::Switched { from, to, .. }) => {
                    let note = format!("Tu micrófono por defecto era \"{from}\" (copia el audio de la PC), así que usé \"{to}\"");
                    (Some(to), Some(note))
                }
                Ok(MicChoice::None) => {
                    mic = None; // se descarta la fuente para no duplicar el audio del escritorio
                    (None, Some("No encontré un micrófono real: solo se graba el audio de la PC".into()))
                }
                Err(e) => {
                    eprintln!("[chispa] no pude revisar el micrófono: {e}");
                    (None, None)
                }
            },
            None => (None, None),
        };

        std::fs::create_dir_all(&opts.clips_dir)?;
        std::fs::create_dir_all(&opts.rec_dir)?;

        // Replay buffer: se intenta primero el encoder por hardware y, si no hay, x264.
        let rb = |c: &ObsContext| {
            c.replay_buffer_builder("chispa-replay", ObsPath::new(&opts.clips_dir.to_string_lossy()))
                .max_time_sec(opts.replay_seconds)
                .video_bitrate(prof.video_kbps)
                .audio_bitrate(prof.audio_kbps)
        };
        let (replay, gpu_encoder) = match rb(&ctx).hardware_encoder(HardwareCodec::H264, HardwarePreset::Speed).build() {
            Ok(r) => (r, true),
            Err(e) => {
                eprintln!("[chispa] sin encoder por hardware ({e}), uso x264");
                (rb(&ctx).build()?, false)
            }
        };

        let mut engine = Self {
            ctx,
            scene,
            base: (monitor.width, monitor.height),
            screen,
            pause_bg: None,
            pause_text: None,
            camera: None,
            camera_name: None,
            cam_wanted: false,
            alert: None,
            alert_until: None,
            preview: None,
            mode: SceneMode::Juego,
            _desktop_audio: desktop_audio,
            mic,
            mic_name,
            mic_note,
            machine,
            profile: prof,
            out_size,
            gpu_encoder,
            opts,
            replay,
            replay_active: false,
            record: None,
            live: None,
            cpu: CpuWatch::new(),
        };
        if engine.opts.replay_on_start {
            engine.set_replay(true)?;
        }
        Ok(engine)
    }

    pub fn has_mic(&self) -> bool {
        self.mic.is_some()
    }

    pub fn replay_active(&self) -> bool {
        self.replay_active
    }

    pub fn set_replay(&mut self, on: bool) -> Result<()> {
        if on == self.replay_active {
            return Ok(());
        }
        if on {
            self.replay.start()?;
        } else {
            self.replay.stop()?;
        }
        self.replay_active = on;
        Ok(())
    }

    pub fn save_clip(&mut self) -> Result<PathBuf> {
        if !self.replay_active {
            return Err(anyhow!("El replay buffer está apagado: activalo para poder clipear"));
        }
        Ok(self.replay.save_buffer()?.to_path_buf())
    }

    pub fn recording(&self) -> Option<(u64, &PathBuf)> {
        self.record.as_ref().map(|(_, t, p)| (t.elapsed().as_secs(), p))
    }

    pub fn start_record(&mut self) -> Result<PathBuf> {
        if let Some((_, _, p)) = &self.record {
            return Ok(p.clone());
        }
        // Mismo formato que los clips: "2026-11-27 15-05-22.mp4"
        let mut path = self.opts.rec_dir.join(format!("{}.mp4", local_stamp()));
        if path.exists() {
            path = self.opts.rec_dir.join(format!("{} ({}).mp4", local_stamp(), unix_now() % 1000));
        }
        let out = self
            .ctx
            .simple_output_builder("chispa-rec", ObsPath::new(&path.to_string_lossy()))
            .prefer_hardware_encoder(HardwareCodec::H264)
            .video_bitrate(self.profile.video_kbps)
            .audio_bitrate(self.profile.audio_kbps)
            .build()?;
        out.start()?;
        self.record = Some((out, Instant::now(), path.clone()));
        Ok(path)
    }

    pub fn stop_record(&mut self) -> Result<Option<PathBuf>> {
        match self.record.take() {
            Some((out, _, path)) => {
                out.stop()?;
                Ok(Some(path))
            }
            None => Ok(None),
        }
    }

    /// (segundos en vivo, destinos)
    pub fn streaming(&self) -> Option<(u64, Vec<String>)> {
        self.live.as_ref().map(|l| (l.started.elapsed().as_secs(), l.names()))
    }

    pub fn stream_health(&self) -> Vec<DestHealth> {
        self.live.as_ref().map(|l| l.health()).unwrap_or_default()
    }

    /// Sale en vivo a todos los destinos con un solo encode. Devuelve avisos de destinos que fallaron.
    pub fn start_stream(&mut self, targets: &[StreamTarget]) -> Result<Vec<String>> {
        if self.live.is_some() {
            return Err(anyhow!("Ya estás en vivo"));
        }
        let (live, errors) = Live::start(&self.ctx, targets, self.profile.video_kbps, self.profile.audio_kbps)?;
        self.live = Some(live);
        Ok(errors)
    }

    pub fn stop_stream(&mut self) -> Result<bool> {
        match self.live.take() {
            Some(l) => {
                l.stop()?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Corta un solo destino y deja los demás en vivo.
    pub fn stop_stream_one(&mut self, name: &str) -> Result<()> {
        let empty = match self.live.as_mut() {
            Some(l) => l.stop_one(name)?,
            None => return Err(anyhow!("No estás en vivo")),
        };
        if empty {
            self.live = None;
        }
        Ok(())
    }

    /// Se llama periódicamente (guardián). Devuelve avisos: destinos caídos, red que no alcanza, CPU al límite.
    pub fn poll(&mut self) -> Vec<String> {
        self.tick_alert();
        let mut notes = Vec::new();
        if let Some(l) = self.live.as_mut() {
            notes.extend(l.poll());
            if l.is_empty() {
                self.live = None;
            }
        }
        if self.live.is_some() || self.record.is_some() {
            if let Some(n) = self.cpu.check() {
                notes.push(n);
            }
        }
        notes
    }

    // ---------- Escenas y cámara ----------

    pub fn camera_on(&self) -> bool {
        self.cam_wanted && self.camera.is_some()
    }

    /// Prende o apaga la cámara. La primera vez la busca y la crea (si no, no gasta nada).
    pub fn set_camera(&mut self, on: bool) -> Result<bool> {
        if on && self.camera.is_none() {
            let item = self.add_camera()?;
            self.camera = Some(item);
        }
        self.cam_wanted = on;
        self.apply_layout()?;
        Ok(on)
    }

    fn add_camera(&mut self) -> Result<ObsSceneItemRef<ObsSourceRef>> {
        let item = self
            .scene
            .add_and_create_source(SourceInfo::new("dshow_input", "Cámara", None, None))
            .context("no pude crear la fuente de cámara")?;
        let props = item.inner_source().get_properties()?;
        let cams: Vec<(String, String)> = match props.get("video_device_id") {
            Some(ObsProperty::List(list)) => list
                .items()
                .iter()
                .filter(|i| !*i.disabled())
                .filter_map(|i| match i.value() {
                    ObsListItemValue::String(id) if !id.is_empty() => Some((i.name().clone(), id.clone())),
                    _ => None,
                })
                // Las cámaras virtuales (OBS, Streamlabs) no son una cámara real
                .filter(|(n, _)| !n.to_lowercase().contains("virtual"))
                .collect(),
            _ => vec![],
        };
        let Some((name, id)) = cams.into_iter().next() else {
            let _ = self.scene.remove_item(&item);
            return Err(anyhow!("No encontré ninguna cámara conectada"));
        };
        let mut settings = self.ctx.data()?;
        settings.set_string("video_device_id", id.as_str())?;
        // En PCs flojas se pide una resolución chica a la cámara: decodificarla cuesta CPU
        if self.profile.tier == profile::Tier::Potato {
            settings.set_int("res_type", 1)?;
            settings.set_string("resolution", "640x360")?;
        }
        item.inner_source().update_settings(settings)?;
        self.camera_name = Some(name);
        Ok(item)
    }

    pub fn set_scene(&mut self, mode: SceneMode) -> Result<SceneMode> {
        if mode == SceneMode::Charla && !self.camera_on() {
            self.set_camera(true).context("La escena Charla necesita la cámara")?;
        }
        if mode == SceneMode::Pausa && self.pause_bg.is_none() {
            self.add_pause_card()?;
        }
        self.mode = mode;
        self.apply_layout()?;
        Ok(mode)
    }

    fn add_pause_card(&mut self) -> Result<()> {
        let (w, h) = self.base;
        let mut bg = self.ctx.data()?;
        bg.set_int("color", 0xFF13_1111u32 as i64)?; // ABGR: gris casi negro
        bg.set_int("width", w as i64)?;
        bg.set_int("height", h as i64)?;
        let bg = self.scene.add_and_create_source(SourceInfo::new("color_source_v3", "Pausa fondo", Some(bg), None))?;
        let mut tx = self.ctx.data()?;
        tx.set_string("text", "Ya vuelvo ⚡")?;
        tx.set_int("color", 0xFF1A_7AFFu32 as i64)?; // naranja Chispa (ABGR)
        let text = self.scene.add_and_create_source(SourceInfo::new("text_gdiplus", "Pausa texto", Some(tx), None))?;
        let info = ObsTransformInfoBuilder::new()
            .set_pos(Vec2::new(w as f32 / 2.0, h as f32 / 2.0))
            .set_alignment(ObsAlignment::CENTER)
            .set_bounds(Vec2::new(w as f32 * 0.6, h as f32 * 0.25))
            .set_bounds_type(ObsBoundsType::ScaleInner)
            .set_bounds_alignment(ObsAlignment::CENTER)
            .build(w, h);
        text.set_transform_info(&info)?;
        self.pause_bg = Some(bg);
        self.pause_text = Some(text);
        Ok(())
    }

    /// Ubica cada cosa según la escena actual
    fn apply_layout(&self) -> Result<()> {
        let (w, h) = (self.base.0 as f32, self.base.1 as f32);
        let pause = self.mode == SceneMode::Pausa;
        self.screen.set_visible(self.mode == SceneMode::Juego)?;
        for p in [&self.pause_bg, &self.pause_text].into_iter().flatten() {
            p.set_visible(pause)?;
        }
        if let Some(cam) = &self.camera {
            let show = self.cam_wanted && !pause;
            cam.set_visible(show)?;
            if !show {
                return Ok(());
            }
            let (pos, size) = match self.mode {
                SceneMode::Charla => ((0.0, 0.0), (w, h)),
                _ => {
                    let (bw, bh) = (w * 0.24, h * 0.24);
                    let m = h * 0.025;
                    ((w - bw - m, h - bh - m), (bw, bh))
                }
            };
            let info = ObsTransformInfoBuilder::new()
                .set_pos(Vec2::new(pos.0, pos.1))
                .set_alignment(ObsAlignment::LEFT | ObsAlignment::TOP)
                .set_bounds(Vec2::new(size.0, size.1))
                .set_bounds_type(ObsBoundsType::ScaleInner)
                .set_bounds_alignment(ObsAlignment::CENTER)
                .build(self.base.0, self.base.1);
            cam.set_transform_info(&info)?;
        }
        Ok(())
    }

    /// Muestra una alerta (sub, raid, bits…) arriba al centro del stream durante unos segundos.
    pub fn show_alert(&mut self, text: &str) -> Result<()> {
        let (w, h) = self.base;
        let mut tx = self.ctx.data()?;
        tx.set_string("text", text)?;
        tx.set_int("color", 0xFFFF_FFFFu32 as i64)?;
        tx.set_bool("outline", true)?;
        tx.set_int("outline_size", 4)?;
        tx.set_int("outline_color", 0xFF1A_7AFFu32 as i64)?;
        match &self.alert {
            Some(item) => item.inner_source().update_settings(tx)?,
            None => {
                let item = self.scene.add_and_create_source(SourceInfo::new("text_gdiplus", "Alerta", Some(tx), None))?;
                let info = ObsTransformInfoBuilder::new()
                    .set_pos(Vec2::new(w as f32 / 2.0, h as f32 * 0.06))
                    .set_alignment(ObsAlignment::CENTER | ObsAlignment::TOP)
                    .set_bounds(Vec2::new(w as f32 * 0.8, h as f32 * 0.09))
                    .set_bounds_type(ObsBoundsType::ScaleInner)
                    .set_bounds_alignment(ObsAlignment::CENTER)
                    .build(w, h);
                item.set_transform_info(&info)?;
                self.alert = Some(item);
            }
        }
        if let Some(item) = &self.alert {
            item.set_visible(true)?;
        }
        self.alert_until = Some(Instant::now() + std::time::Duration::from_secs(6));
        Ok(())
    }

    fn tick_alert(&mut self) {
        if self.alert_until.is_some_and(|t| Instant::now() >= t) {
            self.alert_until = None;
            if let Some(item) = &self.alert {
                let _ = item.set_visible(false);
            }
        }
    }

    // ---------- Vista previa ----------

    /// Muestra la salida (lo que ve el stream) dentro de la ventana `hwnd`, en el rectángulo dado
    /// (px físicos, relativo al área cliente). libobs la dibuja por GPU en una ventana hija.
    pub fn set_preview(&mut self, hwnd: isize, x: i32, y: i32, w: u32, h: u32) -> Result<()> {
        if let Some(d) = &self.preview {
            d.set_pos(x, y)?;
            d.set_size(w.max(2), h.max(2))?;
            return Ok(());
        }
        // Safety: el HWND es la ventana principal de la app; la app cierra la vista previa
        // (close_preview) antes de destruir esa ventana.
        let handle = unsafe { ObsWindowHandle::new_from_handle(hwnd as *mut std::ffi::c_void) };
        let data = ObsDisplayCreationData::new(handle, x, y, w.max(2), h.max(2)).set_background_color(0x0013_1111);
        let d = self.ctx.display(data).context("no pude crear la vista previa")?;
        self.preview = Some(d);
        Ok(())
    }

    pub fn close_preview(&mut self) {
        if let Some(d) = self.preview.take() {
            let _ = self.ctx.remove_display(&d);
        }
    }

    pub fn preview_on(&self) -> bool {
        self.preview.is_some()
    }

    pub fn set_mic_muted(&mut self, muted: bool) -> Result<bool> {
        let mic = self.mic.as_ref().ok_or_else(|| anyhow!("No hay micrófono"))?;
        mic.inner_source().set_muted(muted)?;
        Ok(muted)
    }

    pub fn mic_muted(&self) -> bool {
        self.mic.as_ref().and_then(|m| m.inner_source().muted().ok()).unwrap_or(false)
    }

    pub fn shutdown(&mut self) {
        self.close_preview();
        let _ = self.stop_stream();
        let _ = self.stop_record();
        let _ = self.set_replay(false);
    }
}

fn add_audio(ctx: &ObsContext, scene: &mut ObsSceneRef, id: &str, name: &str) -> Option<ObsSceneItemRef<ObsSourceRef>> {
    let mut build = || -> Result<ObsSceneItemRef<ObsSourceRef>> {
        let mut settings = ctx.data()?;
        settings.set_string("device_id", "default")?;
        Ok(scene.add_and_create_source(SourceInfo::new(id, name, Some(settings), None))?)
    };
    match build() {
        Ok(item) => Some(item),
        Err(e) => {
            eprintln!("[chispa] no pude agregar {name}: {e}");
            None
        }
    }
}

/// Lee la lista de dispositivos de entrada que ve OBS y, si el default de Windows es un loopback, cambia a un mic real.
fn setup_mic(ctx: &ObsContext, src: &ObsSourceRef) -> Result<MicChoice> {
    let props = src.get_properties()?;
    let devices: Vec<(String, String)> = match props.get("device_id") {
        Some(ObsProperty::List(list)) => list
            .items()
            .iter()
            .filter(|i| !*i.disabled())
            .filter_map(|i| match i.value() {
                ObsListItemValue::String(id) if id != "default" => Some((i.name().clone(), id.clone())),
                _ => None,
            })
            .collect(),
        _ => return Err(anyhow!("la fuente de micrófono no tiene lista de dispositivos")),
    };
    let default_id = audio::default_capture_id();
    let choice = audio::choose_mic(&devices, default_id.as_deref());
    if std::env::var_os("CHISPA_DEBUG").is_some() {
        eprintln!("[chispa] mic default={default_id:?} devices={devices:?} -> {choice:?}");
    }
    // Siempre se fija el ID explícito: el "default" de OBS puede resolver a otro dispositivo
    // (en esta PC abría "Mezcla estéreo" aunque el default de Windows era el mic real).
    let id = match &choice {
        MicChoice::Switched { id, .. } => Some(id.clone()),
        MicChoice::Default(name) => devices.iter().find(|(n, _)| n == name).map(|(_, id)| id.clone()),
        MicChoice::None => None,
    };
    if let Some(id) = id {
        let mut settings = ctx.data()?;
        settings.set_string("device_id", id.as_str())?;
        src.update_settings(settings)?;
    }
    Ok(choice)
}

/// Vigila la CPU de toda la PC mientras se graba o transmite.
struct CpuWatch {
    sys: sysinfo::System,
    last_check: Instant,
    high_since: Option<Instant>,
    warned_at: Option<Instant>,
}

impl CpuWatch {
    fn new() -> Self {
        Self { sys: sysinfo::System::new(), last_check: Instant::now(), high_since: None, warned_at: None }
    }

    /// Avisa si la CPU pasa el 90 % durante 10 s seguidos (como mucho una vez por minuto).
    fn check(&mut self) -> Option<String> {
        if self.last_check.elapsed() < std::time::Duration::from_secs(2) {
            return None;
        }
        self.last_check = Instant::now();
        self.sys.refresh_cpu_usage();
        let usage = self.sys.global_cpu_usage();
        if usage < 90.0 {
            self.high_since = None;
            return None;
        }
        let since = *self.high_since.get_or_insert_with(Instant::now);
        let quiet = self.warned_at.is_none_or(|t| t.elapsed().as_secs() > 60);
        if since.elapsed().as_secs() >= 10 && quiet {
            self.warned_at = Some(Instant::now());
            return Some(format!(
                "La PC está al {usage:.0} % de CPU: puede trabarse el juego o el stream. Cerrá programas que no uses."
            ));
        }
        None
    }
}

/// Fecha y hora local para nombres de archivo: "2026-11-27 15-05-22"
fn local_stamp() -> String {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:04}-{:02}-{:02} {:02}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
