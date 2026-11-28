//! The engine's control API as the panel uses it: the hand-written
//! message types, the authenticated channel and the mapping of a point on the
//! panel to the engine's touch coordinates.
use super::DisplayResult;
use prost::Message;
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use tonic::{
    Request,
    metadata::{Ascii, MetadataValue},
    transport::{Channel, Endpoint},
};

#[derive(Clone, PartialEq, Message)]
pub(crate) struct Format {
    #[prost(int32, tag = "1")]
    pub(crate) format: i32,
    #[prost(message, optional, tag = "2")]
    pub(crate) rotation: Option<Rotation>,
    #[prost(uint32, tag = "3")]
    pub(crate) width: u32,
    #[prost(uint32, tag = "4")]
    pub(crate) height: u32,
    #[prost(uint32, tag = "5")]
    pub(crate) display: u32,
    // Folded geometry and a resizable mode disqualify the display from reduction.
    #[prost(message, optional, tag = "7")]
    pub(crate) folded_display: Option<Empty>,
    #[prost(int32, optional, tag = "8")]
    pub(crate) display_mode: Option<i32>,
    // Requests and reports where the pixels travel (inline or a shared file).
    #[prost(message, optional, tag = "6")]
    pub(crate) transport: Option<Transport>,
}
#[derive(Clone, PartialEq, Message)]
pub(crate) struct Transport {
    #[prost(int32, tag = "1")]
    pub(crate) channel: i32,
    #[prost(string, tag = "2")]
    pub(crate) handle: String,
}
#[derive(Clone, PartialEq, Message)]
pub(crate) struct Rotation {
    #[prost(int32, tag = "1")]
    pub(crate) value: i32,
}
#[derive(Clone, PartialEq, Message)]
pub(crate) struct Frame {
    #[prost(message, optional, tag = "1")]
    pub(crate) format: Option<Format>,
    #[prost(bytes = "bytes", tag = "4")]
    pub(crate) pixels: prost::bytes::Bytes,
}
#[derive(Clone, PartialEq, Message)]
pub(crate) struct Touch {
    #[prost(int32, tag = "1")]
    pub(crate) x: i32,
    #[prost(int32, tag = "2")]
    pub(crate) y: i32,
    #[prost(int32, tag = "3")]
    pub(crate) identifier: i32,
    #[prost(int32, tag = "4")]
    pub(crate) pressure: i32,
}
#[derive(Clone, PartialEq, Message)]
pub(crate) struct TouchEvent {
    #[prost(message, repeated, tag = "1")]
    pub(crate) touches: Vec<Touch>,
}
#[derive(Clone, PartialEq, Message)]
pub(crate) struct Keyboard {
    #[prost(int32, tag = "2")]
    pub(crate) event: i32,
    #[prost(string, tag = "4")]
    pub(crate) key: String,
}
#[derive(Clone, PartialEq, Message)]
pub(crate) struct Empty {}

pub(crate) fn credentials(console: u16, rpc: u16, avd: &Path) -> Option<MetadataValue<Ascii>> {
    let roots = if cfg!(windows) {
        vec![PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Temp")]
    } else {
        let mut roots = vec![avd.parent()?.parent()?.join("home")];
        roots.extend(std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from));
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::MetadataExt;
            if let Ok(info) = fs::metadata("/proc/self") {
                roots.push(PathBuf::from(format!("/run/user/{}", info.uid())));
            }
        }
        roots
    };
    for entry in roots
        .iter()
        .filter_map(|r| fs::read_dir(r.join("avd/running")).ok())
        .flatten()
        .flatten()
    {
        if entry.path().extension().and_then(|e| e.to_str()) != Some("ini") {
            continue;
        }
        let Ok(data) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let value = |k: &str| {
            data.lines()
                .filter_map(|l| l.split_once('='))
                .find(|(name, _)| name.trim() == k)
                .map(|(_, v)| v.trim())
        };
        if value("port.serial").and_then(|v| v.parse().ok()) == Some(console)
            && value("grpc.port").and_then(|v| v.parse().ok()) == Some(rpc)
            && value("avd.dir").map(Path::new) == Some(avd)
        {
            let mut token: MetadataValue<Ascii> =
                format!("Bearer {}", value("grpc.token")?).parse().ok()?;
            token.set_sensitive(true);
            return Some(token);
        }
    }
    None
}

pub(crate) fn authorized<T>(body: T, token: &MetadataValue<Ascii>) -> Request<T> {
    let mut request = Request::new(body);
    request
        .metadata_mut()
        .insert("authorization", token.clone());
    request
}
pub(crate) async fn open_controller<T>(
    console: u16,
    rpc: u16,
    avd: &Path,
    input: &mpsc::UnboundedReceiver<T>,
) -> DisplayResult<Option<(Channel, MetadataValue<Ascii>)>> {
    let deadline = Instant::now() + Duration::from_secs(90);
    let token = loop {
        if input.is_closed() {
            return Ok(None);
        }
        if let Some(token) = credentials(console, rpc, avd) {
            break token;
        }
        if Instant::now() >= deadline {
            return Err("O motor não anunciou o canal de vídeo autenticado.".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let endpoint = Endpoint::from_shared(format!("http://127.0.0.1:{rpc}"))?
        .initial_stream_window_size(16 * 1024 * 1024)
        .initial_connection_window_size(32 * 1024 * 1024)
        .connect_timeout(Duration::from_secs(5));
    let channel = loop {
        if input.is_closed() {
            return Ok(None);
        }
        match endpoint.connect().await {
            Ok(channel) => break channel,
            Err(error) if Instant::now() >= deadline => return Err(error.into()),
            Err(_) => tokio::time::sleep(Duration::from_millis(150)).await,
        }
    };
    Ok(Some((channel, token)))
}

/// Engine touch coordinates for `point`, a position on the Android screen as
/// shown (0 to 1 on each axis, from the top left corner). `android_size` is
/// the screen as shown, in pixels, and `rotation` the engine's quarter turns
/// (0 to 3; any other value counts as 0). Points outside the screen land on
/// its nearest edge.
pub(crate) fn normalized_to_engine(
    point: [f32; 2],
    android_size: [usize; 2],
    rotation: i32,
) -> (i32, i32) {
    let max_x = android_size[0] as i32 - 1;
    let max_y = android_size[1] as i32 - 1;
    let x = (point[0] * android_size[0] as f32) as i32;
    let y = (point[1] * android_size[1] as f32) as i32;
    let (x, y) = (x.clamp(0, max_x), y.clamp(0, max_y));
    match rotation {
        1 => (max_y - y, x),
        2 => (max_x - x, max_y - y),
        3 => (y, max_x - x),
        _ => (x, y),
    }
}

#[cfg(test)]
mod tests {
    use super::normalized_to_engine;

    // A landscape screen as shown, 1600 x 900 pixels.
    const SIZE: [usize; 2] = [1600, 900];

    #[test]
    fn upright_screen_keeps_the_shown_axes() {
        assert_eq!(normalized_to_engine([0.0, 0.0], SIZE, 0), (0, 0));
        assert_eq!(normalized_to_engine([0.5, 0.5], SIZE, 0), (800, 450));
        assert_eq!(normalized_to_engine([1.0, 0.0], SIZE, 0), (1599, 0));
        assert_eq!(normalized_to_engine([0.0, 1.0], SIZE, 0), (0, 899));
        assert_eq!(normalized_to_engine([1.0, 1.0], SIZE, 0), (1599, 899));
    }

    #[test]
    fn one_quarter_turn() {
        assert_eq!(normalized_to_engine([0.0, 0.0], SIZE, 1), (899, 0));
        assert_eq!(normalized_to_engine([0.5, 0.5], SIZE, 1), (449, 800));
        assert_eq!(normalized_to_engine([1.0, 0.0], SIZE, 1), (899, 1599));
        assert_eq!(normalized_to_engine([0.0, 1.0], SIZE, 1), (0, 0));
        assert_eq!(normalized_to_engine([1.0, 1.0], SIZE, 1), (0, 1599));
    }

    #[test]
    fn half_turn() {
        assert_eq!(normalized_to_engine([0.0, 0.0], SIZE, 2), (1599, 899));
        assert_eq!(normalized_to_engine([0.5, 0.5], SIZE, 2), (799, 449));
        assert_eq!(normalized_to_engine([1.0, 0.0], SIZE, 2), (0, 899));
        assert_eq!(normalized_to_engine([0.0, 1.0], SIZE, 2), (1599, 0));
        assert_eq!(normalized_to_engine([1.0, 1.0], SIZE, 2), (0, 0));
    }

    #[test]
    fn three_quarter_turns() {
        assert_eq!(normalized_to_engine([0.0, 0.0], SIZE, 3), (0, 1599));
        assert_eq!(normalized_to_engine([0.5, 0.5], SIZE, 3), (450, 799));
        assert_eq!(normalized_to_engine([1.0, 0.0], SIZE, 3), (0, 0));
        assert_eq!(normalized_to_engine([0.0, 1.0], SIZE, 3), (899, 1599));
        assert_eq!(normalized_to_engine([1.0, 1.0], SIZE, 3), (899, 0));
    }

    #[test]
    fn points_off_the_screen_land_on_its_edge() {
        for rotation in 0..4 {
            assert_eq!(
                normalized_to_engine([-0.25, -3.0], SIZE, rotation),
                normalized_to_engine([0.0, 0.0], SIZE, rotation)
            );
            assert_eq!(
                normalized_to_engine([1.5, 2.0], SIZE, rotation),
                normalized_to_engine([1.0, 1.0], SIZE, rotation)
            );
            assert_eq!(
                normalized_to_engine([f32::NAN, 0.5], SIZE, rotation),
                normalized_to_engine([0.0, 0.5], SIZE, rotation)
            );
        }
    }

    #[test]
    fn unknown_rotation_counts_as_upright() {
        for rotation in [-1, 4, i32::MAX] {
            assert_eq!(
                normalized_to_engine([0.25, 0.75], SIZE, rotation),
                (400, 675)
            );
        }
    }

    #[test]
    fn single_pixel_screen() {
        for rotation in 0..4 {
            assert_eq!(normalized_to_engine([0.7, 0.2], [1, 1], rotation), (0, 0));
        }
    }
}
