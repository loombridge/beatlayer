//! rtkit (`org.freedesktop.RealtimeKit1`): asks the system service to make
//! the calling thread real-time.
//!
//! rtkit grants `SCHED_RR` up to its `MaxRealtimePriority` (20 by default),
//! and only to a process whose `RLIMIT_RTTIME` hard limit is at most its
//! `RTTimeUSecMax` (200 ms by default): a real-time thread that runs that
//! long without sleeping is sent `SIGXCPU`, so a runaway thread can't freeze
//! the desktop. This module lowers the limit before asking.

use std::time::Duration;

use crate::dbus::{CallError, Connection, Writer};
use crate::thread::Refusal;

const SERVICE: &str = "org.freedesktop.RealtimeKit1";
const PATH: &str = "/org/freedesktop/RealtimeKit1";
const TIMEOUT: Duration = Duration::from_secs(2);

/// Asks rtkit to make the calling thread real-time at `wanted`, or at rtkit's
/// maximum if that's lower. Returns the priority granted.
#[cfg(target_os = "linux")]
pub(crate) fn make_current_thread_realtime(wanted: i32) -> Result<i32, Refusal> {
    let mut connection = Connection::open(&crate::dbus::system_bus_path(), TIMEOUT)
        .map_err(|e| refusal(&e, true))?;
    // SAFETY: `gettid` always succeeds.
    let tid = unsafe { libc::gettid() };
    request(
        &mut connection,
        u64::try_from(tid).unwrap_or(0),
        wanted,
        lower_rttime_limit,
    )
}

/// The conversation with rtkit, apart from the socket and the process limit,
/// so tests can run it against a fake bus.
fn request(
    connection: &mut Connection,
    thread: u64,
    wanted: i32,
    lower_rttime_limit: impl FnOnce(u64) -> Result<(), i32>,
) -> Result<i32, Refusal> {
    let max = connection
        .property(SERVICE, PATH, SERVICE, "MaxRealtimePriority")
        .map_err(|e| refusal(&e, false))?;
    let rttime = connection
        .property(SERVICE, PATH, SERVICE, "RTTimeUSecMax")
        .map_err(|e| refusal(&e, false))?;
    let level = wanted
        .min(i32::try_from(max.as_i64()).unwrap_or(i32::MAX))
        .max(1);
    let rttime = u64::try_from(rttime.as_i64()).unwrap_or(0);
    lower_rttime_limit(rttime).map_err(Refusal::Os)?;

    let mut body = Writer::default();
    body.u64(thread);
    body.u32(level.unsigned_abs());
    connection
        .call(
            SERVICE,
            PATH,
            SERVICE,
            "MakeThreadRealtime",
            "tu",
            &body.buf,
        )
        .map_err(|e| refusal(&e, false))?;
    Ok(level)
}

/// Sets the process's `RLIMIT_RTTIME` to at most `usec`, as rtkit requires.
#[cfg(target_os = "linux")]
fn lower_rttime_limit(usec: u64) -> Result<(), i32> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is writable.
    if unsafe { libc::getrlimit(libc::RLIMIT_RTTIME, &mut limit) } != 0 {
        return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(-1));
    }
    if limit.rlim_max != libc::RLIM_INFINITY && limit.rlim_max <= usec {
        return Ok(());
    }
    let lowered = libc::rlimit {
        rlim_cur: usec,
        rlim_max: usec,
    };
    // SAFETY: `lowered` is a valid limit, no higher than before.
    if unsafe { libc::setrlimit(libc::RLIMIT_RTTIME, &lowered) } != 0 {
        return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(-1));
    }
    Ok(())
}

fn refusal(error: &CallError, connecting: bool) -> Refusal {
    match error {
        CallError::Io(_) if connecting => Refusal::NoBus,
        CallError::Io(_) | CallError::Protocol(_) => Refusal::Protocol,
        CallError::Remote { name, .. }
            if name == "org.freedesktop.DBus.Error.ServiceUnknown"
                || name == "org.freedesktop.DBus.Error.NameHasNoOwner" =>
        {
            Refusal::NoRtkit
        }
        CallError::Remote { .. } => Refusal::RtkitDenied,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dbus::fake::{self, Answer};

    fn rtkit(deny: bool) -> impl FnMut(&crate::dbus::Message) -> Answer + Send + 'static {
        move |message| {
            let mut args = message.body();
            match message.header.member.as_deref() {
                Some("Get") => {
                    assert_eq!(args.string().unwrap(), SERVICE);
                    match args.string().unwrap().as_str() {
                        "MaxRealtimePriority" => Answer::Return {
                            signature: "v",
                            body: fake::variant_i32(20),
                        },
                        "RTTimeUSecMax" => Answer::Return {
                            signature: "v",
                            body: fake::variant_i64(200_000),
                        },
                        other => panic!("unexpected property {other}"),
                    }
                }
                Some("MakeThreadRealtime") if deny => Answer::Error {
                    name: "org.freedesktop.DBus.Error.AccessDenied",
                    message: "Operation not permitted",
                },
                Some("MakeThreadRealtime") => Answer::Return {
                    signature: "",
                    body: Vec::new(),
                },
                other => panic!("unexpected call {other:?}"),
            }
        }
    }

    #[test]
    fn rtkit_is_asked_for_the_thread_at_its_own_maximum() {
        let (client, bus) = fake::bus(rtkit(false));
        let mut connection = Connection::over(client, TIMEOUT).unwrap();
        let mut limit = None;
        let level = request(&mut connection, 4242, 70, |usec| {
            limit = Some(usec);
            Ok(())
        });
        assert_eq!(level, Ok(20));
        assert_eq!(limit, Some(200_000), "RLIMIT_RTTIME is lowered first");
        drop(connection);

        let calls = bus.join().unwrap();
        let call = calls.last().unwrap();
        assert_eq!(call.header.member.as_deref(), Some("MakeThreadRealtime"));
        assert_eq!(call.header.destination.as_deref(), Some(SERVICE));
        assert_eq!(call.header.path.as_deref(), Some(PATH));
        assert_eq!(call.header.signature, "tu");
        let mut args = call.body();
        assert_eq!(args.u64().unwrap(), 4242);
        assert_eq!(args.u32().unwrap(), 20);
    }

    #[test]
    fn a_refusal_from_rtkit_is_reported() {
        let (client, _bus) = fake::bus(rtkit(true));
        let mut connection = Connection::over(client, TIMEOUT).unwrap();
        let result = request(&mut connection, 1, 70, |_| Ok(()));
        assert_eq!(result, Err(Refusal::RtkitDenied));
    }

    #[test]
    fn a_missing_service_is_told_apart_from_a_refusal() {
        let (client, _bus) = fake::bus(|_| Answer::Error {
            name: "org.freedesktop.DBus.Error.ServiceUnknown",
            message: "The name org.freedesktop.RealtimeKit1 was not provided",
        });
        let mut connection = Connection::over(client, TIMEOUT).unwrap();
        let result = request(&mut connection, 1, 70, |_| Ok(()));
        assert_eq!(result, Err(Refusal::NoRtkit));
    }

    #[test]
    fn a_limit_that_cannot_be_lowered_stops_the_request() {
        let (client, _bus) = fake::bus(rtkit(false));
        let mut connection = Connection::over(client, TIMEOUT).unwrap();
        let result = request(&mut connection, 1, 70, |_| Err(1));
        assert_eq!(result, Err(Refusal::Os(1)));
    }
}
