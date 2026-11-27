//! Detección del micrófono. En muchas PCs el "micrófono" por defecto de Windows es en realidad
//! "Mezcla estéreo" (loopback del audio del escritorio): eso duplica el audio. Acá se detecta.

use windows::Win32::{
    Media::Audio::{eCapture, eConsole, IMMDeviceEnumerator, MMDeviceEnumerator},
    System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED},
};

/// ID (MMDevice) del dispositivo de entrada por defecto de Windows. Es el mismo ID que usa OBS en `device_id`.
pub fn default_capture_id() -> Option<String> {
    unsafe {
        // Puede devolver "ya inicializado"; da igual
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let dev = en.GetDefaultAudioEndpoint(eCapture, eConsole).ok()?;
        let id = dev.GetId().ok()?;
        let s = id.to_string().ok();
        CoTaskMemFree(Some(id.0 as *const _));
        s
    }
}

/// ¿Este dispositivo de entrada es un loopback del escritorio y no un micrófono real?
pub fn is_loopback(name: &str) -> bool {
    let n = name.to_lowercase();
    ["mezcla estéreo", "mezcla estereo", "stereo mix", "what u hear", "wave out", "loopback", "stereomix"]
        .iter()
        .any(|k| n.contains(k))
}

#[derive(Debug, Clone, PartialEq)]
pub enum MicChoice {
    /// Se usa el dispositivo por defecto (es un mic real)
    Default(String),
    /// El default era un loopback; se eligió este otro dispositivo (nombre, id)
    Switched { from: String, to: String, id: String },
    /// No hay ningún micrófono real
    None,
}

/// Decide qué micrófono usar. `devices` = (nombre, id) tal como los lista OBS, sin la entrada "default".
pub fn choose_mic(devices: &[(String, String)], default_id: Option<&str>) -> MicChoice {
    let default = default_id.and_then(|d| devices.iter().find(|(_, id)| id == d));
    match default {
        Some((name, _)) if !is_loopback(name) => MicChoice::Default(name.clone()),
        other => match devices.iter().find(|(n, _)| !is_loopback(n)) {
            Some((n, id)) => MicChoice::Switched {
                from: other.map(|(n, _)| n.clone()).unwrap_or_else(|| "desconocido".into()),
                to: n.clone(),
                id: id.clone(),
            },
            None => MicChoice::None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn devs() -> Vec<(String, String)> {
        vec![
            ("Mezcla estéreo (Realtek High Definition Audio)".into(), "id-mix".into()),
            ("Micrófono (USB Audio Device)".into(), "id-usb".into()),
        ]
    }

    #[test]
    fn switches_away_from_stereo_mix() {
        assert_eq!(
            choose_mic(&devs(), Some("id-mix")),
            MicChoice::Switched {
                from: "Mezcla estéreo (Realtek High Definition Audio)".into(),
                to: "Micrófono (USB Audio Device)".into(),
                id: "id-usb".into()
            }
        );
    }

    #[test]
    fn keeps_real_default() {
        assert_eq!(choose_mic(&devs(), Some("id-usb")), MicChoice::Default("Micrófono (USB Audio Device)".into()));
    }

    #[test]
    fn none_when_only_loopback() {
        let only = vec![("Stereo Mix (Realtek)".to_string(), "x".to_string())];
        assert_eq!(choose_mic(&only, Some("x")), MicChoice::None);
    }
}
