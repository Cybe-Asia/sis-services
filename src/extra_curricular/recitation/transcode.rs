//! Clips iOS cannot play (`AVAudioPlayer` has no WebM or Ogg) are stored as AAC-LC in MP4, mono at
//! 64 kbps, which iOS, Android and every browser play. Chrome and Firefox record WebM or Ogg (Opus or
//! Vorbis); MP4, MP3 and WAV are kept as sent. The work is done by the `ffmpeg` built into the image
//! with only these demuxers, decoders and the AAC encoder (see the Dockerfile).
use super::model::{looks_like, MAX_STORED_BYTES};
use std::path::Path;
use std::time::Duration;

pub const PLAYABLE: &str = "audio/mp4";
const FFMPEG: &str = "ffmpeg";
const TIME_LIMIT: Duration = Duration::from_secs(30);

/// The ffmpeg input format for a type that needs converting, or `None` when it plays everywhere.
pub fn source_format(mime: &str) -> Option<&'static str> {
    match mime {
        "audio/webm" => Some("matroska"),
        "audio/ogg" => Some("ogg"),
        _ => None,
    }
}

/// The clip to store and its type: the AAC rendition when it is needed and works, else the clip as
/// sent. A failed conversion never loses the upload; it is logged without naming anyone.
pub async fn playable(audio: Vec<u8>, mime: &str) -> (Vec<u8>, String) {
    playable_with(FFMPEG, audio, mime).await
}

pub async fn playable_with(program: &str, audio: Vec<u8>, mime: &str) -> (Vec<u8>, String) {
    let Some(format) = source_format(mime) else {
        return (audio, mime.to_string());
    };
    match to_aac(program, &audio, format).await {
        Ok(aac) => (aac, PLAYABLE.to_string()),
        Err(reason) => {
            tracing::warn!(
                reason,
                source = mime,
                "recitation clip kept as sent: AAC conversion failed"
            );
            (audio, mime.to_string())
        }
    }
}

async fn to_aac(program: &str, input: &[u8], format: &str) -> Result<Vec<u8>, &'static str> {
    let dir = std::env::temp_dir().join(format!("recitation-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir(&dir)
        .await
        .map_err(|_| "no temporary directory")?;
    let result = convert(program, &dir, input, format).await;
    let _ = tokio::fs::remove_dir_all(&dir).await;
    result
}

async fn convert(
    program: &str,
    dir: &Path,
    input: &[u8],
    format: &str,
) -> Result<Vec<u8>, &'static str> {
    let (source, target) = (dir.join("clip"), dir.join("clip.m4a"));
    tokio::fs::write(&source, input)
        .await
        .map_err(|_| "input not written")?;
    // The input format is named, never guessed, and only local files are read.
    let status = tokio::process::Command::new(program)
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-protocol_whitelist",
            "file",
            "-f",
            format,
            "-i",
        ])
        .arg(&source)
        .args([
            "-vn",
            "-map_metadata",
            "-1",
            "-ac",
            "1",
            "-c:a",
            "aac",
            "-b:a",
            "64k",
            "-movflags",
            "+faststart",
            "-f",
            "mp4",
        ])
        .arg(&target)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .status();
    match tokio::time::timeout(TIME_LIMIT, status).await {
        Err(_) => return Err("timed out"),
        Ok(Err(_)) => return Err("ffmpeg not runnable"),
        Ok(Ok(s)) if !s.success() => return Err("ffmpeg refused the clip"),
        Ok(Ok(_)) => {}
    }
    let aac = tokio::fs::read(&target).await.map_err(|_| "no output")?;
    if aac.len() > MAX_STORED_BYTES || !looks_like(PLAYABLE, &aac) {
        return Err("output out of bounds");
    }
    Ok(aac)
}

#[cfg(test)]
mod tests {
    use super::*;
    const WEBM: &[u8] = include_bytes!("fixtures/tone.webm");
    const OGG: &[u8] = include_bytes!("fixtures/tone.ogg");

    #[test]
    fn only_webm_and_ogg_are_converted() {
        assert_eq!(source_format("audio/webm"), Some("matroska"));
        assert_eq!(source_format("audio/ogg"), Some("ogg"));
        for kept in ["audio/mp4", "audio/mpeg", "audio/wav"] {
            assert_eq!(source_format(kept), None, "{kept}");
        }
        assert!(looks_like("audio/webm", WEBM) && looks_like("audio/ogg", OGG));
    }

    #[tokio::test]
    async fn a_type_every_player_reads_is_stored_as_sent() {
        let wav = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        // The program is never run for these, so a missing one cannot matter.
        assert_eq!(
            playable_with("/nonexistent/ffmpeg", wav.clone(), "audio/wav").await,
            (wav, "audio/wav".into())
        );
    }

    #[tokio::test]
    async fn a_failed_conversion_keeps_the_clip_as_sent() {
        let (kept, mime) = playable_with("/nonexistent/ffmpeg", WEBM.to_vec(), "audio/webm").await;
        assert_eq!((kept.as_slice(), mime.as_str()), (WEBM, "audio/webm"));
        // A clip labelled Ogg that is not Ogg is refused by ffmpeg and also kept.
        let (kept, mime) = playable_with("ffmpeg", WEBM.to_vec(), "audio/ogg").await;
        assert_eq!((kept.as_slice(), mime.as_str()), (WEBM, "audio/ogg"));
    }

    #[tokio::test]
    #[ignore = "needs ffmpeg on PATH (the image has it; locally: brew install ffmpeg)"]
    async fn webm_and_ogg_become_mono_aac_in_mp4() {
        for (clip, mime) in [(WEBM, "audio/webm"), (OGG, "audio/ogg")] {
            let (aac, stored) = playable(clip.to_vec(), mime).await;
            assert_eq!(stored, PLAYABLE, "{mime}");
            assert!(looks_like(PLAYABLE, &aac));
            // AAC-LC: the MP4 sample entry is `mp4a`.
            assert!(aac.windows(4).any(|w| w == b"mp4a"), "{mime}");
        }
    }
}
