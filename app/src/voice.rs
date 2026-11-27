//! Comandos de voz: "Chispa, clipeá eso".
//! Usa el reconocedor de voz que trae Windows (Windows.Media.SpeechRecognition): no descarga modelos,
//! funciona sin internet y, como solo escucha una lista cerrada de frases, gasta muy poca CPU.

use std::sync::mpsc;

use windows_collections::IIterable;

use windows::{
    core::{Ref, HSTRING},
    Foundation::TypedEventHandler,
    Globalization::Language,
    Media::SpeechRecognition::{
        SpeechContinuousRecognitionResultGeneratedEventArgs, SpeechContinuousRecognitionSession,
        SpeechRecognitionConfidence, SpeechRecognitionListConstraint, SpeechRecognitionResultStatus,
        SpeechRecognizer,
    },
    Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED},
};

/// Frase → acción. Todas empiezan con la palabra de activación "Chispa".
pub fn phrases(lang: &str) -> Vec<(&'static str, &'static str)> {
    if lang == "en" {
        vec![
            ("chispa clip that", "clip"),
            ("chispa clip it", "clip"),
            ("chispa vertical clip", "vertical"),
            ("chispa start recording", "record_start"),
            ("chispa stop recording", "record_stop"),
            ("chispa go live", "stream_start"),
            ("chispa go live on twitch", "stream_start:twitch"),
            ("chispa go live on kick", "stream_start:kick"),
            ("chispa go live on youtube", "stream_start:youtube"),
            ("chispa end stream", "stream_stop"),
            ("chispa mute mic", "mic_mute"),
            ("chispa unmute mic", "mic_unmute"),
            ("chispa game scene", "scene:juego"),
            ("chispa be right back", "scene:pausa"),
            ("chispa chat scene", "scene:charla"),
            ("chispa camera on", "camera_on"),
            ("chispa camera off", "camera_off"),
            ("chispa marker", "marker"),
        ]
    } else {
        vec![
            ("chispa clipeá eso", "clip"),
            ("chispa clipea eso", "clip"),
            ("chispa clip", "clip"),
            ("chispa cortá este clip", "clip"),
            ("chispa clip vertical", "vertical"),
            ("chispa empezá a grabar", "record_start"),
            ("chispa grabá", "record_start"),
            ("chispa pará de grabar", "record_stop"),
            ("chispa en vivo", "stream_start"),
            ("chispa en vivo en twitch", "stream_start:twitch"),
            ("chispa en vivo en kick", "stream_start:kick"),
            ("chispa en vivo en youtube", "stream_start:youtube"),
            ("chispa en vivo en todas", "stream_start:all"),
            ("chispa cortá el stream", "stream_stop"),
            ("chispa terminá el stream", "stream_stop"),
            ("chispa muteá el micrófono", "mic_mute"),
            ("chispa activá el micrófono", "mic_unmute"),
            ("chispa escena juego", "scene:juego"),
            ("chispa ya vuelvo", "scene:pausa"),
            ("chispa escena charla", "scene:charla"),
            ("chispa prendé la cámara", "camera_on"),
            ("chispa apagá la cámara", "camera_off"),
            ("chispa marcador", "marker"),
        ]
    }
}

pub fn action_for(lang: &str, text: &str) -> Option<&'static str> {
    let t = text.trim().trim_end_matches(['.', '!']).to_lowercase().replace(',', "");
    phrases(lang).into_iter().find(|(p, _)| *p == t).map(|(_, a)| a)
}

/// Escucha mientras viva. Al soltarlo deja de escuchar.
pub struct Voice {
    stop: mpsc::Sender<()>,
}

impl Drop for Voice {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

impl Voice {
    /// Arranca el reconocedor en un hilo propio. `on_action` recibe ("clip", texto oído).
    /// Devuelve error si Windows no tiene reconocimiento de voz para ese idioma o no hay micrófono.
    pub fn start(lang: &str, on_action: impl Fn(&'static str, String) + Send + Sync + 'static) -> Result<(Self, String), String> {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<String, String>>();
        let lang = lang.to_string();
        std::thread::Builder::new()
            .name("chispa-voice".into())
            .spawn(move || {
                let session = match setup(&lang, on_action) {
                    Ok((s, rec, name)) => {
                        let _ = ready_tx.send(Ok(name));
                        (s, rec)
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let _ = stop_rx.recv();
                if let Ok(op) = session.0.StopAsync() {
                    let _ = op.join();
                }
                drop(session);
            })
            .map_err(|e| e.to_string())?;
        let name = ready_rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .map_err(|_| "El reconocedor de voz no respondió".to_string())??;
        Ok((Self { stop: stop_tx }, name))
    }
}

fn setup(
    lang: &str,
    on_action: impl Fn(&'static str, String) + Send + Sync + 'static,
) -> Result<(SpeechContinuousRecognitionSession, SpeechRecognizer, String), String> {
    let w = |e: windows::core::Error| format!("Voz: {}", e.message());
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    // Idioma: el del usuario si Windows lo tiene instalado; si no, el del sistema.
    let wanted = if lang == "en" { "en" } else { "es" };
    let installed = SpeechRecognizer::SupportedGrammarLanguages().map_err(w)?;
    let mut chosen: Option<Language> = None;
    for l in installed {
        if l.LanguageTag().map(|t| t.to_string().to_lowercase().starts_with(wanted)).unwrap_or(false) {
            chosen = Some(l);
            break;
        }
    }
    let language = match chosen {
        Some(l) => l,
        None => {
            return Err(format!(
                "Windows no tiene reconocimiento de voz en {}. Agregalo en Configuración → Hora e idioma → Voz.",
                if wanted == "en" { "inglés" } else { "español" }
            ))
        }
    };
    let lang_name = language.DisplayName().map(|n| n.to_string()).unwrap_or_default();
    let rec = SpeechRecognizer::Create(&language).map_err(w)?;

    let list: Vec<HSTRING> = phrases(wanted).iter().map(|(p, _)| HSTRING::from(*p)).collect();
    let iterable: IIterable<HSTRING> = list.into();
    let constraint = SpeechRecognitionListConstraint::CreateWithTag(&iterable, &HSTRING::from("chispa")).map_err(w)?;
    rec.Constraints().map_err(w)?.Append(&constraint).map_err(w)?;
    let compiled = rec.CompileConstraintsAsync().map_err(w)?.join().map_err(w)?;
    if compiled.Status().map_err(w)? != SpeechRecognitionResultStatus::Success {
        return Err("Voz: no pude preparar los comandos".into());
    }

    let lang_owned = wanted.to_string();
    let session = rec.ContinuousRecognitionSession().map_err(w)?;
    session
        .ResultGenerated(&TypedEventHandler::new(
            move |_s: Ref<SpeechContinuousRecognitionSession>, args: Ref<SpeechContinuousRecognitionResultGeneratedEventArgs>| {
                if let Some(args) = args.as_ref() {
                    let r = args.Result()?;
                    let conf = r.Confidence()?;
                    if conf == SpeechRecognitionConfidence::High || conf == SpeechRecognitionConfidence::Medium {
                        let text = r.Text()?.to_string();
                        if let Some(a) = action_for(&lang_owned, &text) {
                            on_action(a, text);
                        }
                    }
                }
                Ok(())
            },
        ))
        .map_err(w)?;
    session.StartAsync().map_err(|e| {
        if e.code().0 as u32 == 0x8004_5509 {
            "Voz: Windows pide aceptar la política de privacidad de voz (Configuración → Privacidad → Voz)".to_string()
        } else {
            w(e)
        }
    })?
    .join()
    .map_err(w)?;
    Ok((session, rec, lang_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_spanish_phrases() {
        assert_eq!(action_for("es", "Chispa, clipeá eso."), Some("clip"));
        assert_eq!(action_for("es", "chispa en vivo en kick"), Some("stream_start:kick"));
        assert_eq!(action_for("es", "hola"), None);
    }

    #[test]
    fn every_phrase_has_wake_word() {
        for l in ["es", "en"] {
            assert!(phrases(l).iter().all(|(p, _)| p.starts_with("chispa ")));
        }
    }
}
