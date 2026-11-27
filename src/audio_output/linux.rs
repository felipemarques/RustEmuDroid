//! PulseAudio sinks through `pactl`, which PipeWire also provides (pipewire-pulse).

use super::{Device, Inventory};
use std::{
    io::Read,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(super) fn inventory() -> Result<Inventory, String> {
    let sinks = pactl(&["list", "sinks"])?;
    let mut inventory = Inventory {
        devices: parse_sinks(&sinks),
        default: None,
    };
    // Older servers lack get-default-sink; `info` has carried the line for long.
    inventory.default = pactl(&["info"]).ok().and_then(|info| default_sink(&info));
    Ok(inventory)
}

fn pactl(args: &[&str]) -> Result<String, String> {
    // Field labels are translated; the parser reads the untranslated ones.
    let mut child = Command::new("pactl")
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("pactl: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(status)) => return Err(format!("pactl: {status}")),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("pactl: timeout".into());
            }
            Err(error) => return Err(format!("pactl: {error}")),
        }
    }
    let mut text = String::new();
    child
        .stdout
        .take()
        .ok_or("pactl: no output")?
        .read_to_string(&mut text)
        .map_err(|error| format!("pactl: {error}"))?;
    Ok(text)
}

pub(super) fn parse_sinks(text: &str) -> Vec<Device> {
    let mut devices = Vec::new();
    let mut current: Option<(Option<String>, Option<String>)> = None;
    let mut finish = |entry: Option<(Option<String>, Option<String>)>| {
        if let Some((Some(id), name)) = entry {
            let name = name
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| id.clone());
            devices.push(Device { id, name });
        }
    };
    for line in text.lines() {
        if line.starts_with("Sink #") {
            finish(current.take());
            current = Some((None, None));
            continue;
        }
        let Some((id, name)) = current.as_mut() else {
            continue;
        };
        // Only the sink's own fields (one tab), not nested property lists.
        let Some(field) = line.strip_prefix('\t').filter(|f| !f.starts_with('\t')) else {
            continue;
        };
        if let Some(value) = field.strip_prefix("Name: ") {
            *id = Some(value.trim().to_owned());
        } else if let Some(value) = field.strip_prefix("Description: ") {
            *name = Some(value.trim().to_owned());
        }
    }
    finish(current);
    devices
}

pub(super) fn default_sink(info: &str) -> Option<String> {
    info.lines()
        .find_map(|line| line.strip_prefix("Default Sink: "))
        .map(|sink| sink.trim().to_owned())
        .filter(|sink| !sink.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_sinks_and_the_default_from_pactl() {
        let sinks = "Sink #47\n\tState: SUSPENDED\n\tName: alsa_output.pci-0000_00_1f.3.analog-stereo\n\tDescription: Built-in Audio Analog Stereo\n\tDriver: PipeWire\n\tProperties:\n\t\tdevice.description = \"Other\"\n\t\tName: nested\n\nSink #52\n\tName: bluez_output.00_11_22.1\n\tDescription: \n";
        assert_eq!(
            parse_sinks(sinks),
            [
                Device {
                    id: "alsa_output.pci-0000_00_1f.3.analog-stereo".into(),
                    name: "Built-in Audio Analog Stereo".into(),
                },
                Device {
                    id: "bluez_output.00_11_22.1".into(),
                    name: "bluez_output.00_11_22.1".into(),
                },
            ]
        );
        assert!(parse_sinks("").is_empty());
        let info = "Server Name: PulseAudio (on PipeWire 1.0.5)\nDefault Sink: bluez_output.00_11_22.1\nDefault Source: x\n";
        assert_eq!(
            default_sink(info).as_deref(),
            Some("bluez_output.00_11_22.1")
        );
        assert_eq!(default_sink("Default Sink: \n"), None);
    }
}
