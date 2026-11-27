//! Clips verticales 9:16 para TikTok, Shorts y Reels.
//! Recorta el centro del clip y lo lleva a 1080x1920 con ffmpeg (en segundo plano, prioridad baja).
//! No se incluye ffmpeg: se usa el que esté junto a chispa.exe o en el PATH.

use std::{
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{anyhow, Context, Result};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const IDLE_PRIORITY_CLASS: u32 = 0x0000_0040;

/// Busca ffmpeg.exe junto al ejecutable y después en el PATH.
pub fn find_ffmpeg() -> Option<PathBuf> {
    let local = std::env::current_exe().ok()?.with_file_name("ffmpeg.exe");
    if local.is_file() {
        return Some(local);
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).map(|p| p.join("ffmpeg.exe")).find(|p| p.is_file())
    })
}

/// Ruta de salida: "clip.mp4" → "clip vertical.mp4"
pub fn vertical_path(clip: &Path) -> PathBuf {
    let stem = clip.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "clip".into());
    clip.with_file_name(format!("{stem} vertical.mp4"))
}

/// Filtro de video: recorte central 9:16 y escala a 1080x1920. Solo usa filtros que existen
/// hasta en versiones viejas de ffmpeg.
pub fn filter() -> &'static str {
    "crop=trunc(ih*9/16/2)*2:ih,scale=1080:1920,setsar=1"
}

/// Convierte (bloquea hasta terminar). Llamar desde un hilo aparte.
pub fn make_vertical(clip: &Path) -> Result<PathBuf> {
    let ffmpeg = find_ffmpeg().ok_or_else(|| {
        anyhow!("Para clips verticales hace falta ffmpeg. Instalalo con: winget install Gyan.FFmpeg")
    })?;
    let out = vertical_path(clip);
    let status = Command::new(ffmpeg)
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(clip)
        .args(["-vf", filter(), "-c:v", "libx264", "-preset", "veryfast", "-crf", "21", "-c:a", "copy"])
        .arg(&out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW | IDLE_PRIORITY_CLASS)
        .status()
        .context("no pude ejecutar ffmpeg")?;
    if !status.success() || !out.is_file() {
        return Err(anyhow!("ffmpeg no pudo crear el clip vertical"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_name() {
        assert_eq!(
            vertical_path(Path::new(r"C:\clips\2026-11-27 10-00-00.mp4")),
            PathBuf::from(r"C:\clips\2026-11-27 10-00-00 vertical.mp4")
        );
    }
}
