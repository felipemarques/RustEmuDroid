//! Hold a relative physical rotation through the EmulatorController sensor model.
use super::{Empty, authorized};
use prost::Message;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{oneshot, watch};
use tonic::{
    client::Grpc,
    codec::ProstCodec,
    codegen::http::uri::PathAndQuery,
    metadata::{Ascii, MetadataValue},
    transport::Channel,
};

type Pose = [f32; 3];
const NEUTRAL: Pose = [0.; 3];
const ROTATION: i32 = 1;
const STEP: i32 = 1;
const RPC_TIMEOUT: Duration = Duration::from_millis(250);
pub(super) const RESTORE_TIMEOUT: Duration = Duration::from_millis(1500);

#[derive(Clone, PartialEq, Message)]
struct PhysicalModelValue {
    #[prost(int32, tag = "1")]
    target: i32,
    #[prost(int32, tag = "2")]
    status: i32,
    #[prost(message, optional, tag = "3")]
    value: Option<ParameterValue>,
    #[prost(int32, tag = "4")]
    interpolation: i32,
}

#[derive(Clone, PartialEq, Message)]
struct ParameterValue {
    #[prost(float, repeated, packed = "true", tag = "1")]
    data: Vec<f32>,
}

#[derive(Clone, Copy, Default)]
struct Goal {
    revision: u64,
    offset: Pose,
}

pub(super) struct Tilt {
    requests: watch::Sender<Goal>,
    acknowledged: Arc<AtomicU64>,
    goal: Goal,
}

pub(super) struct Worker {
    requests: watch::Receiver<Goal>,
    acknowledged: Arc<AtomicU64>,
}

impl Tilt {
    pub(super) fn channel() -> (Self, Worker) {
        let (tx, rx) = watch::channel(Goal::default());
        let acknowledged = Arc::new(AtomicU64::new(0));
        (
            Self {
                requests: tx,
                acknowledged: acknowledged.clone(),
                goal: Goal::default(),
            },
            Worker {
                requests: rx,
                acknowledged,
            },
        )
    }

    pub(super) fn set(&mut self, offset: Pose) {
        if self.goal.offset != offset {
            self.goal = Goal {
                revision: self.goal.revision + 1,
                offset,
            };
            self.requests.send_replace(self.goal);
        }
    }

    pub(super) fn restore(&mut self) {
        self.set(NEUTRAL);
    }

    pub(super) fn restored(&self) -> bool {
        self.goal.offset == NEUTRAL
            && self.acknowledged.load(Ordering::Acquire) == self.goal.revision
    }
}

impl Worker {
    pub(super) async fn run(
        self,
        channel: Channel,
        token: MetadataValue<Ascii>,
        shutdown: oneshot::Receiver<()>,
    ) {
        self.run_with(
            Rpc {
                client: Grpc::new(channel),
                token,
            },
            shutdown,
        )
        .await;
    }

    async fn run_with(mut self, mut rpc: impl Protocol, mut shutdown: oneshot::Receiver<()>) {
        let mut state = Rotation::default();
        let mut revision = 0;
        let mut attempts = 0;
        loop {
            let goal = *self.requests.borrow_and_update();
            if goal.revision != revision {
                revision = goal.revision;
                attempts = 0;
            }
            attempts += 1;
            let done = state
                .apply(&mut rpc, goal.offset, || {
                    self.requests.borrow().revision == goal.revision
                })
                .await;
            if done {
                self.acknowledged.store(goal.revision, Ordering::Release);
            }
            tokio::select! {
                change = self.requests.changed() => {
                    if change.is_err() {
                        break;
                    }
                }
                _ = &mut shutdown => break,
                _ = tokio::time::sleep(RPC_TIMEOUT), if !done && attempts < 3 => {}
            }
        }
        if state.restore(&mut rpc).await {
            self.acknowledged
                .store(self.requests.borrow().revision, Ordering::Release);
        }
    }
}

trait Protocol {
    async fn get(&mut self) -> Option<Pose>;
    async fn set(&mut self, pose: Pose) -> bool;
}

struct Rpc {
    client: Grpc<Channel>,
    token: MetadataValue<Ascii>,
}

impl Protocol for Rpc {
    async fn get(&mut self) -> Option<Pose> {
        tokio::time::timeout(RPC_TIMEOUT, async {
            self.client.ready().await.ok()?;
            let mut request = authorized(
                PhysicalModelValue {
                    target: ROTATION,
                    ..Default::default()
                },
                &self.token,
            );
            request.set_timeout(RPC_TIMEOUT);
            let reply: PhysicalModelValue = self
                .client
                .unary(
                    request,
                    PathAndQuery::from_static(
                        "/android.emulation.control.EmulatorController/getPhysicalModel",
                    ),
                    ProstCodec::default(),
                )
                .await
                .ok()?
                .into_inner();
            let pose: Pose = reply.value?.data.try_into().ok()?;
            (reply.target == ROTATION && reply.status == 0 && pose.into_iter().all(f32::is_finite))
                .then_some(pose)
        })
        .await
        .ok()
        .flatten()
    }

    async fn set(&mut self, pose: Pose) -> bool {
        tokio::time::timeout(RPC_TIMEOUT, async {
            self.client.ready().await.ok()?;
            let mut request = authorized(
                PhysicalModelValue {
                    target: ROTATION,
                    value: Some(ParameterValue { data: pose.into() }),
                    interpolation: STEP,
                    ..Default::default()
                },
                &self.token,
            );
            request.set_timeout(RPC_TIMEOUT);
            let _: tonic::Response<Empty> = self
                .client
                .unary(
                    request,
                    PathAndQuery::from_static(
                        "/android.emulation.control.EmulatorController/setPhysicalModel",
                    ),
                    ProstCodec::default(),
                )
                .await
                .ok()?;
            Some(())
        })
        .await
        .is_ok_and(|result| result.is_some())
    }
}

#[derive(Default)]
struct Rotation {
    original: Option<Pose>,
    suppressed: bool,
}

impl Rotation {
    async fn apply(
        &mut self,
        rpc: &mut impl Protocol,
        offset: Pose,
        still_wanted: impl FnOnce() -> bool,
    ) -> bool {
        if offset == NEUTRAL {
            let restored = self.restore(rpc).await;
            self.suppressed = !restored;
            return restored;
        }
        if self.suppressed {
            return self.restore(rpc).await;
        }
        if self.original.is_none() {
            let Some(first) = rpc.get().await else {
                self.suppressed = true;
                return true;
            };
            // The controller exposes current rotation, so skip a pose in transit.
            tokio::time::sleep(Duration::from_millis(65)).await;
            let Some(original) = rpc
                .get()
                .await
                .filter(|second| second.iter().zip(first).all(|(a, b)| (a - b).abs() < 0.05))
            else {
                self.suppressed = true;
                return true;
            };
            if !still_wanted() {
                return true;
            }
            self.original = Some(original);
        } else if !still_wanted() {
            return true;
        }
        let original = self
            .original
            .expect("the baseline was captured before applying tilt");
        let target = std::array::from_fn(|i| original[i] + offset[i]);
        if rpc.set(target).await {
            true
        } else {
            // A timeout may follow an applied write. Restore before allowing another tilt.
            self.suppressed = true;
            self.restore(rpc).await
        }
    }

    async fn restore(&mut self, rpc: &mut impl Protocol) -> bool {
        if let Some(original) = self.original {
            if !rpc.set(original).await {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(32)).await;
            if !rpc
                .get()
                .await
                .is_some_and(|current| same_pose(current, original, 0.1))
            {
                return false;
            }
            self.original = None;
        }
        true
    }
}

fn same_pose(a: Pose, b: Pose, tolerance: f32) -> bool {
    a.into_iter()
        .zip(b)
        .all(|(a, b)| ((a - b + 180.).rem_euclid(360.) - 180.).abs() < tolerance)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct Mock {
        reads: VecDeque<Option<Pose>>,
        writes: Vec<Pose>,
        results: VecDeque<bool>,
        current: Option<Pose>,
    }

    impl Protocol for Mock {
        async fn get(&mut self) -> Option<Pose> {
            self.reads.pop_front().unwrap_or(self.current)
        }
        async fn set(&mut self, pose: Pose) -> bool {
            self.writes.push(pose);
            self.current = Some(pose);
            self.results.pop_front().unwrap_or(true)
        }
    }

    #[tokio::test]
    async fn relative_axis_updates_restore_the_same_nonzero_baseline() {
        let original = [12., -21., 90.];
        let mut rpc = Mock {
            reads: [Some(original), Some(original)].into(),
            ..Default::default()
        };
        let mut state = Rotation::default();
        assert!(state.apply(&mut rpc, [20., 0., 0.], || true).await);
        assert!(state.apply(&mut rpc, [20., -10., 0.], || true).await);
        assert!(state.apply(&mut rpc, NEUTRAL, || true).await);
        assert!(state.apply(&mut rpc, NEUTRAL, || true).await);
        assert_eq!(rpc.writes, [[32., -21., 90.], [32., -31., 90.], original]);
    }

    #[tokio::test]
    async fn unsupported_moving_and_cancelled_pose_never_write() {
        for (reads, still_wanted) in [
            ([None, None], true),
            ([Some(NEUTRAL), Some([8., 0., 0.])], true),
            ([Some(NEUTRAL), Some(NEUTRAL)], false),
        ] {
            let mut rpc = Mock {
                reads: reads.into(),
                ..Default::default()
            };
            let mut state = Rotation::default();
            assert!(state.apply(&mut rpc, [15., 0., 0.], || still_wanted).await);
            assert!(state.restore(&mut rpc).await);
            assert!(rpc.writes.is_empty());
        }
    }

    #[tokio::test]
    async fn uncertain_write_restores_before_accepting_another_activation() {
        let original = [10., 12., 15.];
        let mut rpc = Mock {
            reads: [Some(original), Some(original)].into(),
            results: [false, false, true].into(),
            ..Default::default()
        };
        let mut state = Rotation::default();
        assert!(!state.apply(&mut rpc, [25., 0., 0.], || true).await);
        assert!(state.apply(&mut rpc, [35., 0., 0.], || true).await);
        assert!(state.apply(&mut rpc, [45., 0., 0.], || true).await);
        assert!(state.apply(&mut rpc, NEUTRAL, || true).await);
        assert_eq!(rpc.writes, [[35., 12., 15.], original, original]);
    }

    #[tokio::test]
    async fn failed_restore_readback_suppresses_the_next_held_offset() {
        let original = [10., 12., 15.];
        let tilted = [30., 12., 15.];
        let mut rpc = Mock {
            reads: [Some(original), Some(original), Some(tilted), Some(original)].into(),
            ..Default::default()
        };
        let mut state = Rotation::default();
        assert!(state.apply(&mut rpc, [20., 0., 0.], || true).await);
        assert!(!state.apply(&mut rpc, NEUTRAL, || true).await);
        assert!(state.apply(&mut rpc, [10., 0., 0.], || true).await);
        assert_eq!(rpc.writes, [tilted, original, original]);
    }

    #[test]
    fn an_old_ack_does_not_complete_a_newer_restore() {
        let (mut tilt, worker) = Tilt::channel();
        assert!(tilt.restored());
        tilt.set([1., 0., 0.]);
        tilt.restore();
        worker.acknowledged.store(1, Ordering::Release);
        assert!(!tilt.restored());
        worker.acknowledged.store(2, Ordering::Release);
        assert!(tilt.restored());
    }

    #[tokio::test]
    #[ignore = "changes and restores physical rotation on the selected idle AVD with RustEmuDroid Home foreground"]
    async fn physical_rotation_changes_and_restores_on_selected_guest() {
        use std::{path::PathBuf, process::Command, time::Instant};
        use tonic::transport::Endpoint;

        let env = |name| std::env::var(name).unwrap();
        let console: u16 = env("RUSTEMUDROID_TEST_CONSOLE").parse().unwrap();
        let rpc: u16 = env("RUSTEMUDROID_TEST_TOUCH_RPC").parse().unwrap();
        let avd = PathBuf::from(env("RUSTEMUDROID_TEST_AVD"));
        let adb = env("RUSTEMUDROID_TEST_ADB");
        let serial = format!("emulator-{console}");
        let guest = |args: &[&str]| {
            let reply = Command::new(&adb)
                .args(["-s", &serial])
                .args(args)
                .output()
                .unwrap();
            assert!(reply.status.success());
            String::from_utf8(reply.stdout).unwrap()
        };
        assert_eq!(
            PathBuf::from(
                guest(&["emu", "avd", "path"])
                    .lines()
                    .next()
                    .unwrap()
                    .trim()
            ),
            avd
        );
        assert!(
            guest(&["shell", "dumpsys", "activity", "top-resumed"])
                .contains("org.rustemudroid.home")
        );

        let token = super::super::credentials(console, rpc, &avd).unwrap();
        let channel = Endpoint::from_shared(format!("http://127.0.0.1:{rpc}"))
            .unwrap()
            .connect_timeout(Duration::from_secs(2))
            .connect()
            .await
            .unwrap();
        let mut reader = Rpc {
            client: Grpc::new(channel.clone()),
            token: token.clone(),
        };
        let original = reader
            .get()
            .await
            .expect("engine must report physical rotation before use");
        let (mut tilt, worker) = Tilt::channel();
        let (cancel, shutdown) = oneshot::channel();
        let task = tokio::spawn(worker.run(channel, token, shutdown));
        tilt.set([15., 0., 0.]);
        let until = Instant::now() + Duration::from_secs(2);
        let mut moved = None;
        while Instant::now() < until {
            if let Some(actual) = reader.get().await
                && !same_pose(actual, original, 0.2)
            {
                moved = Some(actual);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tilt.restore();
        let until = Instant::now() + RESTORE_TIMEOUT;
        while !tilt.restored() && Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let _ = cancel.send(());
        task.await.unwrap();
        let restored = reader.get().await;
        println!("Rotation before={original:?} held={moved:?} restored={restored:?}");
        assert!(tilt.restored() && restored.is_some_and(|p| same_pose(p, original, 0.2)));
        let target = [original[0] + 15., original[1], original[2]];
        assert!(moved.is_some_and(|p| same_pose(p, target, 0.25)));
    }
}
