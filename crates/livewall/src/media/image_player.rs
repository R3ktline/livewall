use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use image::AnimationDecoder;

use super::Frame;

pub struct ImagePlayer {
    static_frame: Option<Frame>,
    frames: Vec<(Frame, Duration)>,
    index: usize,
    last_delay: Duration,
}

impl ImagePlayer {
    pub fn open(path: &Path) -> Result<Self> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        if ext == "gif" {
            let f = File::open(path)?;
            let decoder = image::codecs::gif::GifDecoder::new(BufReader::new(f))
                .context("gif decoder")?;
            return Self::from_animation(decoder.into_frames());
        }

        // webp may be animated; try animation first, fall back to still.
        if ext == "webp" {
            let data = std::fs::read(path)?;
            if let Ok(decoder) = image::codecs::webp::WebPDecoder::new(std::io::Cursor::new(&data))
            {
                if decoder.has_animation() {
                    // Re-open: WebPDecoder::new consumes; decode still via image::open for frames
                    // image crate WebPDecoder into_frames
                    let decoder =
                        image::codecs::webp::WebPDecoder::new(std::io::Cursor::new(&data))?;
                    return Self::from_animation(decoder.into_frames());
                }
            }
        }

        let img = image::open(path)
            .with_context(|| format!("opening image {}", path.display()))?
            .to_rgba8();
        let frame = Frame::from_rgba8(img.width(), img.height(), &img);
        Ok(Self {
            static_frame: Some(frame),
            frames: Vec::new(),
            index: 0,
            last_delay: Duration::from_millis(100),
        })
    }

    fn from_animation(
        frames: image::Frames,
    ) -> Result<Self> {
        let mut out = Vec::new();
        for f in frames {
            let f = f.context("animation frame")?;
            let delay = Duration::from(f.delay());
            let rgba = f.into_buffer();
            out.push((
                Frame::from_rgba8(rgba.width(), rgba.height(), &rgba),
                if delay.is_zero() {
                    Duration::from_millis(100)
                } else {
                    delay
                },
            ));
        }
        if out.is_empty() {
            anyhow::bail!("animation contained no frames");
        }
        Ok(Self {
            static_frame: None,
            frames: out,
            index: 0,
            last_delay: Duration::from_millis(100),
        })
    }

    pub fn next_frame(&mut self) -> Result<Option<Frame>> {
        if let Some(f) = self.static_frame.take() {
            return Ok(Some(f));
        }
        if self.frames.is_empty() {
            return Ok(None);
        }
        if self.index >= self.frames.len() {
            return Ok(None);
        }
        let (frame, delay) = &self.frames[self.index];
        self.last_delay = *delay;
        self.index += 1;
        // Clone pixels for SHM frames
        let frame = match frame {
            Frame::Shm {
                width,
                height,
                stride,
                pixels,
            } => Frame::Shm {
                width: *width,
                height: *height,
                stride: *stride,
                pixels: pixels.clone(),
            },
            Frame::Dmabuf(_) => anyhow::bail!("unexpected dmabuf in image player"),
        };
        Ok(Some(frame))
    }

    pub fn reset(&mut self) -> Result<()> {
        self.index = 0;
        Ok(())
    }

    pub fn last_delay(&self) -> Duration {
        self.last_delay
    }
}
