use anyhow::{Context, Result};
use std::{fs::File, path::Path};
use symphonia::core::{
    audio::SampleBuffer, codecs::DecoderOptions, formats::FormatOptions, io::MediaSourceStream,
    meta::MetadataOptions, probe::Hint,
};

pub struct PcmFrames {
    pub data: Vec<i16>, // interleaved mono
    pub sample_rate: u32,
}

// Decode an MP3 file fully to 8 kHz mono PCM with simple linear resample.
pub fn decode_mp3_to_pcm_8k<P: AsRef<Path>>(path: P) -> Result<PcmFrames> {
    let file = File::open(&path)
        .with_context(|| format!("open audio file: {}", path.as_ref().display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("mp3");
    let fmt_opts = FormatOptions::default();
    let meta_opts = MetadataOptions::default();
    let probed = symphonia::default::get_probe().format(&hint, mss, &fmt_opts, &meta_opts)?;
    let mut format = probed.format;
    let track = format.default_track().context("no default track")?;
    let dec_opts = DecoderOptions::default();
    let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &dec_opts)?;
    let src_rate = track.codec_params.sample_rate.unwrap_or(48000) as usize;
    let channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(1);
    let mut pcm: Vec<i16> = Vec::new();

    while let Ok(p) = format.next_packet() {
        let packet = p;
        let decoded = decoder.decode(&packet)?;
        let mut buf = SampleBuffer::<i16>::new(decoded.capacity() as u64, *decoded.spec());
        buf.copy_interleaved_ref(decoded);
        // Downmix to mono
        if channels > 1 {
            for frame in buf.samples().chunks_exact(channels) {
                let sum: i32 = frame.iter().map(|&s| s as i32).sum();
                pcm.push((sum / channels as i32) as i16);
            }
        } else {
            pcm.extend_from_slice(buf.samples());
        }
    }

    // Resample to 8kHz using simple linear interpolation
    let dst_rate = 8000usize;
    if src_rate == dst_rate {
        return Ok(PcmFrames {
            data: pcm,
            sample_rate: 8000,
        });
    }
    let factor = dst_rate as f64 / src_rate as f64;
    let out_len = (pcm.len() as f64 * factor).floor() as usize;
    let mut out = vec![0i16; out_len];
    for (i, slot) in out.iter_mut().enumerate().take(out_len) {
        let src_pos = i as f64 / factor;
        let idx = src_pos.floor() as usize;
        let frac = src_pos - idx as f64;
        let a = pcm.get(idx).copied().unwrap_or(0) as f64;
        let b = pcm.get(idx + 1).copied().unwrap_or(a as i16) as f64;
        let v = a + (b - a) * frac;
        *slot = v.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16;
    }
    Ok(PcmFrames {
        data: out,
        sample_rate: 8000,
    })
}

pub fn split_into_20ms_frames(pcm: &[i16], sample_rate: u32) -> Vec<Vec<i16>> {
    let samples_per_20ms = (sample_rate as usize / 50).max(1);
    pcm.chunks(samples_per_20ms).map(|c| c.to_vec()).collect()
}
