use std::{
    io::Read,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Sender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use super::{
    mirror_control::{service, transport_socket},
    mirror_session::{MirrorEvent, adb_target},
    mirror_target::{ConfiguredTarget, MirrorTarget},
    valid_package,
};

pub(super) struct ForegroundWatch {
    epoch: Arc<AtomicU64>,
    stop: Sender<()>,
    worker: Option<JoinHandle<()>>,
}

impl ForegroundWatch {
    pub(super) fn start(
        configured: ConfiguredTarget,
        target: MirrorTarget,
        events: Sender<MirrorEvent>,
    ) -> Self {
        let (stop, stopping) = mpsc::channel();
        let epoch = Arc::new(AtomicU64::new(0));
        let observed_epoch = epoch.clone();
        let worker = thread::Builder::new()
            .name("blue-foreground".into())
            .spawn(move || {
                let mut previous = None;
                loop {
                    let package = focused_package(configured, target);
                    if previous != package {
                        let generation = observed_epoch.fetch_add(1, Ordering::AcqRel) + 1;
                        previous.clone_from(&package);
                        if events
                            .send(MirrorEvent::Foreground(generation, package))
                            .is_err()
                        {
                            return;
                        }
                    }
                    if stopping.recv_timeout(Duration::from_secs(1))
                        != Err(mpsc::RecvTimeoutError::Timeout)
                    {
                        return;
                    }
                }
            });
        if let Err(error) = &worker {
            eprintln!("BlueStacks foreground tracking: {error}");
        }
        Self {
            epoch,
            stop,
            worker: worker.ok(),
        }
    }

    pub(super) fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }
}

impl Drop for ForegroundWatch {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn focused_package(configured: ConfiguredTarget, target: MirrorTarget) -> Option<String> {
    // A restarted server may reuse a transport ID for a different emulator.
    if adb_target(configured).ok()?.ok()? != target {
        return None;
    }
    let mut socket = transport_socket(target.transport_id.get()).ok()?;
    service(
        &mut socket,
        "shell:dumpsys window | grep -m 1 'mCurrentFocus='",
    )
    .ok()?;
    let mut response = String::new();
    socket.take(4096).read_to_string(&mut response).ok()?;
    window_package(&response).map(str::to_owned)
}

fn window_package(response: &str) -> Option<&str> {
    // mFocusedApp may still identify the game behind a notification shade or keyguard.
    let window = response
        .trim()
        .strip_prefix("mCurrentFocus=Window{")?
        .strip_suffix('}')?;
    let (_, component) = window.rsplit_once(char::is_whitespace)?;
    let (package, activity) = component.split_once('/')?;
    (valid_package(package) && !activity.is_empty()).then_some(package)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn selects_the_window_receiving_input_and_clears_overlays() {
        assert_eq!(
            window_package(
                "  mCurrentFocus=Window{acbb1 u0 org.supertuxkart.stk/.MainActivity}\r\n"
            ),
            Some("org.supertuxkart.stk")
        );
        for line in [
            "mCurrentFocus=null",
            "mCurrentFocus=Window{ab u0 NotificationShade}",
            "mFocusedApp=AppWindowToken{aa com.android.settings/.Settings}",
            "mCurrentFocus=Window{ac u0 ../wrong/.Main}",
            "mCurrentFocus=Window{ab u0 org.supertuxkart.stk/}",
        ] {
            assert_eq!(window_package(line), None, "{line}");
        }
    }

    #[test]
    fn each_instance_has_its_own_profile_file_under_the_launcher() {
        let root = Path::new("state");
        let path = super::super::mapping_file(root, "Pie64_1").unwrap();
        assert_eq!(path.parent(), Some(root));
        assert_ne!(path, super::super::mapping_file(root, "Pie64").unwrap());
        assert_ne!(path, root.join("touch-maps.json"));
        assert_eq!(super::super::mapping_file(root, "../another"), None);
    }
}
