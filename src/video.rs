//! Inline video playback (cargo feature `video`).
//!
//! Wraps asciline's decoder so a video file is decoded off the main thread
//! into RGBA8 frames. The app then hands each frame to the renderer, which
//! reuses the sixel/kitty image pipeline to draw it over the terminal.
//!
//! `ffmpeg` (and `ffprobe` for probing) is spawned as a subprocess and must be
//! on PATH at runtime.

use crossbeam_channel::Receiver;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

/// A decoded RGBA8 video frame.
#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A running video decode stream. Frames arrive via [`VideoStream::recv`] /
/// [`VideoStream::try_recv`]; the channel is closed (recv returns `Err`) at
/// end-of-stream or after [`VideoStream::stop`].
pub struct VideoStream {
    rx: Receiver<VideoFrame>,
    stop: Arc<AtomicBool>,
    pid: Arc<AtomicU32>,
    handle: Option<JoinHandle<()>>,
}

impl VideoStream {
    /// True once the decode thread has exited (EOF or stopped).
    pub fn finished(&self) -> bool {
        self.handle.as_ref().is_some_and(|h| h.is_finished())
    }

    /// Blocking receive of the next frame; `Err` once the stream ends.
    pub fn recv(&self) -> Result<VideoFrame, crossbeam_channel::RecvError> {
        self.rx.recv()
    }

    /// Non-blocking receive of the next frame.
    pub fn try_recv(&self) -> Result<VideoFrame, crossbeam_channel::TryRecvError> {
        self.rx.try_recv()
    }

    /// Ask the decoder to stop (kills the ffmpeg child so a blocked pipe read
    /// unblocks) and join the thread.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let pid = self.pid.load(Ordering::SeqCst);
        if pid != 0 {
            use nix::libc;
            // SAFETY: kill() with a valid signal on a child pid is safe.
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Metadata about a video source (see [`asciline::video::probe_video`]).
#[derive(Debug, Clone, Copy)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration: f64,
}

/// Probe a media file's dimensions / fps / duration via `ffprobe`.
pub fn probe(src: &str) -> Result<VideoInfo, String> {
    let info = asciline::video::probe_video(src, false).map_err(|e| format!("{e:#}"))?;
    Ok(VideoInfo {
        width: info.width,
        height: info.height,
        fps: info.fps,
        duration: info.duration,
    })
}

/// Spawn a decoder for `src` (a video file) producing RGBA8 frames of
/// `cols x rows` pixels (passed to ffmpeg's `scale=` filter). The channel is
/// bounded so a slow consumer applies backpressure to the decoder.
pub fn start(src: String, cols: u32, rows: u32) -> Result<VideoStream, String> {
    let (tx, rx) = crossbeam_channel::bounded::<VideoFrame>(4);
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = Arc::clone(&stop);
    let pid = Arc::new(AtomicU32::new(0));
    let pid2 = Arc::clone(&pid);

    let handle = std::thread::Builder::new()
        .name("video-decode".into())
        .spawn(move || {
            use asciline::video::{FrameReader, SourceParams};
            let params = SourceParams {
                src,
                is_webcam: false,
                cols,
                rows,
                target_fps: None,
                seek_secs: 0.0,
                mirror: false,
            };
            let mut reader = match FrameReader::new(&params, &pid2) {
                Ok(r) => r,
                Err(e) => {
                    log::debug!("video: decode failed: {e:#}");
                    return;
                }
            };
            while !stop2.load(Ordering::Relaxed) {
                let Some(rgb) = reader.read_frame() else {
                    break; // EOF
                };
                let rgba = rgb24_to_rgba(&rgb);
                if tx
                    .send(VideoFrame {
                        width: reader.cols,
                        height: reader.rows,
                        rgba,
                    })
                    .is_err()
                {
                    break; // receiver dropped
                }
            }
        })
        .map_err(|e| format!("spawn decode thread: {e}"))?;

    Ok(VideoStream {
        rx,
        stop,
        pid,
        handle: Some(handle),
    })
}

/// Convert an RGB24 buffer (`w*h*3`) to RGBA8 (`w*h*4`, opaque alpha).
fn rgb24_to_rgba(rgb: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgb.len() / 3 * 4);
    for px in rgb.chunks_exact(3) {
        out.extend_from_slice(&[px[0], px[1], px[2], 255]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb24_to_rgba_expands_and_sets_alpha() {
        let rgb = [255, 0, 0, 0, 255, 0];
        assert_eq!(rgb24_to_rgba(&rgb), vec![255, 0, 0, 255, 0, 255, 0, 255]);
    }

    /// Decode a tiny ffmpeg-generated video end-to-end (requires ffmpeg).
    #[test]
    fn decodes_generated_video() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .map(|o| !o.status.success())
            .unwrap_or(true)
        {
            eprintln!("skipping: ffmpeg not available");
            return;
        }
        let dir = std::env::temp_dir().join(format!("term-video-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("clip.mp4");
        // 2 frames of solid red 16x8 at 25 fps.
        let status = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y"])
            .args(["-f", "lavfi", "-i", "color=c=red:s=16x8:d=0.08:r=25"])
            .args(["-pix_fmt", "yuv420p"])
            .arg(&path)
            .status()
            .expect("run ffmpeg");
        assert!(status.success(), "ffmpeg failed");

        let stream = start(path.to_string_lossy().into_owned(), 16, 8).expect("start stream");
        let mut frames = 0;
        while let Ok(frame) = stream.recv() {
            assert_eq!((frame.width, frame.height), (16, 8));
            assert_eq!(frame.rgba.len(), 16 * 8 * 4);
            // Red-dominant pixels.
            assert!(frame.rgba[0] > 200 && frame.rgba[1] < 40 && frame.rgba[2] < 40);
            frames += 1;
        }
        assert!(frames >= 2, "expected at least 2 frames, got {frames}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
