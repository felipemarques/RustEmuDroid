//! Which device the window's player uses and what the user is told about it,
//! decided without calling the Windows audio interfaces, so it runs in tests without a sound
//! card. The chosen device plays while it works. While it does not, the
//! system default plays and the chosen device is tried again every few
//! seconds.

use crate::locale::Locale;
use std::time::{Duration, Instant};

/// How often the chosen device is tried again while it is unavailable.
pub(crate) const CHOSEN_RETRY: Duration = Duration::from_secs(3);
/// How often the system default is tried again after it failed to open.
pub(crate) const DEFAULT_RETRY: Duration = Duration::from_secs(1);
/// How often the system default is resolved again while it plays, so the
/// sound follows a change of the default as the engine's own output does.
pub(crate) const DEFAULT_CHECK: Duration = Duration::from_secs(1);
/// Without any open output for this long, the user is told.
pub(crate) const SILENT_NOTICE: Duration = Duration::from_secs(3);
/// A device that fails this soon after opening is not reopened at once, so
/// one that keeps failing cannot spin the player.
const SETTLE: Duration = Duration::from_secs(1);

const AUDCLNT_E_DEVICE_INVALIDATED: i32 = 0x8889_0004_u32 as i32;
const AUDCLNT_E_DEVICE_IN_USE: i32 = 0x8889_000A_u32 as i32;
/// HRESULT_FROM_WIN32(ERROR_NOT_FOUND): no such endpoint, or no default one.
const E_NOTFOUND: i32 = 0x8007_0490_u32 as i32;

/// Why a device cannot play.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Fault {
    /// Unplugged, disabled or removed.
    Gone,
    /// Another program holds it in exclusive mode.
    Busy,
    /// Any other Windows audio failure: the call and its HRESULT.
    Call(&'static str, i32),
}

impl Fault {
    pub(crate) fn from_call(what: &'static str, hr: i32) -> Self {
        match hr {
            AUDCLNT_E_DEVICE_INVALIDATED | E_NOTFOUND => Self::Gone,
            AUDCLNT_E_DEVICE_IN_USE => Self::Busy,
            _ => Self::Call(what, hr),
        }
    }

    pub(crate) fn text(&self, locale: Locale) -> String {
        match self {
            Self::Gone => locale
                .pick(
                    "dispositivo desconectado ou desativado",
                    "device disconnected or disabled",
                    "dispositivo desconectado o desactivado",
                )
                .into(),
            Self::Busy => locale
                .pick(
                    "dispositivo em uso exclusivo por outro programa",
                    "device in exclusive use by another program",
                    "dispositivo en uso exclusivo por otro programa",
                )
                .into(),
            Self::Call(what, hr) => format!(
                "{} {what}: 0x{:08X}",
                locale.pick(
                    "erro do áudio do Windows em",
                    "Windows audio error in",
                    "error del audio de Windows en",
                ),
                *hr as u32
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Chosen,
    Default,
}

/// Where the sound is now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    /// No device has answered yet, or one is being reopened.
    Opening,
    /// The chosen device plays.
    Chosen,
    /// The chosen device is unavailable for this reason; the system default plays.
    Default(Fault),
    /// Nothing plays: why the system default could not be opened either.
    Silent(Fault),
}

/// A change the user is told about, once, after it happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Notice {
    /// The sound moved to the system default because the chosen device failed.
    Fallback(Fault),
    /// The chosen device plays (again).
    Returned,
    /// No output has been open for `SILENT_NOTICE`.
    Silent(Fault),
}

/// What the player does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Try to open this device. While the system default plays, the chosen
    /// device is opened beside it and replaces it only if that works.
    Open(Target),
    /// Compare the system default with the endpoint in use.
    CheckDefault,
    Wait,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Told {
    Nothing,
    Fallback,
    Silent,
}

pub(crate) struct Router {
    on: Option<Target>,
    /// Why the chosen device is not in use.
    chosen_fault: Option<Fault>,
    /// Why the system default could not be opened.
    default_fault: Option<Fault>,
    next_chosen: Instant,
    next_default: Instant,
    next_check: Instant,
    opened_at: Instant,
    /// Since when no output is open.
    silent_since: Option<Instant>,
    /// What the user was last told, so each change is announced once.
    told: Told,
}

impl Router {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            on: None,
            chosen_fault: None,
            default_fault: None,
            next_chosen: now,
            next_default: now,
            next_check: now,
            opened_at: now,
            silent_since: Some(now),
            told: Told::Nothing,
        }
    }

    pub(crate) fn step(&self, now: Instant) -> Step {
        match self.on {
            Some(Target::Chosen) => Step::Wait,
            // The chosen device comes first, also while the default plays.
            _ if now >= self.next_chosen => Step::Open(Target::Chosen),
            Some(Target::Default) if now >= self.next_check => Step::CheckDefault,
            None if now >= self.next_default => Step::Open(Target::Default),
            _ => Step::Wait,
        }
    }

    pub(crate) fn opened(&mut self, target: Target, now: Instant) {
        self.on = Some(target);
        self.opened_at = now;
        self.silent_since = None;
        self.default_fault = None;
        match target {
            Target::Chosen => self.chosen_fault = None,
            Target::Default => self.next_check = now + DEFAULT_CHECK,
        }
    }

    /// An attempt to open `target` failed. A device that is playing stays.
    pub(crate) fn failed(&mut self, target: Target, fault: Fault, now: Instant) {
        match target {
            Target::Chosen => {
                self.chosen_fault = Some(fault);
                self.next_chosen = now + CHOSEN_RETRY;
            }
            Target::Default => {
                self.default_fault = Some(fault);
                self.next_default = now + DEFAULT_RETRY;
            }
        }
    }

    /// The device in use stopped (unplugged, format changed, taken over).
    pub(crate) fn lost(&mut self, fault: Fault, now: Instant) {
        let Some(target) = self.on.take() else {
            return;
        };
        let settled = now.duration_since(self.opened_at) >= SETTLE;
        match target {
            Target::Chosen => {
                self.chosen_fault = Some(fault);
                // A format change only needs the device opened again.
                self.next_chosen = if settled { now } else { now + CHOSEN_RETRY };
                self.next_default = now;
            }
            Target::Default => {
                self.next_default = if settled { now } else { now + DEFAULT_RETRY };
            }
        }
        self.silent_since = Some(now);
    }

    /// The system default was compared with the endpoint in use.
    pub(crate) fn checked(&mut self, now: Instant) {
        self.next_check = now + DEFAULT_CHECK;
    }

    pub(crate) fn status(&self) -> Status {
        match (self.on, &self.default_fault) {
            (Some(Target::Chosen), _) => Status::Chosen,
            (Some(Target::Default), _) => {
                Status::Default(self.chosen_fault.clone().unwrap_or(Fault::Gone))
            }
            (None, Some(fault)) => Status::Silent(fault.clone()),
            (None, None) => Status::Opening,
        }
    }

    /// What to tell the user now: each change once, and only after it is true.
    pub(crate) fn notice(&mut self, now: Instant) -> Option<Notice> {
        let (told, notice) = match self.status() {
            Status::Chosen => (Told::Nothing, Notice::Returned),
            Status::Default(fault) => (Told::Fallback, Notice::Fallback(fault)),
            Status::Silent(fault)
                if self
                    .silent_since
                    .is_some_and(|since| now.duration_since(since) >= SILENT_NOTICE) =>
            {
                (Told::Silent, Notice::Silent(fault))
            }
            Status::Silent(_) | Status::Opening => return None,
        };
        if told == self.told {
            return None;
        }
        self.told = told;
        Some(notice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    /// The player's loop with scripted devices: opens what the router asks
    /// for and returns the notices raised at `now`.
    fn run(
        router: &mut Router,
        now: Instant,
        chosen: Result<(), Fault>,
        default: Result<(), Fault>,
    ) -> Vec<Notice> {
        let mut notices = Vec::new();
        for _ in 0..4 {
            match router.step(now) {
                Step::Open(target) => {
                    let device = match target {
                        Target::Chosen => &chosen,
                        Target::Default => &default,
                    };
                    match device {
                        Ok(()) => router.opened(target, now),
                        Err(fault) => router.failed(target, fault.clone(), now),
                    }
                }
                Step::CheckDefault => router.checked(now),
                Step::Wait => {}
            }
            notices.extend(router.notice(now));
        }
        notices
    }

    #[test]
    fn the_chosen_device_plays_without_a_notice() {
        let start = Instant::now();
        let mut router = Router::new(start);
        assert_eq!(router.status(), Status::Opening);
        assert_eq!(router.step(start), Step::Open(Target::Chosen));
        assert!(run(&mut router, start, Ok(()), Ok(())).is_empty());
        assert_eq!(router.status(), Status::Chosen);
        assert_eq!(router.step(start + 60_000 * MS), Step::Wait);
    }

    #[test]
    fn falls_back_only_after_the_default_opened_and_returns_when_the_device_is_back() {
        let start = Instant::now();
        let mut router = Router::new(start);
        run(&mut router, start, Ok(()), Ok(()));

        // Unplugged after a minute: one immediate attempt, then the default.
        let lost = start + 60_000 * MS;
        router.lost(Fault::Gone, lost);
        assert_eq!(router.status(), Status::Opening);
        assert_eq!(router.notice(lost), None, "nothing is true yet");
        assert_eq!(router.step(lost), Step::Open(Target::Chosen));
        router.failed(Target::Chosen, Fault::Gone, lost);
        assert_eq!(router.notice(lost), None);
        assert_eq!(router.step(lost), Step::Open(Target::Default));
        router.opened(Target::Default, lost);
        assert_eq!(router.status(), Status::Default(Fault::Gone));
        assert_eq!(router.notice(lost), Some(Notice::Fallback(Fault::Gone)));
        assert_eq!(router.notice(lost), None, "told once");

        // The default is followed every second, the chosen device retried every three.
        assert_eq!(router.step(lost + 500 * MS), Step::Wait);
        assert_eq!(router.step(lost + DEFAULT_CHECK), Step::CheckDefault);
        router.checked(lost + DEFAULT_CHECK);
        assert_eq!(router.step(lost + DEFAULT_CHECK + MS), Step::Wait);
        let retry = lost + CHOSEN_RETRY;
        assert_eq!(router.step(retry), Step::Open(Target::Chosen));
        // Still busy: the default keeps playing and nothing new is said.
        assert!(run(&mut router, retry, Err(Fault::Busy), Ok(())).is_empty());
        assert_eq!(router.status(), Status::Default(Fault::Busy));

        let back = retry + CHOSEN_RETRY;
        assert_eq!(
            run(&mut router, back, Ok(()), Ok(())),
            [Notice::Returned],
            "told once on the way back"
        );
        assert_eq!(router.status(), Status::Chosen);
        assert_eq!(router.step(back + 60_000 * MS), Step::Wait);
    }

    #[test]
    fn a_format_change_reopens_the_chosen_device_without_a_notice() {
        let start = Instant::now();
        let mut router = Router::new(start);
        run(&mut router, start, Ok(()), Ok(()));
        let lost = start + 5_000 * MS;
        router.lost(Fault::Gone, lost);
        assert!(run(&mut router, lost, Ok(()), Ok(())).is_empty());
        assert_eq!(router.status(), Status::Chosen);
    }

    #[test]
    fn a_device_that_fails_right_after_opening_is_not_reopened_at_once() {
        let start = Instant::now();
        let mut router = Router::new(start);
        run(&mut router, start, Ok(()), Ok(()));
        let lost = start + 100 * MS;
        router.lost(Fault::Call("IAudioClient::GetCurrentPadding", -1), lost);
        assert_eq!(router.step(lost), Step::Open(Target::Default));
        router.opened(Target::Default, lost);
        assert_eq!(router.step(lost + CHOSEN_RETRY - MS), Step::CheckDefault);
        router.checked(lost + CHOSEN_RETRY - MS);
        assert_eq!(router.step(lost + CHOSEN_RETRY), Step::Open(Target::Chosen));
    }

    #[test]
    fn the_lost_default_is_reopened_and_the_fallback_is_not_announced_twice() {
        let start = Instant::now();
        let mut router = Router::new(start);
        assert_eq!(
            run(&mut router, start, Err(Fault::Gone), Ok(())),
            [Notice::Fallback(Fault::Gone)]
        );
        // The default endpoint itself went away two seconds later.
        let lost = start + 2_000 * MS;
        router.lost(Fault::Gone, lost);
        assert_eq!(router.step(lost), Step::Open(Target::Default));
        assert!(run(&mut router, lost, Err(Fault::Gone), Ok(())).is_empty());
        assert_eq!(router.status(), Status::Default(Fault::Gone));
    }

    #[test]
    fn silence_is_announced_after_a_few_seconds_and_then_the_recovery() {
        let start = Instant::now();
        let mut router = Router::new(start);
        let fault = Fault::Call("MMDeviceEnumerator", -2);
        assert!(run(&mut router, start, Err(Fault::Gone), Err(fault.clone())).is_empty());
        assert_eq!(router.status(), Status::Silent(fault.clone()));
        // Both are tried again, the default every second.
        assert_eq!(router.step(start + 500 * MS), Step::Wait);
        assert_eq!(
            router.step(start + DEFAULT_RETRY),
            Step::Open(Target::Default)
        );
        let before = start + SILENT_NOTICE - MS;
        assert!(run(&mut router, before, Err(Fault::Gone), Err(fault.clone())).is_empty());
        let after = start + SILENT_NOTICE;
        assert_eq!(router.notice(after), Some(Notice::Silent(fault.clone())));
        assert_eq!(router.notice(after + 10_000 * MS), None, "told once");

        // The default comes back first, the chosen device later.
        let default = after + 2_000 * MS;
        assert_eq!(
            run(&mut router, default, Err(Fault::Gone), Ok(())),
            [Notice::Fallback(Fault::Gone)]
        );
        let chosen = default + CHOSEN_RETRY;
        assert_eq!(run(&mut router, chosen, Ok(()), Ok(())), [Notice::Returned]);
    }

    #[test]
    fn known_failures_are_named_in_the_three_languages() {
        assert_eq!(Fault::from_call("GetDevice", E_NOTFOUND), Fault::Gone);
        assert_eq!(
            Fault::from_call(
                "IAudioRenderClient::GetBuffer",
                AUDCLNT_E_DEVICE_INVALIDATED
            ),
            Fault::Gone
        );
        assert_eq!(
            Fault::from_call("IAudioClient::Initialize", AUDCLNT_E_DEVICE_IN_USE),
            Fault::Busy
        );
        let other = Fault::from_call("IMMDevice::Activate", 0x8007_000E_u32 as i32);
        assert_eq!(
            other.text(Locale::EnUs),
            "Windows audio error in IMMDevice::Activate: 0x8007000E"
        );
        for fault in [Fault::Gone, Fault::Busy, other] {
            let texts = [Locale::PtBr, Locale::EnUs, Locale::EsEs].map(|l| fault.text(l));
            assert!(texts[0] != texts[1] && texts[1] != texts[2] && texts[0] != texts[2]);
        }
    }
}
