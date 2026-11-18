use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::util::err;

pub struct AudioCapture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<(), String>>>,
    wav: PathBuf,
}

impl AudioCapture {
    pub fn start(kind: &str, wav: PathBuf) -> Result<Self, String> {
        if kind != "mic" && kind != "speaker" {
            return Err("未知的声音来源".into());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let kind = kind.to_string();
        let kind_thread = kind.clone();
        let wav_thread = wav.clone();
        let (tx, rx) = mpsc::channel();
        let thread = thread::spawn(move || capture_loop(&kind_thread, &wav_thread, &stop_thread, &tx));
        match rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(())) => Ok(Self { stop, thread: Some(thread), wav }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                let _ = thread.join();
                Err(kind_error(&kind, "打开音频设备超时"))
            }
        }
    }

    pub fn finish(mut self) -> Result<PathBuf, String> {
        self.stop.store(true, Ordering::Relaxed);
        let thread = self.thread.take().ok_or("音频线程已经结束")?;
        thread.join().map_err(|_| "音频线程异常退出".to_string())??;
        if !self.wav.exists() {
            return Err("没有采到声音".into());
        }
        Ok(self.wav)
    }
}

fn kind_error(kind: &str, detail: &str) -> String {
    let name = if kind == "speaker" { "扬声器" } else { "麦克风" };
    format!("{name}打开失败：{detail}")
}

fn capture_loop(kind: &str, wav: &Path, stop: &AtomicBool, ready: &mpsc::Sender<Result<(), String>>) -> Result<(), String> {
    #[cfg(windows)]
    {
        windows_capture(kind, wav, stop, ready)
    }
    #[cfg(not(windows))]
    {
        let _ = (wav, stop);
        let error = kind_error(kind, "当前系统不能录制声音");
        let _ = ready.send(Err(error.clone()));
        Err(error)
    }
}

#[cfg(windows)]
fn windows_capture(kind: &str, wav: &Path, stop: &AtomicBool, ready: &mpsc::Sender<Result<(), String>>) -> Result<(), String> {
    let com = match ComGuard::enter() {
        Ok(guard) => guard,
        Err(error) => {
            let error = kind_error(kind, &error);
            let _ = ready.send(Err(error.clone()));
            return Err(error);
        }
    };
    let opened = match open_session(kind) {
        Ok(session) => session,
        Err(error) => {
            let error = kind_error(kind, &error);
            let _ = ready.send(Err(error.clone()));
            drop(com);
            return Err(error);
        }
    };
    if let Err(error) = opened.start() {
        let error = kind_error(kind, &error);
        let _ = ready.send(Err(error.clone()));
        drop(opened);
        drop(com);
        return Err(error);
    }
    let _ = ready.send(Ok(()));
    let recorded = opened.pump(stop);
    let result = match recorded {
        Ok(pcm) if pcm.is_empty() => Err("没有采到声音".into()),
        Ok(pcm) => write_wav(wav, opened.channels, opened.rate, opened.bits, opened.tag, &pcm),
        Err(error) => Err(error),
    };
    drop(opened);
    drop(com);
    result
}

#[cfg(windows)]
struct ComGuard {
    uninit: bool,
}

#[cfg(windows)]
impl ComGuard {
    fn enter() -> Result<Self, String> {
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_err() {
            return Err(format!("COM {hr:?}"));
        }
        Ok(Self { uninit: true })
    }
}

#[cfg(windows)]
impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.uninit {
            unsafe { windows::Win32::System::Com::CoUninitialize() };
        }
    }
}

#[cfg(windows)]
struct Session {
    client: windows::Win32::Media::Audio::IAudioClient,
    capture: windows::Win32::Media::Audio::IAudioCaptureClient,
    channels: u16,
    rate: u32,
    bits: u16,
    tag: u16,
    align: u16,
}

#[cfg(windows)]
impl Session {
    fn start(&self) -> Result<(), String> {
        unsafe { self.client.Start().map_err(err) }
    }

    fn pump(&self, stop: &AtomicBool) -> Result<Vec<u8>, String> {
        let mut pcm = Vec::new();
        let started = std::time::Instant::now();
        self.read(stop, &mut pcm, started)?;
        Ok(pcm)
    }

    fn read(&self, stop: &AtomicBool, pcm: &mut Vec<u8>, started: std::time::Instant) -> Result<(), String> {
        use windows::Win32::Media::Audio::AUDCLNT_BUFFERFLAGS_SILENT;
        const MAX_BYTES: usize = 32 * 1024 * 1024;
        while !stop.load(Ordering::Relaxed) && started.elapsed() < Duration::from_secs(20) && pcm.len() < MAX_BYTES {
            let packet = unsafe { self.capture.GetNextPacketSize().map_err(err)? };
            if packet == 0 {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            let mut data = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            unsafe { self.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None).map_err(err)? };
            let nbytes = frames as usize * self.align as usize;
            if nbytes > 0 && pcm.len() + nbytes <= MAX_BYTES {
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    pcm.resize(pcm.len() + nbytes, 0);
                } else {
                    let slice = unsafe { std::slice::from_raw_parts(data, nbytes) };
                    pcm.extend_from_slice(slice);
                }
            }
            unsafe { self.capture.ReleaseBuffer(frames).map_err(err)? };
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for Session {
    fn drop(&mut self) {
        unsafe { let _ = self.client.Stop(); }
    }
}

#[cfg(windows)]
fn open_session(kind: &str) -> Result<Session, String> {
    use windows::core::GUID;
    use windows::Win32::Media::Audio::{
        eCapture, eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEX,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL};

    const MMDEVICE: GUID = GUID::from_u128(0xBCDE0395_E52F_467C_8E3D_C4579291692E);
    const PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
    const FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

    let enumerator: IMMDeviceEnumerator = unsafe { CoCreateInstance(&MMDEVICE, None, CLSCTX_ALL).map_err(err)? };
    let flow = if kind == "speaker" { eRender } else { eCapture };
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(flow, eConsole).map_err(err)? };
    let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None).map_err(err)? };
    let format_ptr = unsafe { client.GetMixFormat().map_err(err)? };
    let raw = copy_mix_format(format_ptr);
    unsafe { CoTaskMemFree(Some(format_ptr as *const _)) };
    let raw = raw?;
    let wave = read_wave(&raw)?;
    let tag = simple_tag(&raw, wave.wFormatTag, &PCM, &FLOAT)?;
    let bits = wave.wBitsPerSample;
    let channels = wave.nChannels;
    let rate = wave.nSamplesPerSec;
    let align = wave.nBlockAlign;
    if channels == 0 || rate < 8000 || rate > 192000 || align == 0 {
        return Err("音频格式不可用".into());
    }
    if tag == 1 && bits != 16 && bits != 32 {
        return Err(format!("不支持 {bits} 位 PCM"));
    }
    if tag == 3 && bits != 32 {
        return Err(format!("不支持 {bits} 位浮点音频"));
    }
    let expected = channels as u32 * bits as u32 / 8;
    if align as u32 != expected {
        return Err("音频帧长度不匹配".into());
    }
    let flags = if kind == "speaker" { AUDCLNT_STREAMFLAGS_LOOPBACK } else { 0 };
    unsafe {
        client
            .Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, raw.as_ptr() as *const WAVEFORMATEX, None)
            .map_err(err)?;
    }
    let capture: IAudioCaptureClient = unsafe { client.GetService().map_err(err)? };
    Ok(Session { client, capture, channels, rate, bits, tag, align })
}

#[cfg(windows)]
fn copy_mix_format(ptr: *mut windows::Win32::Media::Audio::WAVEFORMATEX) -> Result<Vec<u8>, String> {
    if ptr.is_null() {
        return Err("没有音频格式".into());
    }
    let wave = unsafe { std::ptr::read_unaligned(ptr) };
    let extra = (wave.cbSize as usize).min(512);
    let bytes = 18 + extra;
    let raw = unsafe { std::slice::from_raw_parts(ptr as *const u8, bytes).to_vec() };
    Ok(raw)
}

#[cfg(windows)]
fn read_wave(raw: &[u8]) -> Result<windows::Win32::Media::Audio::WAVEFORMATEX, String> {
    if raw.len() < 18 {
        return Err("音频格式不完整".into());
    }
    Ok(unsafe { std::ptr::read_unaligned(raw.as_ptr() as *const windows::Win32::Media::Audio::WAVEFORMATEX) })
}

#[cfg(windows)]
fn simple_tag(raw: &[u8], tag: u16, pcm: &windows::core::GUID, float_pcm: &windows::core::GUID) -> Result<u16, String> {
    if tag == 1 || tag == 3 {
        return Ok(tag);
    }
    if tag == 0xFFFE {
        if raw.len() < 40 {
            return Err("音频格式不完整".into());
        }
        let guid = unsafe { std::ptr::read_unaligned(raw.as_ptr().add(24) as *const windows::core::GUID) };
        if guid == *pcm {
            return Ok(1);
        }
        if guid == *float_pcm {
            return Ok(3);
        }
    }
    Err("不支持的音频格式".into())
}

pub fn write_wav(path: &Path, channels: u16, rate: u32, bits: u16, tag: u16, pcm: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(err)?;
    }
    let block = channels as u32 * bits as u32 / 8;
    if block == 0 {
        return Err("音频帧长度无效".into());
    }
    let average = rate * block;
    let data_len = pcm.len() as u32;
    let mut header = Vec::with_capacity(44);
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&(36 + data_len).to_le_bytes());
    header.extend_from_slice(b"WAVEfmt ");
    header.extend_from_slice(&16u32.to_le_bytes());
    header.extend_from_slice(&tag.to_le_bytes());
    header.extend_from_slice(&channels.to_le_bytes());
    header.extend_from_slice(&rate.to_le_bytes());
    header.extend_from_slice(&average.to_le_bytes());
    header.extend_from_slice(&(block as u16).to_le_bytes());
    header.extend_from_slice(&bits.to_le_bytes());
    header.extend_from_slice(b"data");
    header.extend_from_slice(&data_len.to_le_bytes());
    let mut file = std::fs::File::create(path).map_err(err)?;
    use std::io::Write;
    file.write_all(&header).map_err(err)?;
    file.write_all(pcm).map_err(err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::write_wav;
    use std::path::PathBuf;

    #[test]
    fn wav_header_is_pcm_stereo() {
        let path = std::env::temp_dir().join(format!("shotlet-wav-test-{}.wav", std::process::id()));
        write_wav(&path, 2, 48000, 16, 1, &[0, 1, 2, 3]).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes([bytes[20], bytes[21]]), 1);
        assert_eq!(u16::from_le_bytes([bytes[22], bytes[23]]), 2);
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 48000);
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(bytes.len(), 48);
    }

    #[test]
    #[ignore = "opens the default microphone and speaker for under a second"]
    fn default_endpoints_produce_a_wav() {
        let dir = std::env::temp_dir();
        for kind in ["mic", "speaker"] {
            let path: PathBuf = dir.join(format!("shotlet-audio-{kind}-{}.wav", std::process::id()));
            let _ = std::fs::remove_file(&path);
            let capture = super::AudioCapture::start(kind, path.clone()).unwrap_or_else(|error| panic!("{kind}: {error}"));
            std::thread::sleep(std::time::Duration::from_millis(400));
            let written = capture.finish().unwrap_or_else(|error| panic!("{kind}: {error}"));
            let bytes = std::fs::read(&written).unwrap_or_else(|error| panic!("{kind}: {error}"));
            let _ = std::fs::remove_file(&written);
            assert!(bytes.len() > 44, "{kind} only wrote {} bytes", bytes.len());
            assert_eq!(&bytes[0..4], b"RIFF");
        }
    }
}
