use std::{
    fmt,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    num::{NonZeroU16, NonZeroU64},
};

const ENABLE_ADB: &str = "bst.enable_adb_access";
const INSTANCE_PREFIX: &str = "bst.instance.";
const ACTIVE_PORT_SUFFIX: &str = ".status.adb_port";
const ADB_HEADER: &str = "List of devices attached";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TargetError {
    InvalidInstance,
    AdbDisabled,
    InvalidAdbSetting,
    DuplicateSetting,
    InvalidConfiguredPort,
    NoActivePort,
    PortMismatch,
    SharedActivePort,
    InvalidDeviceList,
    NotConnected,
    NotReady,
    InvalidTransportId,
    AmbiguousTransport,
}

impl fmt::Display for TargetError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(match self {
            Self::InvalidInstance => "Invalid BlueStacks instance identifier.",
            Self::AdbDisabled => "ADB access is disabled in this BlueStacks installation.",
            Self::InvalidAdbSetting => "The BlueStacks ADB access setting is invalid.",
            Self::DuplicateSetting => "The BlueStacks ADB settings contain duplicate keys.",
            Self::InvalidConfiguredPort => {
                "The selected instance lacks a valid configured ADB port."
            }
            Self::NoActivePort => "BlueStacks reports no valid active ADB port for this instance.",
            Self::PortMismatch => "The configured and active BlueStacks ADB ports differ.",
            Self::SharedActivePort => {
                "More than one BlueStacks instance reports this active ADB port."
            }
            Self::InvalidDeviceList => "Expected one complete `adb devices -l` listing.",
            Self::NotConnected => "ADB does not list the selected BlueStacks endpoint.",
            Self::NotReady => {
                "The selected BlueStacks ADB transport is unavailable or unauthorized."
            }
            Self::InvalidTransportId => {
                "ADB did not provide one valid transport ID for the endpoint."
            }
            Self::AmbiguousTransport => "The BlueStacks endpoint has an ambiguous ADB transport.",
        })
    }
}

impl std::error::Error for TargetError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ConfiguredTarget {
    endpoint: SocketAddrV4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct MirrorTarget {
    pub(super) endpoint: SocketAddrV4,
    pub(super) transport_id: NonZeroU64,
}

/// Resolve an explicitly selected instance from one snapshot of the discovered installation's
/// configuration. BlueStacks stores a port, so the endpoint uses its canonical IPv4 loopback.
/// The caller must confirm that the listening process belongs to this installation and instance.
pub(super) fn configured_target(
    config: &str,
    instance: &str,
) -> Result<ConfiguredTarget, TargetError> {
    if !valid_instance(instance) {
        return Err(TargetError::InvalidInstance);
    }
    match setting(config, ENABLE_ADB)? {
        Some("1") => {}
        Some("0") | None => return Err(TargetError::AdbDisabled),
        Some(_) => return Err(TargetError::InvalidAdbSetting),
    }

    let configured_key = format!("{INSTANCE_PREFIX}{instance}.adb_port");
    let active_key = format!("{INSTANCE_PREFIX}{instance}{ACTIVE_PORT_SUFFIX}");
    let configured = setting(config, &configured_key)?
        .and_then(port)
        .ok_or(TargetError::InvalidConfiguredPort)?;
    let active = setting(config, &active_key)?
        .and_then(port)
        .ok_or(TargetError::NoActivePort)?;
    if configured != active {
        return Err(TargetError::PortMismatch);
    }
    if settings(config).any(|(key, value)| {
        let other = key
            .strip_prefix(INSTANCE_PREFIX)
            .and_then(|key| key.strip_suffix(ACTIVE_PORT_SUFFIX));
        other.is_some_and(|other| other != instance && valid_instance(other))
            && port(value) == Some(active)
    }) {
        return Err(TargetError::SharedActivePort);
    }
    Ok(ConfiguredTarget {
        endpoint: SocketAddrV4::new(Ipv4Addr::LOCALHOST, active.get()),
    })
}

impl ConfiguredTarget {
    pub(super) fn endpoint(self) -> SocketAddrV4 {
        self.endpoint
    }

    /// Match a fresh, successful `adb devices -l` response from the same local ADB server used
    /// for the session. Transport IDs are scoped to that server and its current connection.
    pub(super) fn validate_adb_devices(self, output: &str) -> Result<MirrorTarget, TargetError> {
        let mut rows = output
            .trim_start_matches('\u{feff}')
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty());
        if rows.next() != Some(ADB_HEADER) || rows.clone().any(|line| line == ADB_HEADER) {
            return Err(TargetError::InvalidDeviceList);
        }
        let serial = self.endpoint.to_string();
        let mut matched = None;
        for row in rows.clone() {
            let mut fields = row.split_ascii_whitespace();
            let Some(name) = fields.next() else { continue };
            if name == serial {
                if matched.replace(fields).is_some() {
                    return Err(TargetError::AmbiguousTransport);
                }
            } else if loopback_alias(name, self.endpoint.port()) {
                return Err(TargetError::AmbiguousTransport);
            }
        }
        let mut fields = matched.ok_or(TargetError::NotConnected)?;
        if fields.next() != Some("device") {
            return Err(TargetError::NotReady);
        }
        let mut ids = fields.filter_map(transport_field);
        let transport_id = ids
            .next()
            .and_then(transport_id)
            .ok_or(TargetError::InvalidTransportId)?;
        if ids.next().is_some()
            || rows
                .flat_map(str::split_ascii_whitespace)
                .filter_map(transport_field)
                .filter_map(self::transport_id)
                .filter(|&id| id == transport_id)
                .take(2)
                .count()
                != 1
        {
            return Err(TargetError::AmbiguousTransport);
        }

        Ok(MirrorTarget {
            endpoint: self.endpoint,
            transport_id,
        })
    }
}

pub(super) fn valid_instance(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn settings(config: &str) -> impl Iterator<Item = (&str, &str)> {
    config
        .trim_start_matches('\u{feff}')
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| {
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .unwrap_or(value);
            (key.trim(), value)
        })
}

fn setting<'a>(config: &'a str, name: &str) -> Result<Option<&'a str>, TargetError> {
    let mut values = settings(config).filter_map(|(key, value)| (key == name).then_some(value));
    let value = values.next();
    if values.next().is_some() {
        return Err(TargetError::DuplicateSetting);
    }
    Ok(value)
}

fn port(value: &str) -> Option<NonZeroU16> {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit())
        .then(|| value.parse().ok())
        .flatten()
}

fn transport_id(value: &str) -> Option<NonZeroU64> {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit())
        .then(|| value.parse().ok())
        .flatten()
}

fn transport_field(field: &str) -> Option<&str> {
    field.strip_prefix("transport_id:")
}

fn loopback_alias(serial: &str, expected_port: u16) -> bool {
    if let Ok(endpoint) = serial.parse::<SocketAddr>() {
        let loopback = match endpoint {
            SocketAddr::V4(endpoint) => endpoint.ip().is_loopback(),
            SocketAddr::V6(endpoint) => {
                endpoint.ip().is_loopback()
                    || endpoint
                        .ip()
                        .to_ipv4_mapped()
                        .is_some_and(|ip| ip.is_loopback())
            }
        };
        return loopback && endpoint.port() == expected_port;
    }
    serial.rsplit_once(':').is_some_and(|(host, port)| {
        host.trim_end_matches('.').eq_ignore_ascii_case("localhost")
            && port.parse::<u16>() == Ok(expected_port)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = concat!(
        "\u{feff}bst.enable_adb_access=\"1\"\r\n",
        "bst.installed_images=\"Pie64,Nougat32\"\r\n",
        "bst.instance.Pie64.adb_port=\"5585\"\r\n",
        "bst.instance.Pie64.display_name=\"BlueStacks App Player\"\r\n",
        "bst.instance.Pie64.status.adb_port=\"5585\"\r\n",
        "bst.instance.Pie64_1.adb_port=\"5586\"\r\n",
        "bst.instance.Pie64_1.display_name=\"Segunda instalação\"\r\n",
        "bst.instance.Pie64_1.status.adb_port=\"5586\"\r\n",
    );
    const DEVICE: &str =
        "127.0.0.1:5585\tdevice product:Pie model:Pixel_2 device:Pie64 transport_id:27";

    fn pie64() -> ConfiguredTarget {
        configured_target(CONFIG, "Pie64").unwrap()
    }

    fn listing(rows: &str) -> String {
        format!("{ADB_HEADER}\r\n{rows}\r\n\r\n")
    }

    #[test]
    fn checks_the_chosen_instance_and_one_ready_transport_among_other_devices() {
        let configured = pie64();
        assert_eq!(configured.endpoint().to_string(), "127.0.0.1:5585");
        assert_eq!(
            configured_target(CONFIG, "Pie64_1")
                .unwrap()
                .endpoint()
                .to_string(),
            "127.0.0.1:5586"
        );
        let output = listing(&format!(
            "emulator-5554 device transport_id:1\r\n{DEVICE}\r\n192.168.1.5:5585 device transport_id:28"
        ));
        let matched = configured.validate_adb_devices(&output).unwrap();
        assert_eq!(matched.endpoint, configured.endpoint());
        assert_eq!(matched.transport_id.get(), 27);
    }

    #[test]
    fn requires_explicit_enablement_and_an_exact_instance_id() {
        assert_eq!(
            configured_target(&CONFIG.replace("access=\"1\"", "access=\"0\""), "Pie64"),
            Err(TargetError::AdbDisabled)
        );
        assert_eq!(
            configured_target("", "Pie64"),
            Err(TargetError::AdbDisabled)
        );
        assert_eq!(
            configured_target(&CONFIG.replace("access=\"1\"", "access=\"1"), "Pie64"),
            Err(TargetError::InvalidAdbSetting)
        );
        assert_eq!(
            configured_target(CONFIG, "Pie"),
            Err(TargetError::InvalidConfiguredPort)
        );
        assert_eq!(
            configured_target(CONFIG, "Pie64.status"),
            Err(TargetError::InvalidInstance)
        );
    }

    #[test]
    fn requires_the_active_port_to_agree_with_the_configured_port() {
        let active_setting = "bst.instance.Pie64.status.adb_port=\"5585\"";
        for value in ["", "0", "-1", "65536", "localhost:5585", "+5585"] {
            let changed = CONFIG.replace(
                active_setting,
                &format!("bst.instance.Pie64.status.adb_port=\"{value}\""),
            );
            assert_eq!(
                configured_target(&changed, "Pie64"),
                Err(TargetError::NoActivePort)
            );
        }
        assert_eq!(
            configured_target(&CONFIG.replace(active_setting, ""), "Pie64"),
            Err(TargetError::NoActivePort)
        );
        let changed = CONFIG.replace(
            active_setting,
            "bst.instance.Pie64.status.adb_port=\"5586\"",
        );
        assert_eq!(
            configured_target(&changed, "Pie64"),
            Err(TargetError::PortMismatch)
        );
    }

    #[test]
    fn rejects_duplicate_keys_and_shared_active_ports() {
        for line in [
            "bst.enable_adb_access=\"1\"",
            "bst.instance.Pie64.adb_port=\"5555\"",
            "bst.instance.Pie64.status.adb_port=\"5585\"",
        ] {
            assert_eq!(
                configured_target(&format!("{CONFIG}{line}\n"), "Pie64"),
                Err(TargetError::DuplicateSetting)
            );
        }
        let changed = CONFIG.replace(
            "bst.instance.Pie64_1.status.adb_port=\"5586\"",
            "bst.instance.Pie64_1.status.adb_port=\"5585\"",
        );
        assert_eq!(
            configured_target(&changed, "Pie64"),
            Err(TargetError::SharedActivePort)
        );
    }

    #[test]
    fn rejects_non_ready_or_missing_targets() {
        for state in ["offline", "unauthorized", "bootloader", "recovery"] {
            assert_eq!(
                pie64().validate_adb_devices(&listing(&format!(
                    "127.0.0.1:5585 {state} transport_id:27"
                ))),
                Err(TargetError::NotReady)
            );
        }
        assert_eq!(
            pie64().validate_adb_devices(&listing("192.168.1.5:5585 device transport_id:27")),
            Err(TargetError::NotConnected)
        );
        assert_eq!(
            pie64().validate_adb_devices(DEVICE),
            Err(TargetError::InvalidDeviceList)
        );
        assert_eq!(
            pie64().validate_adb_devices(&listing("")),
            Err(TargetError::NotConnected)
        );
    }

    #[test]
    fn rejects_duplicates_and_loopback_aliases() {
        for row in [
            "127.0.0.1:5585 unauthorized transport_id:28",
            "localhost:5585 device transport_id:29",
            "[::1]:5585 device transport_id:30",
            "127.0.0.2:5585 device transport_id:31",
            "[::ffff:127.0.0.1]:5585 device transport_id:32",
            "emulator-5554 device transport_id:27",
        ] {
            assert_eq!(
                pie64().validate_adb_devices(&listing(&format!("{DEVICE}\n{row}"))),
                Err(TargetError::AmbiguousTransport)
            );
        }
    }

    #[test]
    fn requires_a_positive_transport_id_from_the_long_device_listing() {
        for suffix in [
            "",
            "transport_id:",
            "transport_id:0",
            "transport_id:+27",
            "transport_id:18446744073709551616",
        ] {
            assert_eq!(
                pie64().validate_adb_devices(&listing(&format!("127.0.0.1:5585 device {suffix}"))),
                Err(TargetError::InvalidTransportId)
            );
        }
        assert_eq!(
            pie64().validate_adb_devices(&listing(&format!("{DEVICE} transport_id:28"))),
            Err(TargetError::AmbiguousTransport)
        );
    }
}
