//! The stock ADB streaming-install protocol, on an already selected transport.

use std::{
    io::{self, Read, Write},
    net::TcpStream,
    time::{Duration, Instant},
};

use super::mirror_control::{read_bounded, service};

const IO_TICK: Duration = Duration::from_millis(200);
const APK_RESPONSE_LIMIT: usize = 16 * 1024;

fn check_transfer(current: &impl Fn() -> bool, deadline: Instant) -> io::Result<()> {
    if !current() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "The selected BlueStacks session is no longer available.",
        ));
    }
    if Instant::now() >= deadline {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Android did not finish installing the APK in time.",
        ));
    }
    Ok(())
}

pub(super) fn install_apk(
    mut transport: TcpStream,
    mut apk: impl Read,
    size: u64,
    deadline: Instant,
    current: impl Fn() -> bool,
) -> io::Result<()> {
    if !(1..=i64::MAX as u64).contains(&size) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "The APK must be a nonempty file with a supported size.",
        ));
    }
    check_transfer(&current, deadline)?;
    // Android 9's adb install uses raw exec and stdin with an explicit byte count.
    // Match its omitted filename: some Pie builds parse a trailing "-" as an unknown option.
    // Keep the socket open for the Android Package Manager result after writing exactly that count.
    service(
        &mut transport,
        &format!("exec:cmd package install -r -S {size}"),
    )?;
    transport.set_write_timeout(Some(IO_TICK))?;
    transport.set_read_timeout(Some(IO_TICK))?;

    let mut remaining = size;
    let mut buffer = [0u8; 32 * 1024];
    while remaining > 0 {
        check_transfer(&current, deadline)?;
        let count = remaining.min(buffer.len() as u64) as usize;
        apk.read_exact(&mut buffer[..count])?;
        let mut bytes = &buffer[..count];
        while !bytes.is_empty() {
            check_transfer(&current, deadline)?;
            match transport.write(bytes) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(written) => bytes = &bytes[written..],
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(error),
            }
        }
        remaining -= count as u64;
    }
    let output = read_bounded(transport, APK_RESPONSE_LIMIT, deadline, || !current())?;
    let reply = String::from_utf8_lossy(&output);
    if reply.trim() == "Success" {
        Ok(())
    } else if reply.trim().is_empty() {
        Err(io::Error::other(
            "Android did not confirm the APK installation.",
        ))
    } else {
        Err(io::Error::other(reply.trim().to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, net::TcpListener, thread};

    fn streams() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let request = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (device, _) = listener.accept().unwrap();
        for socket in [&request, &device] {
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(3)))
                .unwrap();
        }
        (request, device)
    }

    fn expect_request(device: &mut TcpStream, command: &str) {
        let mut prefix = [0; 4];
        device.read_exact(&mut prefix).unwrap();
        let length = usize::from_str_radix(std::str::from_utf8(&prefix).unwrap(), 16).unwrap();
        let mut request = vec![0; length];
        device.read_exact(&mut request).unwrap();
        assert_eq!(request, command.as_bytes());
        device.write_all(b"OKAY").unwrap();
    }

    #[test]
    fn streams_the_apk_on_the_selected_socket_and_requires_android_success() {
        const APK: &[u8] = b"PK\x03\x04apk payload";
        for (reply, success) in [
            (&b"Success\r\n"[..], true),
            (
                &b"Failure [INSTALL_FAILED_UPDATE_INCOMPATIBLE]\n"[..],
                false,
            ),
            (&b""[..], false),
        ] {
            let (request, mut device) = streams();
            let fake = thread::spawn(move || {
                expect_request(
                    &mut device,
                    &format!("exec:cmd package install -r -S {}", APK.len()),
                );
                let mut bytes = [0; APK.len()];
                device.read_exact(&mut bytes).unwrap();
                assert_eq!(bytes, APK);
                device.write_all(reply).unwrap();
            });
            let result = install_apk(
                request,
                APK,
                APK.len() as u64,
                Instant::now() + Duration::from_secs(3),
                || true,
            );
            fake.join().unwrap();
            assert_eq!(result.is_ok(), success, "{result:?}");
        }
    }

    #[test]
    fn a_revoked_selection_sends_no_install_request() {
        let (request, mut device) = streams();
        let error = install_apk(
            request,
            &b"abc"[..],
            3,
            Instant::now() + Duration::from_secs(3),
            || false,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(device.read(&mut [0; 1]).unwrap(), 0);
    }

    #[test]
    fn disconnect_during_a_local_read_drops_the_android_install_stream() {
        struct CancellingFile<'a>(&'a Cell<bool>);

        impl Read for CancellingFile<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                buffer.copy_from_slice(b"abc");
                self.0.set(false);
                Ok(buffer.len())
            }
        }

        let (request, mut device) = streams();
        let fake = thread::spawn(move || {
            expect_request(&mut device, "exec:cmd package install -r -S 3");
            assert_eq!(device.read(&mut [0; 1]).unwrap(), 0);
        });
        let current = Cell::new(true);
        let error = install_apk(
            request,
            CancellingFile(&current),
            3,
            Instant::now() + Duration::from_secs(3),
            || current.get(),
        )
        .unwrap_err();
        fake.join().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    }
}
