//! Android's sound through a chosen Windows output: the engine runs with
//! `-no-audio` and this session plays its `streamAudio` gRPC stream.

use super::{DisplayResult, authorized, credentials};
use crate::audio_output::windows::{self as output, Queue};
use prost::Message;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tonic::{
    client::Grpc, codec::ProstCodec, codegen::http::uri::PathAndQuery, transport::Endpoint,
};

#[derive(Clone, PartialEq, Message)]
struct AudioFormat {
    #[prost(uint64, tag = "1")]
    sampling_rate: u64,
    /// Mono = 0, Stereo = 1.
    #[prost(int32, tag = "2")]
    channels: i32,
    /// AUD_FMT_U8 = 0, AUD_FMT_S16 = 1.
    #[prost(int32, tag = "3")]
    format: i32,
}

#[derive(Clone, PartialEq, Message)]
struct AudioPacket {
    #[prost(uint64, tag = "2")]
    timestamp: u64,
    #[prost(bytes = "vec", tag = "3")]
    audio: Vec<u8>,
}

fn request() -> AudioFormat {
    AudioFormat {
        sampling_rate: output::RATE as u64,
        channels: (output::CHANNELS == 2) as i32,
        format: 1,
    }
}

/// Stops both threads when dropped with the display.
pub(super) struct Audio {
    stop: Arc<AtomicBool>,
    /// Set when the chosen output went away and the sound moved to the default.
    pub(super) notice: Arc<Mutex<Option<String>>>,
}

impl Drop for Audio {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

pub(super) fn start(console: u16, rpc: u16, avd: PathBuf, endpoint: String) -> Audio {
    let stop = Arc::new(AtomicBool::new(false));
    let notice = Arc::new(Mutex::new(None));
    let queue = Arc::new(Queue::default());
    {
        let (stop, queue, notice) = (stop.clone(), queue.clone(), notice.clone());
        std::thread::spawn(move || {
            output::play(endpoint, queue, stop, |error| {
                eprintln!("RustEmuDroid audio: {error}");
                *notice.lock().unwrap() = Some(error.to_owned());
            })
        });
    }
    {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            runtime.block_on(async {
                while !stop.load(Ordering::Acquire) {
                    if let Err(error) = receive(console, rpc, &avd, &queue, &stop).await {
                        // The engine may still be starting or closing; try again.
                        eprintln!("RustEmuDroid audio stream: {error}");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            });
        });
    }
    Audio { stop, notice }
}

async fn receive(
    console: u16,
    rpc: u16,
    avd: &std::path::Path,
    queue: &Queue,
    stop: &AtomicBool,
) -> DisplayResult<()> {
    let deadline = Instant::now() + Duration::from_secs(90);
    let token = loop {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Some(token) = credentials(console, rpc, avd) {
            break token;
        }
        if Instant::now() >= deadline {
            return Err("O motor não anunciou o canal de áudio autenticado.".into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    let channel = Endpoint::from_shared(format!("http://127.0.0.1:{rpc}"))?
        .connect_timeout(Duration::from_secs(5))
        .connect()
        .await?;
    let mut client = Grpc::new(channel);
    client.ready().await?;
    let mut stream = client
        .server_streaming(
            authorized(request(), &token),
            PathAndQuery::from_static("/android.emulation.control.EmulatorController/streamAudio"),
            ProstCodec::<AudioFormat, AudioPacket>::default(),
        )
        .await?
        .into_inner();
    // QA only: packet age on arrival (the engine stamps it when the packet is full).
    let stats = std::env::var_os("RUSTEMUDROID_AUDIO_STATS").is_some();
    let (mut ages, mut since) = (Vec::new(), Instant::now());
    // The engine sends nothing while Android is silent; wake up to notice a stop.
    while !stop.load(Ordering::Acquire) {
        match tokio::time::timeout(Duration::from_millis(200), stream.message()).await {
            Err(_) => continue,
            Ok(Ok(Some(packet))) => {
                queue.push(&packet.audio);
                if stats {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_micros() as u64);
                    ages.push(now.saturating_sub(packet.timestamp) as f64 / 1000.);
                    if since.elapsed() >= Duration::from_secs(1) {
                        let max = ages.iter().copied().fold(0., f64::max);
                        let avg = ages.iter().sum::<f64>() / ages.len() as f64;
                        eprintln!(
                            "RustEmuDroid audio stats: {} packets, {} bytes last, age ms avg {avg:.1} max {max:.1}",
                            ages.len(),
                            packet.audio.len()
                        );
                        ages.clear();
                        since = Instant::now();
                    }
                }
            }
            Ok(Ok(None)) => return Err("streamAudio ended".into()),
            Ok(Err(status)) => return Err(status.into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_the_engine_for_48_khz_stereo_s16() {
        let bytes = request().encode_to_vec();
        // samplingRate = 48000 (field 1, varint), channels = Stereo, format = S16.
        assert_eq!(bytes, [0x08, 0x80, 0xf7, 0x02, 0x10, 0x01, 0x18, 0x01]);
        let packet = AudioPacket {
            timestamp: 7,
            audio: vec![1, 0, 2, 0],
        };
        let decoded = AudioPacket::decode(packet.encode_to_vec().as_slice()).unwrap();
        assert_eq!(decoded.audio, [1, 0, 2, 0]);
    }
}
