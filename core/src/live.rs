//! En vivo: multistream con UN solo encode.
//! Se crean un encoder de video y uno de audio, y cada destino (Twitch, Kick, YouTube) es una
//! salida RTMP propia que comparte esos encoders. Transmitir a 3 plataformas cuesta la CPU/GPU de 1;
//! solo se multiplica el uso de red.
//!
//! El guardián vigila cada salida: frames perdidos por la red, reconexiones y caídas.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Result};
use libobs_simple::wrapper::{
    capabilities::{OutputCapabilities, OutputCompatibilityRequest},
    context::ObsContext,
    data::{
        output::{ObsOutputRef, ObsOutputStatisticsExt, ObsOutputTrait},
        ObsDataSetters,
    },
    encoders::{audio::ObsAudioEncoder, video::ObsVideoEncoder},
    settings::PropertyValue,
};

use crate::engine::StreamTarget;

/// Una salida RTMP a un destino.
struct Dest {
    name: String,
    output: ObsOutputRef,
    /// Última muestra (frames perdidos, frames totales) para calcular la tasa de pérdida
    last: (u64, u64),
    /// % de frames perdidos en la última ventana
    drop_pct: f32,
    reconnecting: bool,
    warned_at: Option<Instant>,
}

/// Estado de salud de un destino, para la UI
#[derive(Clone, Debug, serde::Serialize)]
pub struct DestHealth {
    pub name: String,
    pub drop_pct: f32,
    pub reconnecting: bool,
}

pub struct Live {
    _venc: Arc<ObsVideoEncoder>,
    _aenc: Arc<ObsAudioEncoder>,
    dests: Vec<Dest>,
    pub started: Instant,
}

/// Umbral de pérdida (en %) a partir del cual el guardián avisa
const DROP_WARN_PCT: f32 = 5.0;
/// No repetir el mismo aviso más seguido que esto
const WARN_EVERY: Duration = Duration::from_secs(60);

impl Live {
    /// Arranca el vivo en todos los destinos. Si algunos fallan pero otros no, sigue con los que
    /// andan y devuelve los errores para avisar. Si fallan todos, error.
    pub fn start(ctx: &ObsContext, targets: &[StreamTarget], video_kbps: u32, audio_kbps: u32) -> Result<(Self, Vec<String>)> {
        if targets.is_empty() {
            return Err(anyhow!("Elegí al menos una plataforma en Ajustes"));
        }
        for t in targets {
            if t.key.trim().is_empty() {
                return Err(anyhow!("Falta la clave de {}: cargala en Ajustes", t.name));
            }
        }

        // Un plan RTMP + H.264 + AAC; preferimos el encoder por hardware.
        let caps = ctx.capabilities()?;
        let request = OutputCompatibilityRequest::new()
            .protocol("RTMP")
            .video_codec("h264")
            .audio_codec("aac")
            .prefer_hardware_video(true)
            .require_output_capabilities(
                OutputCapabilities::ENCODED | OutputCapabilities::VIDEO | OutputCapabilities::AUDIO | OutputCapabilities::SERVICE,
            );
        let plan = caps
            .best_output_plan(&request)
            .map_err(|r| anyhow!("No encontré cómo transmitir en esta PC: {}", r.summary()))?;
        let vtype = plan.video_encoder().ok_or_else(|| anyhow!("no hay encoder H.264"))?;
        let atype = plan.audio_encoder().ok_or_else(|| anyhow!("no hay encoder AAC"))?;

        // Encoder de video (CBR, como piden las plataformas). Uno solo para todos los destinos.
        let mut vs = vtype.default_settings_mut()?;
        let vschema = vtype.settings_schema_for(&vs)?;
        set_opt(&vschema, &mut vs, "rate_control", PropertyValue::String("CBR".into()));
        set_opt(&vschema, &mut vs, "bitrate", PropertyValue::Integer(video_kbps as i64));
        // Keyframe cada 2 s: lo exigen Twitch, YouTube y Kick
        set_opt(&vschema, &mut vs, "keyint_sec", PropertyValue::Integer(2));
        let venc = ctx.create_video_encoder(vtype, "chispa-live-video", Some(vs))?;

        let mut as_ = atype.default_settings_mut()?;
        let aschema = atype.settings_schema_for(&as_)?;
        set_opt(&aschema, &mut as_, "bitrate", PropertyValue::Integer(audio_kbps as i64));
        let aenc = ctx.create_audio_encoder(atype, "chispa-live-audio", Some(as_), 0)?;

        let service_type = ctx
            .service_type("rtmp_custom")?
            .ok_or_else(|| anyhow!("falta el servicio rtmp_custom de OBS"))?;

        let mut dests = Vec::new();
        let mut errors = Vec::new();
        for (i, t) in targets.iter().enumerate() {
            let r = (|| -> Result<ObsOutputRef> {
                let mut ss = service_type.default_settings_mut()?;
                let sschema = service_type.settings_schema_for(&ss)?;
                sschema.set(&mut ss, "server", PropertyValue::String(t.url.trim().into()))?;
                sschema.set(&mut ss, "key", PropertyValue::String(t.key.trim().into()))?;
                let service = ctx.create_service(&service_type, format!("chispa-svc-{i}"), Some(ss))?;

                let mut os = plan.output().default_settings_mut()?;
                // Bitrate dinámico de OBS: si la red se congestiona baja la calidad en vez de perder frames
                os.set_bool("dyn_bitrate", true)?;
                let out = ctx
                    .output_pipeline(plan.output(), format!("chispa-live-{i}"), Some(os))
                    .video_encoder(venc.clone())
                    .audio_encoder(0, aenc.clone())
                    .service(service)
                    .build()?
                    .into_output();
                out.start()?;
                Ok(out)
            })();
            match r {
                Ok(output) => dests.push(Dest {
                    name: t.name.clone(),
                    output,
                    last: (0, 0),
                    drop_pct: 0.0,
                    reconnecting: false,
                    warned_at: None,
                }),
                Err(e) => errors.push(format!("{}: {e}", t.name)),
            }
        }
        if dests.is_empty() {
            return Err(anyhow!("No pude salir en vivo. {}", errors.join(" · ")));
        }
        Ok((Self { _venc: venc, _aenc: aenc, dests, started: Instant::now() }, errors))
    }

    pub fn names(&self) -> Vec<String> {
        self.dests.iter().map(|d| d.name.clone()).collect()
    }

    pub fn health(&self) -> Vec<DestHealth> {
        self.dests
            .iter()
            .map(|d| DestHealth { name: d.name.clone(), drop_pct: d.drop_pct, reconnecting: d.reconnecting })
            .collect()
    }

    /// Corta un destino por nombre. Devuelve true si no queda ninguno.
    pub fn stop_one(&mut self, name: &str) -> Result<bool> {
        if let Some(i) = self.dests.iter().position(|d| d.name == name) {
            let d = self.dests.remove(i);
            d.output.stop()?;
        }
        Ok(self.dests.is_empty())
    }

    pub fn stop(self) -> Result<()> {
        let mut first_err = None;
        for d in self.dests {
            if let Err(e) = d.output.stop() {
                first_err.get_or_insert(e);
            }
        }
        match first_err {
            Some(e) => Err(e.into()),
            None => Ok(()),
        }
    }

    /// Guardián. Se llama cada ~0,5 s. Devuelve avisos para mostrar. Quita los destinos que se cayeron.
    pub fn poll(&mut self) -> Vec<String> {
        let mut notes = Vec::new();
        let grace = self.started.elapsed() < Duration::from_secs(8);
        self.dests.retain_mut(|d| {
            let snap = match d.output.status_snapshot() {
                Ok(s) => s,
                Err(_) => return true,
            };
            d.reconnecting = snap.reconnecting;
            if !snap.active && !snap.reconnecting && !grace {
                let why = snap.last_error.map(|e| format!(" ({e})")).unwrap_or_default();
                notes.push(format!("Se cortó la transmisión a {}{why}. Revisá la clave y tu conexión.", d.name));
                return false;
            }
            let dropped = snap.dropped_network_frames.value().copied().unwrap_or(0);
            let total = snap.total_output_frames;
            if let Some(pct) = drop_rate(d.last, (dropped, total)) {
                d.drop_pct = pct;
            }
            d.last = (dropped, total);
            if d.drop_pct >= DROP_WARN_PCT && d.warned_at.is_none_or(|t| t.elapsed() > WARN_EVERY) {
                d.warned_at = Some(Instant::now());
                notes.push(format!(
                    "{}: se pierde el {:.0} % de los frames. Tu subida no alcanza: probá con menos plataformas o un perfil más bajo.",
                    d.name, d.drop_pct
                ));
            }
            true
        });
        notes
    }

    pub fn is_empty(&self) -> bool {
        self.dests.is_empty()
    }
}

/// % de frames perdidos entre dos muestras (perdidos, totales). None si no hubo frames nuevos.
fn drop_rate(prev: (u64, u64), cur: (u64, u64)) -> Option<f32> {
    // Contadores reiniciados (reconexión): empezar de nuevo
    if cur.0 < prev.0 || cur.1 < prev.1 {
        return None;
    }
    // Como en OBS: el total de frames incluye los perdidos
    let total = cur.1 - prev.1;
    let lost = cur.0 - prev.0;
    if total == 0 {
        return None;
    }
    Some((100.0 * lost as f32 / total as f32).min(100.0))
}

fn set_opt(
    schema: &libobs_simple::wrapper::settings::SettingsSchema,
    data: &mut libobs_simple::wrapper::data::ObsData,
    name: &str,
    value: PropertyValue,
) {
    if schema.property(name).is_some() {
        let _ = schema.set(data, name, value);
    }
}

#[cfg(test)]
mod tests {
    use super::drop_rate;

    #[test]
    fn rate_basic() {
        assert_eq!(drop_rate((0, 0), (10, 100)), Some(10.0));
    }

    #[test]
    fn rate_none_without_frames() {
        assert_eq!(drop_rate((5, 100), (5, 100)), None);
    }

    #[test]
    fn rate_reset_after_reconnect() {
        assert_eq!(drop_rate((50, 1000), (0, 10)), None);
    }
}
