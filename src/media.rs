//! Small, dependency-light media encoders. Video is standard MJPEG/PCM AVI:
//! all drawing, JPEG encoding, WAV parsing and AVI muxing happen in Rust. No
//! ffmpeg, browser automation, shelling out, or non-Rust rendering service.

use std::{
    fs::File,
    io::{Seek, SeekFrom, Write},
    path::Path,
};

use anyhow::{bail, Context, Result};
use image::{codecs::jpeg::JpegEncoder, Rgb, RgbImage};
use rusttype::{point, Font, Scale};

use crate::model::private_file;

#[derive(Clone, Debug)]
pub struct Audio {
    pub rate: u32,
    pub samples: Vec<i16>, // 16-bit mono PCM
}

impl Audio {
    pub fn from_wav(bytes: &[u8]) -> Result<Self> {
        if bytes.get(0..4) != Some(b"RIFF".as_slice())
            || bytes.get(8..12) != Some(b"WAVE".as_slice())
        {
            bail!("Not a RIFF/WAVE file");
        }
        let mut fmt = None;
        let mut data = None;
        let mut offset = 12usize;
        while offset.checked_add(8).is_some_and(|end| end <= bytes.len()) {
            let id = &bytes[offset..offset + 4];
            let len = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into()?) as usize;
            let start = offset + 8;
            let end = start.checked_add(len).context("WAV chunk overflow")?;
            if end > bytes.len() {
                bail!("Truncated WAV chunk");
            }
            if id == b"fmt " {
                fmt = Some(&bytes[start..end]);
            }
            if id == b"data" {
                data = Some(&bytes[start..end]);
            }
            offset = end.checked_add(len % 2).context("WAV padding overflow")?;
        }
        let fmt = fmt.context("WAV is missing a format chunk")?;
        let data = data.context("WAV is missing audio data")?;
        if fmt.len() < 16 {
            bail!("Invalid WAV format chunk");
        }
        let format = u16::from_le_bytes(fmt[0..2].try_into()?);
        let channels = u16::from_le_bytes(fmt[2..4].try_into()?);
        let rate = u32::from_le_bytes(fmt[4..8].try_into()?);
        let bits = u16::from_le_bytes(fmt[14..16].try_into()?);
        if format != 1
            || !(1..=2).contains(&channels)
            || bits != 16
            || !(8_000..=96_000).contains(&rate)
        {
            bail!("WAV must be 16-bit PCM, mono or stereo, 8–96 kHz");
        }
        let frame_bytes = channels as usize * 2;
        let samples = data
            .chunks_exact(frame_bytes)
            .map(|frame| {
                let left = i16::from_le_bytes([frame[0], frame[1]]) as i32;
                if channels == 1 {
                    left as i16
                } else {
                    let right = i16::from_le_bytes([frame[2], frame[3]]) as i32;
                    ((left + right) / 2) as i16
                }
            })
            .collect();
        Ok(Self { rate, samples })
    }

    pub fn resampled(&self, rate: u32) -> Self {
        if self.rate == rate {
            return self.clone();
        }
        let len = (self.samples.len() as u64 * rate as u64 / self.rate as u64) as usize;
        let samples = (0..len)
            .map(|i| {
                let source = i as f64 * self.rate as f64 / rate as f64;
                let low = source as usize;
                let high = (low + 1).min(self.samples.len().saturating_sub(1));
                let fraction = source.fract();
                let a = *self.samples.get(low).unwrap_or(&0) as f64;
                let b = *self.samples.get(high).unwrap_or(&0) as f64;
                (a + (b - a) * fraction) as i16
            })
            .collect();
        Self { rate, samples }
    }

    pub fn to_wav(&self) -> Result<Vec<u8>> {
        let size = self
            .samples
            .len()
            .checked_mul(2)
            .context("audio too long")?;
        let size = u32::try_from(size).context("audio exceeds WAV size limit")?;
        let mut out = Vec::with_capacity(size as usize + 44);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + size).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&self.rate.to_le_bytes());
        out.extend_from_slice(&(self.rate * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&size.to_le_bytes());
        for sample in &self.samples {
            out.extend_from_slice(&sample.to_le_bytes());
        }
        Ok(out)
    }
}

pub fn join_speech(wavs: &[Vec<u8>], pause_ms: u32) -> Result<Audio> {
    let mut tracks = wavs
        .iter()
        .map(|wav| Audio::from_wav(wav))
        .collect::<Result<Vec<_>>>()?;
    if tracks.is_empty() {
        bail!("Speech provider returned no clips");
    }
    let rate = tracks[0].rate;
    let mut joined = Audio {
        rate,
        samples: Vec::new(),
    };
    let count = tracks.len();
    for (i, track) in tracks.drain(..).enumerate() {
        joined.samples.extend(track.resampled(rate).samples);
        if i + 1 < count {
            joined.samples.extend(std::iter::repeat_n(
                0,
                (rate as u64 * pause_ms as u64 / 1_000) as usize,
            ));
        }
    }
    Ok(joined)
}

pub struct VideoSlide {
    pub title: String,
    pub bullets: Vec<String>,
}

pub struct VideoInfo {
    pub frames: usize,
    pub seconds: usize,
    pub narrated: bool,
}

/// Export a playable AVI (MJPEG at 2 fps, optional 16-bit mono PCM audio).
/// One video frame is a designed slide; the progress indicator moves on each
/// frame. Narration determines slide duration when a TTS provider is available.
pub fn write_video(
    path: &Path,
    slides: &[VideoSlide],
    narration: &[Option<Audio>],
) -> Result<VideoInfo> {
    if slides.is_empty() || slides.len() != narration.len() {
        bail!("Video needs matching slides and narration");
    }
    const WIDTH: u32 = 960;
    const HEIGHT: u32 = 540;
    const FPS: u32 = 2;
    let narrated = narration.iter().any(Option::is_some);
    let rate = narration
        .iter()
        .flatten()
        .next()
        .map(|a| a.rate)
        .unwrap_or(24_000);
    let mut frames_per_slide = Vec::new();
    let mut pcm = Vec::new();
    for audio in narration {
        let audio = audio.as_ref().map(|a| a.resampled(rate));
        let duration = audio
            .as_ref()
            .map(|a| a.samples.len() as f64 / rate as f64 + 0.4)
            .unwrap_or(4.0);
        let frames = (duration * FPS as f64).ceil().clamp(6.0, 40.0) as usize;
        frames_per_slide.push(frames);
        if narrated {
            let size = (frames as u64 * rate as u64 / FPS as u64) as usize;
            let mut samples = audio.map(|a| a.samples).unwrap_or_default();
            samples.resize(size, 0);
            pcm.extend(samples);
        }
    }
    let total_frames: usize = frames_per_slide.iter().sum();
    let header = avi_header(
        WIDTH,
        HEIGHT,
        FPS,
        total_frames as u32,
        if narrated {
            Some((rate, pcm.len() as u32))
        } else {
            None
        },
    );
    let mut file = private_file(path)?;
    file.write_all(&header.bytes)?;
    let movi_start = header.movi_tag as u64;
    let mut index: Vec<([u8; 4], u32, u32, u32)> =
        Vec::with_capacity(total_frames * if narrated { 2 } else { 1 });
    let font = Font::try_from_bytes(include_bytes!("../assets/DejaVuSans.ttf") as &[u8])
        .context("embedded font is invalid")?;
    let bold = Font::try_from_bytes(include_bytes!("../assets/DejaVuSans-Bold.ttf") as &[u8])
        .context("embedded font is invalid")?;
    let mut frame_number = 0usize;
    for (slide_index, (slide, &count)) in slides.iter().zip(&frames_per_slide).enumerate() {
        for step in 0..count {
            let image = draw_slide(
                WIDTH,
                HEIGHT,
                slide,
                slide_index,
                slides.len(),
                step,
                count,
                &font,
                &bold,
            );
            let mut jpeg = Vec::new();
            JpegEncoder::new_with_quality(&mut jpeg, 83).encode_image(&image)?;
            let pos = file.stream_position()?;
            avi_chunk(&mut file, b"00dc", &jpeg)?;
            index.push((*b"00dc", 0x10, (pos - movi_start) as u32, jpeg.len() as u32));
            if narrated {
                let start = frame_number * rate as usize / FPS as usize;
                let end = (frame_number + 1) * rate as usize / FPS as usize;
                let mut data = Vec::with_capacity((end - start) * 2);
                for sample in &pcm[start..end] {
                    data.extend_from_slice(&sample.to_le_bytes());
                }
                let pos = file.stream_position()?;
                avi_chunk(&mut file, b"01wb", &data)?;
                index.push((*b"01wb", 0, (pos - movi_start) as u32, data.len() as u32));
            }
            frame_number += 1;
        }
    }
    let movi_end = file.stream_position()?;
    let movi_size = (movi_end - (header.movi_size_at as u64 + 4)) as u32;
    patch_u32(&mut file, header.movi_size_at as u64, movi_size)?;
    file.seek(SeekFrom::End(0))?;
    file.write_all(b"idx1")?;
    file.write_all(&((index.len() * 16) as u32).to_le_bytes())?;
    for (id, flags, offset, size) in index {
        file.write_all(&id)?;
        file.write_all(&flags.to_le_bytes())?;
        file.write_all(&offset.to_le_bytes())?;
        file.write_all(&size.to_le_bytes())?;
    }
    let len = file.stream_position()?;
    patch_u32(&mut file, 4, (len - 8) as u32)?;
    file.sync_all()?;
    Ok(VideoInfo {
        frames: total_frames,
        seconds: total_frames / FPS as usize,
        narrated,
    })
}

struct Header {
    bytes: Vec<u8>,
    movi_size_at: usize,
    movi_tag: usize,
}

fn avi_header(width: u32, height: u32, fps: u32, frames: u32, audio: Option<(u32, u32)>) -> Header {
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    push_u32(&mut b, 0);
    b.extend_from_slice(b"AVI ");
    let hdrl = start_list(&mut b, b"hdrl");
    let mut avih = Vec::new();
    push_u32(&mut avih, 1_000_000 / fps);
    push_u32(&mut avih, 2_500_000);
    push_u32(&mut avih, 0);
    push_u32(&mut avih, 0x10); // AVIF_HASINDEX
    push_u32(&mut avih, frames);
    push_u32(&mut avih, 0);
    push_u32(&mut avih, if audio.is_some() { 2 } else { 1 });
    push_u32(&mut avih, width * height * 3);
    push_u32(&mut avih, width);
    push_u32(&mut avih, height);
    for _ in 0..4 {
        push_u32(&mut avih, 0);
    }
    chunk(&mut b, b"avih", &avih);
    let strl = start_list(&mut b, b"strl");
    let mut strh = Vec::new();
    strh.extend_from_slice(b"vidsMJPG");
    push_u32(&mut strh, 0);
    push_u16(&mut strh, 0);
    push_u16(&mut strh, 0);
    push_u32(&mut strh, 0);
    push_u32(&mut strh, 1);
    push_u32(&mut strh, fps);
    push_u32(&mut strh, 0);
    push_u32(&mut strh, frames);
    push_u32(&mut strh, width * height * 3);
    push_u32(&mut strh, u32::MAX);
    push_u32(&mut strh, 0);
    push_u16(&mut strh, 0);
    push_u16(&mut strh, 0);
    push_u16(&mut strh, width as u16);
    push_u16(&mut strh, height as u16);
    chunk(&mut b, b"strh", &strh);
    let mut strf = Vec::new();
    push_u32(&mut strf, 40);
    push_u32(&mut strf, width);
    push_u32(&mut strf, height);
    push_u16(&mut strf, 1);
    push_u16(&mut strf, 24);
    strf.extend_from_slice(b"MJPG");
    push_u32(&mut strf, width * height * 3);
    for _ in 0..4 {
        push_u32(&mut strf, 0);
    }
    chunk(&mut b, b"strf", &strf);
    end_list(&mut b, strl);
    if let Some((rate, samples)) = audio {
        let strl = start_list(&mut b, b"strl");
        let mut strh = Vec::new();
        strh.extend_from_slice(b"auds");
        push_u32(&mut strh, 0);
        push_u32(&mut strh, 0);
        push_u16(&mut strh, 0);
        push_u16(&mut strh, 0);
        push_u32(&mut strh, 0);
        push_u32(&mut strh, 2);
        push_u32(&mut strh, rate * 2);
        push_u32(&mut strh, 0);
        push_u32(&mut strh, samples);
        push_u32(&mut strh, 8_192);
        push_u32(&mut strh, u32::MAX);
        push_u32(&mut strh, 2);
        for _ in 0..4 {
            push_u16(&mut strh, 0);
        }
        chunk(&mut b, b"strh", &strh);
        let mut strf = Vec::new();
        push_u16(&mut strf, 1);
        push_u16(&mut strf, 1);
        push_u32(&mut strf, rate);
        push_u32(&mut strf, rate * 2);
        push_u16(&mut strf, 2);
        push_u16(&mut strf, 16);
        chunk(&mut b, b"strf", &strf);
        end_list(&mut b, strl);
    }
    end_list(&mut b, hdrl);
    let movi_size_at = start_list(&mut b, b"movi");
    let movi_tag = b.len() - 4;
    Header {
        bytes: b,
        movi_size_at,
        movi_tag,
    }
}

fn push_u32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}
fn push_u16(b: &mut Vec<u8>, v: u16) {
    b.extend_from_slice(&v.to_le_bytes());
}
fn chunk(b: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    b.extend_from_slice(tag);
    push_u32(b, data.len() as u32);
    b.extend_from_slice(data);
    if data.len() % 2 != 0 {
        b.push(0);
    }
}
fn start_list(b: &mut Vec<u8>, tag: &[u8; 4]) -> usize {
    b.extend_from_slice(b"LIST");
    let size_at = b.len();
    push_u32(b, 0);
    b.extend_from_slice(tag);
    size_at
}
fn end_list(b: &mut [u8], size_at: usize) {
    let size = (b.len() - size_at - 4) as u32;
    b[size_at..size_at + 4].copy_from_slice(&size.to_le_bytes());
}
fn avi_chunk(file: &mut File, tag: &[u8; 4], bytes: &[u8]) -> Result<()> {
    file.write_all(tag)?;
    file.write_all(&(bytes.len() as u32).to_le_bytes())?;
    file.write_all(bytes)?;
    if bytes.len() % 2 != 0 {
        file.write_all(&[0])?;
    }
    Ok(())
}
fn patch_u32(file: &mut File, position: u64, value: u32) -> Result<()> {
    file.seek(SeekFrom::Start(position))?;
    file.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn draw_slide(
    width: u32,
    height: u32,
    slide: &VideoSlide,
    index: usize,
    total: usize,
    step: usize,
    steps: usize,
    font: &Font<'_>,
    bold: &Font<'_>,
) -> RgbImage {
    let colors = [
        ([18, 39, 67], [22, 68, 103]),
        ([19, 47, 56], [26, 92, 98]),
        ([47, 33, 68], [89, 56, 116]),
        ([58, 40, 48], [111, 61, 63]),
    ];
    let (a, b) = colors[index % colors.len()];
    let mut image = RgbImage::from_fn(width, height, |x, y| {
        let t = (x as f32 / width as f32 * 0.65 + y as f32 / height as f32 * 0.35).clamp(0.0, 1.0);
        Rgb([
            (a[0] as f32 * (1.0 - t) + b[0] as f32 * t) as u8,
            (a[1] as f32 * (1.0 - t) + b[1] as f32 * t) as u8,
            (a[2] as f32 * (1.0 - t) + b[2] as f32 * t) as u8,
        ])
    });
    rect(&mut image, 58, 65, 5, 36, Rgb([100, 218, 205]));
    draw_text(
        &mut image,
        bold,
        "NOTEBOOK  /  VIDEO OVERVIEW",
        72,
        64,
        17.0,
        Rgb([124, 225, 217]),
    );
    let title_lines = wrap(font, &slide.title, 43.0, 820.0);
    let mut y = 137;
    for line in title_lines.into_iter().take(2) {
        draw_text(&mut image, bold, &line, 60, y, 43.0, Rgb([249, 251, 254]));
        y += 58;
    }
    y += 17;
    for bullet in slide.bullets.iter().take(4) {
        let lines = wrap(font, bullet, 23.0, 762.0);
        rect(&mut image, 63, (y + 12) as u32, 7, 7, Rgb([114, 226, 205]));
        for line in lines.into_iter().take(2) {
            draw_text(&mut image, font, &line, 87, y, 23.0, Rgb([224, 236, 245]));
            y += 32;
        }
        y += 9;
        if y > 430 {
            break;
        }
    }
    rect(&mut image, 60, 487, width - 120, 3, Rgb([84, 117, 135]));
    let progress = ((index as f64 + (step + 1) as f64 / steps as f64) / total as f64
        * (width - 120) as f64) as u32;
    rect(&mut image, 60, 487, progress, 3, Rgb([120, 232, 211]));
    draw_text(
        &mut image,
        font,
        &format!("{:02} / {:02}", index + 1, total),
        826,
        505,
        17.0,
        Rgb([182, 206, 223]),
    );
    image
}

fn rect(image: &mut RgbImage, x: u32, y: u32, w: u32, h: u32, color: Rgb<u8>) {
    for cy in y..y.saturating_add(h).min(image.height()) {
        for cx in x..x.saturating_add(w).min(image.width()) {
            image.put_pixel(cx, cy, color);
        }
    }
}

fn text_width(font: &Font<'_>, text: &str, size: f32) -> f32 {
    text.chars()
        .map(|c| {
            font.glyph(c)
                .scaled(Scale::uniform(size))
                .h_metrics()
                .advance_width
        })
        .sum()
}

fn wrap(font: &Font<'_>, text: &str, size: f32, max_width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let next = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if text_width(font, &next, size) > max_width && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
            line = word.to_owned();
        } else {
            line = next;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn draw_text(
    image: &mut RgbImage,
    font: &Font<'_>,
    text: &str,
    x: i32,
    y: i32,
    size: f32,
    color: Rgb<u8>,
) {
    let scale = Scale::uniform(size);
    let baseline = y as f32 + font.v_metrics(scale).ascent;
    for glyph in font.layout(text, scale, point(x as f32, baseline)) {
        if let Some(bounds) = glyph.pixel_bounding_box() {
            glyph.draw(|dx, dy, opacity| {
                let px = bounds.min.x + dx as i32;
                let py = bounds.min.y + dy as i32;
                if px >= 0 && py >= 0 && px < image.width() as i32 && py < image.height() as i32 {
                    let pixel = image.get_pixel_mut(px as u32, py as u32);
                    for c in 0..3 {
                        pixel[c] =
                            (pixel[c] as f32 * (1.0 - opacity) + color[c] as f32 * opacity) as u8;
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_roundtrip_and_splice() {
        let original = Audio {
            rate: 24_000,
            samples: vec![4, 8, 12, -4],
        };
        assert_eq!(
            Audio::from_wav(&original.to_wav().unwrap())
                .unwrap()
                .samples,
            original.samples
        );
        let merged = join_speech(
            &[original.to_wav().unwrap(), original.to_wav().unwrap()],
            100,
        )
        .unwrap();
        assert_eq!(merged.samples.len(), 8 + 2400);
    }

    #[test]
    fn avi_is_riff_with_mjpeg_frames() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.avi");
        let info = write_video(
            &file,
            &[VideoSlide {
                title: "Evidence".into(),
                bullets: vec!["A documented fact".into()],
            }],
            &[None],
        )
        .unwrap();
        let bytes = std::fs::read(file).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"AVI ");
        assert!(bytes.windows(4).any(|w| w == b"MJPG"));
        assert_eq!(info.frames, 8);
        assert_eq!(
            u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize + 8,
            bytes.len()
        );
    }
}
