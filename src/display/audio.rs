//! Android's sound through a chosen Windows output: the engine runs with
//! `-no-audio` and this session plays its `streamAudio` stream.

use super::{AudioNotice, authorized, credentials};
use crate::{
    audio_output::{
        Device,
        windows::{self as output, Fault, Queue, Status},
    },
    locale::Locale,
};
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

/// After failing for this long, the engine's stream is reported as missing.
const STREAM_NOTICE: Duration = Duration::from_secs(5);

/// Why the engine's stream does not arrive.
#[derive(Clone, Debug, PartialEq, Eq)]
enum StreamFault {
    /// The engine never published its authenticated control channel.
    NoChannel,
    /// The engine ended the stream.
    Ended,
    /// The connection or the call failed: the client library's own words.
    Call(String),
}

impl StreamFault {
    fn call(error: impl std::error::Error) -> Self {
        let mut text = error.to_string();
        let mut source = error.source();
        while let Some(inner) = source {
            text = format!("{text}: {inner}");
            source = inner.source();
        }
        Self::Call(text)
    }

    fn status(status: tonic::Status) -> Self {
        Self::Call(format!("{:?}: {}", status.code(), status.message()))
    }

    fn text(&self, locale: Locale) -> String {
        match self {
            Self::NoChannel => locale
                .pick(
                    "o motor não anunciou o canal de áudio autenticado",
                    "the engine did not announce its authenticated audio channel",
                    "el motor no anunció el canal de audio autenticado",
                )
                .into(),
            Self::Ended => locale
                .pick(
                    "o motor encerrou o fluxo de áudio",
                    "the engine closed the audio stream",
                    "el motor cerró el flujo de audio",
                )
                .into(),
            Self::Call(text) => text.clone(),
        }
    }
}

/// The engine's stream as the user hears about it: a failure that lasts is
/// told once, and so is its end.
#[derive(Default)]
struct Link {
    /// The first failure since the stream last opened.
    failing_since: Option<Instant>,
    told: bool,
}

impl Link {
    /// An attempt failed. True when the user is to be told now. One failed
    /// attempt while the engine starts or closes is not worth a notice; a
    /// channel that was never announced has already been awaited for long.
    fn failed(&mut self, fault: &StreamFault, now: Instant) -> bool {
        let since = *self.failing_since.get_or_insert(now);
        let lasting =
            *fault == StreamFault::NoChannel || now.duration_since(since) >= STREAM_NOTICE;
        let tell = lasting && !self.told;
        self.told |= tell;
        tell
    }

    /// The stream opened. True when the user had been told that it was missing.
    fn opened(&mut self) -> bool {
        self.failing_since = None;
        std::mem::take(&mut self.told)
    }
}

enum Event {
    Output(output::Notice),
    StreamLost(StreamFault),
    StreamBack,
}

#[derive(Default)]
struct Shared {
    /// Where the player's sound goes; `None` until the player reports.
    output: Option<Status>,
    /// Why the engine's stream is missing, while the user has been told so.
    stream: Option<StreamFault>,
    /// Changes the window has not shown yet.
    events: Vec<Event>,
}

/// Stops both threads when dropped with the display.
pub(super) struct Audio {
    stop: Arc<AtomicBool>,
    /// The chosen device as the settings name it.
    name: String,
    shared: Arc<Mutex<Shared>>,
}

impl Drop for Audio {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl Audio {
    /// One line on where this session's sound is right now.
    pub(super) fn status(&self, locale: Locale) -> String {
        let shared = self.shared.lock().unwrap();
        status_text(
            shared.output.as_ref(),
            shared.stream.as_ref(),
            &self.name,
            locale,
        )
    }

    /// What changed since the last call, in the order it happened.
    pub(super) fn take_notices(&self, locale: Locale) -> Vec<AudioNotice> {
        let events = std::mem::take(&mut self.shared.lock().unwrap().events);
        events
            .iter()
            .map(|event| notice(event, &self.name, locale))
            .collect()
    }
}

fn not_playing(locale: Locale) -> &'static str {
    locale.pick(
        "O som do Android não está tocando",
        "Android sound is not playing",
        "El sonido de Android no se está reproduciendo",
    )
}

/// Why nothing plays, as a sentence without its final period.
fn no_output(fault: &Fault, locale: Locale) -> String {
    format!(
        "{} ({})",
        locale.pick(
            "Nenhuma saída de áudio pôde ser aberta",
            "No audio output could be opened",
            "No se ha podido abrir ninguna salida de audio",
        ),
        fault.text(locale)
    )
}

fn no_stream(fault: &StreamFault, locale: Locale) -> String {
    format!(
        "{} ({})",
        locale.pick(
            "O motor não está entregando o áudio à janela",
            "The engine is not delivering its audio to the window",
            "El motor no está entregando el audio a la ventana",
        ),
        fault.text(locale)
    )
}

fn unavailable(name: &str, fault: &Fault, locale: Locale) -> String {
    format!(
        "“{name}” {} ({})",
        locale.pick("está indisponível", "is unavailable", "no está disponible"),
        fault.text(locale)
    )
}

fn status_text(
    output: Option<&Status>,
    stream: Option<&StreamFault>,
    name: &str,
    locale: Locale,
) -> String {
    let now = locale.pick("Nesta sessão:", "This session:", "En esta sesión:");
    let silent = |why: String| format!("{now} {}. {why}.", not_playing(locale));
    match (stream, output) {
        (Some(fault), _) => silent(no_stream(fault, locale)),
        (None, Some(Status::Silent(fault))) => silent(no_output(fault, locale)),
        (None, Some(Status::Default(fault))) => format!(
            "{now} {} {}",
            locale.pick(
                "O som do Android toca na saída padrão do sistema.",
                "Android sound plays on the system default output.",
                "El sonido de Android se reproduce en la salida predeterminada del sistema.",
            ),
            unavailable(name, fault, locale) + "."
        ),
        (None, Some(Status::Chosen)) => format!(
            "{now} {} “{name}”.",
            locale.pick(
                "O som do Android toca em",
                "Android sound plays on",
                "El sonido de Android se reproduce en",
            )
        ),
        (None, Some(Status::Opening) | None) => format!(
            "{now} {}",
            locale.pick(
                "Abrindo a saída de áudio…",
                "Opening the audio output…",
                "Abriendo la salida de audio…",
            )
        ),
    }
}

fn notice(event: &Event, name: &str, locale: Locale) -> AudioNotice {
    // What gives the sound back for certain: the engine's own output.
    let remedy = locale.pick(
        "Para voltar ao som do próprio motor, escolha “Padrão do sistema” em Configurações → Som → Saída de áudio e reinicie o Android.",
        "To go back to the engine's own sound, select “System default” in Settings → Sound → Audio output and restart Android.",
        "Para volver al sonido del propio motor, elige “Predeterminada del sistema” en Ajustes → Sonido → Salida de audio y reinicia Android.",
    );
    let silent = |why: String| AudioNotice {
        text: format!("{}.", not_playing(locale)),
        details: Some(format!("{why}. {remedy}")),
        problem: true,
    };
    match event {
        Event::Output(output::Notice::Silent(fault)) => silent(no_output(fault, locale)),
        Event::StreamLost(fault) => silent(no_stream(fault, locale)),
        Event::Output(output::Notice::Fallback(fault)) => AudioNotice {
            text: locale
                .pick(
                    "O som do Android passou para a saída padrão do sistema.",
                    "Android sound moved to the system default output.",
                    "El sonido de Android ha pasado a la salida predeterminada del sistema.",
                )
                .into(),
            details: Some(format!(
                "{}. {}",
                unavailable(name, fault, locale),
                locale.pick(
                    "O som volta para esse dispositivo quando ele estiver disponível.",
                    "The sound returns to that device when it is available again.",
                    "El sonido volverá a ese dispositivo cuando esté disponible.",
                )
            )),
            problem: true,
        },
        Event::Output(output::Notice::Returned) => AudioNotice {
            text: format!(
                "{} “{name}”.",
                locale.pick(
                    "O som do Android voltou para",
                    "Android sound is back on",
                    "El sonido de Android ha vuelto a",
                )
            ),
            details: None,
            problem: false,
        },
        Event::StreamBack => AudioNotice {
            text: locale
                .pick(
                    "O som do Android voltou a tocar.",
                    "Android sound is playing again.",
                    "El sonido de Android vuelve a reproducirse.",
                )
                .into(),
            details: None,
            problem: false,
        },
    }
}

pub(super) fn start(console: u16, rpc: u16, avd: PathBuf, device: Device) -> Audio {
    let stop = Arc::new(AtomicBool::new(false));
    let shared = Arc::new(Mutex::new(Shared::default()));
    let queue = Arc::new(Queue::default());
    {
        let (stop, queue, shared) = (stop.clone(), queue.clone(), shared.clone());
        std::thread::spawn(move || {
            output::play(device.id, queue, stop, |status, notice| {
                eprintln!("RustEmuDroid audio: {status:?}");
                let mut shared = shared.lock().unwrap();
                shared.output = Some(status);
                shared.events.extend(notice.map(Event::Output));
            })
        });
    }
    {
        let (stop, shared) = (stop.clone(), shared.clone());
        std::thread::spawn(move || {
            let lost = |fault: StreamFault| {
                let mut shared = shared.lock().unwrap();
                shared.stream = Some(fault.clone());
                shared.events.push(Event::StreamLost(fault));
            };
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => return lost(StreamFault::call(error)),
            };
            runtime.block_on(async {
                let mut link = Link::default();
                while !stop.load(Ordering::Acquire) {
                    let opened = || {
                        let back = link.opened();
                        let mut shared = shared.lock().unwrap();
                        shared.stream = None;
                        if back {
                            shared.events.push(Event::StreamBack);
                        }
                    };
                    let Err(fault) = receive(console, rpc, &avd, &queue, &stop, opened).await
                    else {
                        continue;
                    };
                    // The engine may still be starting or closing; try again,
                    // and tell the user if it lasts.
                    eprintln!("RustEmuDroid audio stream: {fault:?}");
                    if link.failed(&fault, Instant::now()) {
                        lost(fault);
                    } else if link.told {
                        shared.lock().unwrap().stream = Some(fault);
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            });
        });
    }
    Audio {
        stop,
        name: device.name,
        shared,
    }
}

async fn receive(
    console: u16,
    rpc: u16,
    avd: &std::path::Path,
    queue: &Queue,
    stop: &AtomicBool,
    opened: impl FnOnce(),
) -> Result<(), StreamFault> {
    let deadline = Instant::now() + Duration::from_secs(90);
    let token = loop {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Some(token) = credentials(console, rpc, avd) {
            break token;
        }
        if Instant::now() >= deadline {
            return Err(StreamFault::NoChannel);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    let channel = Endpoint::from_shared(format!("http://127.0.0.1:{rpc}"))
        .map_err(StreamFault::call)?
        .connect_timeout(Duration::from_secs(5))
        .connect()
        .await
        .map_err(StreamFault::call)?;
    let mut client = Grpc::new(channel);
    client.ready().await.map_err(StreamFault::call)?;
    let mut stream = client
        .server_streaming(
            authorized(request(), &token),
            PathAndQuery::from_static("/android.emulation.control.EmulatorController/streamAudio"),
            ProstCodec::<AudioFormat, AudioPacket>::default(),
        )
        .await
        .map_err(StreamFault::status)?
        .into_inner();
    opened();
    // QA only: packet age on arrival (the engine stamps it when the packet is full).
    let stats = std::env::var_os("RUSTEMUDROID_AUDIO_STATS").is_some();
    let (mut ages, mut since) = (Vec::new(), Instant::now());
    // The engine sends nothing while Android's output is idle; wake up to notice a stop.
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
            Ok(Ok(None)) => return Err(StreamFault::Ended),
            Ok(Err(status)) => return Err(StreamFault::status(status)),
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

    #[test]
    fn a_stream_failure_is_told_once_it_lasts_and_so_is_its_end() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let refused = StreamFault::Call("transport error".into());
        let mut link = Link::default();
        // One failed attempt while the engine starts or closes says nothing.
        assert!(!link.failed(&refused, at(0)));
        assert!(!link.opened());
        // A failure that lasts five seconds is told, once.
        assert!(!link.failed(&StreamFault::Ended, at(10_000)));
        assert!(!link.failed(&refused, at(14_999)));
        assert!(link.failed(&refused, at(15_000)));
        assert!(!link.failed(&refused, at(16_000)));
        assert!(link.told);
        // So is the stream coming back, and a later failure counts from its start.
        assert!(link.opened());
        assert!(!link.opened());
        assert!(!link.failed(&refused, at(60_000)));
        assert!(!link.opened());
        // A channel the engine never announced has been awaited for 90 seconds.
        assert!(link.failed(&StreamFault::NoChannel, at(200_000)));
    }

    #[test]
    fn the_status_line_says_where_the_sound_really_is() {
        let text = |output: Option<Status>, stream: Option<StreamFault>| {
            status_text(output.as_ref(), stream.as_ref(), "Headphones", Locale::EnUs)
        };
        assert_eq!(text(None, None), "This session: Opening the audio output…");
        assert_eq!(
            text(Some(Status::Chosen), None),
            "This session: Android sound plays on “Headphones”."
        );
        assert_eq!(
            text(Some(Status::Default(Fault::Gone)), None),
            "This session: Android sound plays on the system default output. “Headphones” is unavailable (device disconnected or disabled)."
        );
        assert_eq!(
            text(Some(Status::Silent(Fault::Busy)), None),
            "This session: Android sound is not playing. No audio output could be opened (device in exclusive use by another program)."
        );
        // A missing stream is silence whatever the device is doing.
        assert_eq!(
            text(Some(Status::Chosen), Some(StreamFault::Ended)),
            "This session: Android sound is not playing. The engine is not delivering its audio to the window (the engine closed the audio stream)."
        );
        for locale in [Locale::PtBr, Locale::EsEs] {
            let status = Some(Status::Default(Fault::Gone));
            let translated = status_text(status.as_ref(), None, "Headphones", locale);
            assert!(translated.contains("“Headphones”") && !translated.contains("Android sound"));
        }
    }

    #[test]
    fn a_silent_notice_gives_the_reason_and_the_way_back_in_three_languages() {
        let lost = Event::StreamLost(StreamFault::NoChannel);
        let silent = Event::Output(output::Notice::Silent(Fault::Gone));
        let english = notice(&lost, "Headphones", Locale::EnUs);
        assert!(english.problem);
        assert_eq!(english.text, "Android sound is not playing.");
        assert_eq!(
            english.details.as_deref(),
            Some(
                "The engine is not delivering its audio to the window (the engine did not announce its authenticated audio channel). To go back to the engine's own sound, select “System default” in Settings → Sound → Audio output and restart Android."
            )
        );
        for (locale, default, settings) in [
            (
                Locale::PtBr,
                "“Padrão do sistema”",
                "Configurações → Som → Saída de áudio",
            ),
            (
                Locale::EnUs,
                "“System default”",
                "Settings → Sound → Audio output",
            ),
            (
                Locale::EsEs,
                "“Predeterminada del sistema”",
                "Ajustes → Sonido → Salida de audio",
            ),
        ] {
            for event in [&lost, &silent] {
                let notice = notice(event, "Headphones", locale);
                let details = notice.details.unwrap();
                assert!(notice.problem && details.contains(default) && details.contains(settings));
                assert!(details.contains("Android"), "restart Android: {details}");
            }
            assert!(
                notice(&silent, "x", locale)
                    .details
                    .unwrap()
                    .contains(&Fault::Gone.text(locale))
            );
        }
    }

    #[test]
    fn moving_to_the_default_and_back_is_said_with_the_device_name() {
        let fallback = notice(
            &Event::Output(output::Notice::Fallback(Fault::Busy)),
            "Headphones",
            Locale::EnUs,
        );
        assert!(fallback.problem);
        assert_eq!(
            fallback.text,
            "Android sound moved to the system default output."
        );
        assert_eq!(
            fallback.details.as_deref(),
            Some(
                "“Headphones” is unavailable (device in exclusive use by another program). The sound returns to that device when it is available again."
            )
        );
        let back = notice(
            &Event::Output(output::Notice::Returned),
            "Headphones",
            Locale::PtBr,
        );
        assert_eq!(
            (back.text.as_str(), back.details, back.problem),
            ("O som do Android voltou para “Headphones”.", None, false)
        );
        let playing = notice(&Event::StreamBack, "Headphones", Locale::EsEs);
        assert_eq!(playing.text, "El sonido de Android vuelve a reproducirse.");
        assert!(!playing.problem);
    }
}
