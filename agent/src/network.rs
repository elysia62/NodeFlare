//! Bounded dual-stack connection attempts for the persistent reporting socket.
//! Prefer IPv4 without excluding IPv6-only hosts or letting one broken address
//! family consume the entire connection budget.

use std::collections::HashSet;
use std::io;
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

const FAMILY_FALLBACK_DELAY: Duration = Duration::from_millis(250);
const ADDRESS_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) fn connect_tcp(addresses: Vec<SocketAddr>, deadline: Instant) -> io::Result<TcpStream> {
    connect_with(addresses, deadline, |address, timeout| {
        TcpStream::connect_timeout(&address, timeout)
    })
}

struct CancelAttempts(Arc<AtomicBool>);

impl Drop for CancelAttempts {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "TCP connection timed out")
}

fn try_addresses<T>(
    addresses: Vec<SocketAddr>,
    deadline: Instant,
    cancelled: &AtomicBool,
    connect: &impl Fn(SocketAddr, Duration) -> io::Result<T>,
) -> io::Result<T> {
    let mut last_error =
        io::Error::new(io::ErrorKind::NotFound, "hostname resolved to no addresses");
    for address in addresses {
        if cancelled.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "connection attempt cancelled",
            ));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(timed_out());
        }
        match connect(address, remaining.min(ADDRESS_TIMEOUT)) {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

fn spawn_family<T: Send + 'static, F>(
    addresses: Vec<SocketAddr>,
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
    connect: Arc<F>,
    results: mpsc::SyncSender<io::Result<T>>,
) -> io::Result<()>
where
    F: Fn(SocketAddr, Duration) -> io::Result<T> + Send + Sync + 'static,
{
    thread::Builder::new()
        .name("nodeflare-connect".into())
        .spawn(move || {
            let result = try_addresses(addresses, deadline, &cancelled, &*connect);
            // An in-flight OS connect can finish after the other family wins, but
            // no further addresses are tried and its unused socket is dropped.
            if !cancelled.load(Ordering::Acquire) {
                let _ = results.send(result);
            }
        })?;
    Ok(())
}

fn connect_with<T: Send + 'static, F>(
    addresses: Vec<SocketAddr>,
    deadline: Instant,
    connect: F,
) -> io::Result<T>
where
    F: Fn(SocketAddr, Duration) -> io::Result<T> + Send + Sync + 'static,
{
    let mut seen = HashSet::new();
    let (v4, v6): (Vec<_>, Vec<_>) = addresses
        .into_iter()
        .filter(|address| seen.insert(*address))
        .partition(SocketAddr::is_ipv4);
    let cancelled = CancelAttempts(Arc::new(AtomicBool::new(false)));
    // No thread or artificial delay for literal IPs and single-family DNS.
    if v4.is_empty() || v6.is_empty() {
        return try_addresses(
            if v4.is_empty() { v6 } else { v4 },
            deadline,
            &cancelled.0,
            &connect,
        );
    }
    if Instant::now() >= deadline {
        return Err(timed_out());
    }

    let connect = Arc::new(connect);
    let (send, receive) = mpsc::sync_channel(2);
    spawn_family(
        v4,
        deadline,
        Arc::clone(&cancelled.0),
        Arc::clone(&connect),
        send.clone(),
    )?;
    let delay = FAMILY_FALLBACK_DELAY.min(deadline.saturating_duration_since(Instant::now()));
    let mut pending = 1;
    let mut last_error = timed_out();
    match receive.recv_timeout(delay) {
        Ok(Ok(stream)) => return Ok(stream),
        Ok(Err(error)) => {
            last_error = error;
            pending -= 1;
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {}
        Err(mpsc::RecvTimeoutError::Disconnected) => return Err(last_error),
    }
    if Instant::now() >= deadline {
        return Err(timed_out());
    }
    // IPv4 failed immediately, or has had a short head start. IPv6 now gets
    // its own attempts, not the leftovers after every IPv4 address times out.
    spawn_family(v6, deadline, Arc::clone(&cancelled.0), connect, send)?;
    pending += 1;
    while pending > 0 {
        match receive.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Ok(stream)) => return Ok(stream),
            Ok(Err(error)) => {
                last_error = error;
                pending -= 1;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => return Err(timed_out()),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    Err(last_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn v4() -> SocketAddr {
        "127.0.0.1:12345".parse().unwrap()
    }
    fn v6() -> SocketAddr {
        "[::1]:12345".parse().unwrap()
    }
    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(5)
    }

    #[test]
    fn ipv4_wins_even_when_dns_lists_ipv6_first() {
        assert_eq!(
            connect_with(vec![v6(), v4()], deadline(), |address, _| {
                assert!(
                    address.is_ipv4(),
                    "healthy IPv4 must not start an IPv6 connection"
                );
                Ok(address)
            })
            .unwrap(),
            v4()
        );
    }

    #[test]
    fn ipv4_route_failure_does_not_prevent_ipv6_only_connectivity() {
        assert_eq!(
            connect_with(vec![v4(), v6()], deadline(), |address, _| {
                if address.is_ipv4() {
                    Err(io::Error::new(
                        io::ErrorKind::NetworkUnreachable,
                        "no IPv4 route",
                    ))
                } else {
                    Ok(address)
                }
            })
            .unwrap(),
            v6()
        );
    }

    #[test]
    fn stalled_ipv4_does_not_hold_up_ipv6_or_start_more_attempts_after_success() {
        let (release, wait) = mpsc::channel();
        let wait = std::sync::Mutex::new(wait);
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = Arc::clone(&calls);
        let (finished, done) = mpsc::channel();
        let worker = thread::spawn(move || {
            finished
                .send(connect_with(
                    vec![v4(), "127.0.0.2:12345".parse().unwrap(), v6()],
                    deadline(),
                    move |address, _| {
                        observed.lock().unwrap().push(address);
                        if address.is_ipv4() {
                            wait.lock()
                                .unwrap()
                                .recv_timeout(Duration::from_secs(3))
                                .unwrap();
                            Err(io::Error::new(io::ErrorKind::TimedOut, "stalled IPv4"))
                        } else {
                            Ok(address)
                        }
                    },
                ))
                .unwrap();
        });
        let result = done.recv_timeout(Duration::from_secs(2));
        release.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(result.unwrap().unwrap(), v6());
        assert_eq!(*calls.lock().unwrap(), [v4(), v6()]);
    }

    #[test]
    fn unavailable_ipv6_does_not_cancel_an_in_flight_ipv4_connection() {
        let (release, wait) = mpsc::channel();
        let wait = std::sync::Mutex::new(wait);
        let connected = connect_with(vec![v6(), v4()], deadline(), move |address, _| {
            if address.is_ipv4() {
                wait.lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap();
                Ok(address)
            } else {
                release.send(()).unwrap();
                Err(io::Error::new(
                    io::ErrorKind::NetworkUnreachable,
                    "no IPv6 route",
                ))
            }
        })
        .unwrap();
        assert_eq!(connected, v4());
    }

    #[test]
    fn shared_deadline_stops_waiting_for_both_families() {
        let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let wait = Arc::clone(&release);
        let (finished, done) = mpsc::channel();
        let result = connect_with(
            vec![v4(), v6()],
            Instant::now() + Duration::from_millis(750),
            move |_, _| {
                let (flag, ready) = &*wait;
                let _ = ready
                    .wait_timeout_while(flag.lock().unwrap(), Duration::from_secs(3), |released| {
                        !*released
                    })
                    .unwrap();
                finished.send(()).unwrap();
                Err::<(), _>(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "simulated stalled connect",
                ))
            },
        );
        *release.0.lock().unwrap() = true;
        release.1.notify_all();
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        // Both finite workers exit even after their caller has timed out.
        done.recv_timeout(Duration::from_secs(3)).unwrap();
        done.recv_timeout(Duration::from_secs(3)).unwrap();
    }

    #[test]
    fn each_family_retries_distinct_addresses_and_keeps_its_last_error() {
        let attempts = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = Arc::clone(&attempts);
        let second: SocketAddr = "127.0.0.2:12345".parse().unwrap();
        let error = connect_with(
            vec![v4(), v4(), second],
            deadline(),
            move |address, timeout| {
                assert!(timeout <= ADDRESS_TIMEOUT);
                seen.lock().unwrap().push(address);
                Err::<(), _>(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    address.to_string(),
                ))
            },
        )
        .unwrap_err();
        assert_eq!(error.to_string(), second.to_string());
        assert_eq!(*attempts.lock().unwrap(), [v4(), second]);
    }

    #[test]
    fn expired_budget_and_empty_dns_results_do_not_dial() {
        for addresses in [vec![v4()], vec![v4(), v6()]] {
            let error = connect_with(addresses, Instant::now(), |_, _| -> io::Result<()> {
                panic!("past deadline")
            })
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        }
        assert_eq!(
            connect_with(Vec::new(), deadline(), |_, _| -> io::Result<()> {
                panic!("no addresses")
            })
            .unwrap_err()
            .kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn single_family_ipv6_is_never_filtered_out() {
        assert_eq!(
            connect_with(vec![v6()], deadline(), |address, _| Ok(address)).unwrap(),
            v6()
        );
    }

    #[test]
    fn real_ipv4_listener_works_with_unavailable_ipv6_in_dns() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let stream = connect_tcp(vec!["[::1]:0".parse().unwrap(), address], deadline()).unwrap();
        assert_eq!(stream.peer_addr().unwrap(), address);
    }

    #[test]
    fn real_ipv6_listener_works_when_ipv4_is_unavailable() {
        let listener = match TcpListener::bind("[::1]:0") {
            Ok(listener) => listener,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::AddrNotAvailable | io::ErrorKind::Unsupported
                ) || matches!(error.raw_os_error(), Some(47 | 97 | 10047)) =>
            {
                return;
            }
            Err(error) => panic!("IPv6 loopback: {error}"),
        };
        let address = listener.local_addr().unwrap();
        let stream =
            connect_tcp(vec!["127.0.0.1:0".parse().unwrap(), address], deadline()).unwrap();
        assert_eq!(stream.peer_addr().unwrap(), address);
    }
}
