//! chispa-proto: consola de prueba del motor, sin UI.
//!   c = clip | v = clip vertical | r = grabar/parar | m = mutear mic | p = replay on/off
//!   k = cámara | e1/e2/e3 = escena juego/pausa/charla | a = alerta de prueba | s = estado | q = salir
//!   --seconds N: sale solo después de N segundos y guarda un clip final

use std::{
    io::BufRead,
    sync::mpsc,
    time::{Duration, Instant},
};

use chispa_core::{Command, EngineHandle, EngineOptions};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let seconds: Option<u64> = args
        .iter()
        .position(|a| a == "--seconds")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok());

    let engine = EngineHandle::spawn(EngineOptions::default_dirs());

    // Esperar a que cargue el motor
    let t0 = Instant::now();
    loop {
        let s = engine.status();
        if let Some(e) = s.error {
            eprintln!("[chispa] {e}");
            std::process::exit(1);
        }
        if s.ready {
            println!(
                "[chispa] Listo en {:.1}s · {} hilos, {} GB · perfil {} {} @{} · {} kbps · encoder {} · mic: {}",
                t0.elapsed().as_secs_f32(),
                s.threads,
                s.ram_gb,
                s.tier,
                s.resolution,
                s.fps,
                s.video_kbps,
                s.encoder,
                if s.has_mic { "sí" } else { "no" }
            );
            println!("[chispa] Clips: {} | Grabaciones: {}", s.clips_dir, s.rec_dir);
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if tx.send(line.trim().to_lowercase()).is_err() {
                break;
            }
        }
    });

    let start = Instant::now();
    loop {
        let cmd = match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(c) => Some(c),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            // stdin cerrado: pausar para no girar en vacío
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                std::thread::sleep(Duration::from_millis(200));
                None
            }
        };
        let to_send = match cmd.as_deref() {
            Some("c") => Some(Command::Clip),
            Some("r") => Some(Command::ToggleRecord),
            Some("m") => Some(Command::ToggleMic),
            Some("p") => Some(Command::ToggleReplay),
            Some("v") => Some(Command::ClipVertical),
            Some("k") => Some(Command::ToggleCamera),
            Some("e1") => Some(Command::SetScene(chispa_core::SceneMode::Juego)),
            Some("e2") => Some(Command::SetScene(chispa_core::SceneMode::Pausa)),
            Some("e3") => Some(Command::SetScene(chispa_core::SceneMode::Charla)),
            Some("a") => Some(Command::ShowAlert("Prueba: Pepe se suscribió".into())),
            Some("s") => {
                println!("[chispa] {:?}", engine.status());
                None
            }
            Some("q") => break,
            _ => None,
        };
        if let Some(c) = to_send {
            match engine.send(c) {
                Ok(m) => println!("[chispa] {m}"),
                Err(e) => println!("[chispa] Error: {e}"),
            }
        }
        if let Some(n) = engine.take_notice() {
            println!("[chispa] Aviso: {n}");
        }
        if seconds.is_some_and(|s| start.elapsed() >= Duration::from_secs(s)) {
            match engine.send(Command::Clip) {
                Ok(m) => println!("[chispa] {m}: {}", engine.status().last_clip.unwrap_or_default()),
                Err(e) => println!("[chispa] Error al clipear: {e}"),
            }
            break;
        }
    }
    let _ = engine.send(Command::Shutdown);
}
