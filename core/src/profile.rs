//! Autoconfig mínima de la Fase 0: mira la PC y elige un perfil.
//! La versión completa (GPU/encoder, cámara, mic, speed test) llega en la Fase 1.

use sysinfo::System;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Potato,
    Normal,
    Alto,
}

#[derive(Debug, Clone)]
pub struct Profile {
    pub tier: Tier,
    pub out_height: u32,
    pub fps: u32,
    pub video_kbps: u32,
    pub audio_kbps: u32,
}

#[derive(Debug, Clone)]
pub struct Machine {
    pub threads: usize,
    pub ram_gb: f64,
}

pub fn detect_machine() -> Machine {
    let mut sys = System::new();
    sys.refresh_memory();
    Machine {
        threads: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4),
        ram_gb: sys.total_memory() as f64 / 1024f64.powi(3),
    }
}

pub fn pick_profile(m: &Machine) -> Profile {
    let tier = if m.threads <= 4 || m.ram_gb <= 8.5 {
        Tier::Potato
    } else if m.threads <= 8 || m.ram_gb <= 16.5 {
        Tier::Normal
    } else {
        Tier::Alto
    };
    match tier {
        Tier::Potato => Profile { tier, out_height: 720, fps: 30, video_kbps: 3500, audio_kbps: 128 },
        Tier::Normal => Profile { tier, out_height: 720, fps: 60, video_kbps: 4500, audio_kbps: 160 },
        Tier::Alto => Profile { tier, out_height: 1080, fps: 60, video_kbps: 6000, audio_kbps: 160 },
    }
}

/// Escala manteniendo el aspecto del monitor; ancho par (lo pide el encoder).
pub fn output_size(base_w: u32, base_h: u32, target_h: u32) -> (u32, u32) {
    let h = target_h.min(base_h);
    let w = ((base_w as u64 * h as u64 / base_h as u64) as u32) & !1;
    (w, h & !1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn potato_for_weak_pc() {
        let p = pick_profile(&Machine { threads: 4, ram_gb: 7.9 });
        assert_eq!(p.tier, Tier::Potato);
        assert_eq!((p.out_height, p.fps), (720, 30));
    }

    #[test]
    fn alto_for_strong_pc() {
        assert_eq!(pick_profile(&Machine { threads: 16, ram_gb: 32.0 }).tier, Tier::Alto);
    }

    #[test]
    fn keeps_aspect_and_even() {
        assert_eq!(output_size(1920, 1080, 720), (1280, 720));
        assert_eq!(output_size(1366, 768, 720), (1280, 720));
        assert_eq!(output_size(2560, 1080, 720), (1706, 720));
        assert_eq!(output_size(1280, 720, 1080), (1280, 720));
    }
}
