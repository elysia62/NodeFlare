use super::*;
use std::sync::atomic::AtomicUsize;

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "public IP probe did not make progress"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn public_ip_clients_constrain_the_requested_family_and_total_timeout() {
    for (ipv6, family) in [
        (false, ureq::config::IpFamily::Ipv4Only),
        (true, ureq::config::IpFamily::Ipv6Only),
    ] {
        let client = public_ip_client(ipv6);
        assert_eq!(client.config().ip_family(), family);
        assert_eq!(
            client.config().timeouts().global,
            Some(PUBLIC_IP_PROBE_TIMEOUT)
        );
    }
}

#[test]
fn either_family_is_published_without_waiting_for_the_other() {
    for fast_ipv6 in [false, true] {
        let probe = PublicIpProbe::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let worker_calls = Arc::clone(&calls);
        let (release, released) = mpsc::channel();
        let released = Mutex::new(released);
        probe.refresh_with(move |url, ipv6| {
            worker_calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                url,
                if ipv6 {
                    PUBLIC_IP_V6_URL
                } else {
                    PUBLIC_IP_V4_URL
                }
            );
            if ipv6 != fast_ipv6 {
                // Bound the test worker even if an assertion in the calling
                // thread fails before releasing it.
                released
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .ok()?;
            }
            Some(
                if ipv6 { "2001:db8::7" } else { "203.0.113.7" }
                    .parse()
                    .unwrap(),
            )
        });
        wait_until(|| {
            if fast_ipv6 {
                probe.v6().is_some()
            } else {
                probe.v4().is_some()
            }
        });
        assert!(probe.probing.load(Ordering::SeqCst));
        assert!(if fast_ipv6 {
            probe.v4().is_none()
        } else {
            probe.v6().is_none()
        });
        // A slow family must not permit overlapping refreshes.
        probe.refresh_with(|_, _| panic!("overlapping public IP refresh"));
        release.send(()).unwrap();
        wait_until(|| !probe.probing.load(Ordering::SeqCst));
        assert_eq!(probe.v4().as_deref(), Some("203.0.113.7"));
        assert_eq!(probe.v6().as_deref(), Some("2001:db8::7"));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        // Finishing early does not bypass the normal ten-minute refresh cadence.
        probe.refresh_with(|_, _| panic!("public IP refresh was not throttled"));
        assert!(!probe.probing.load(Ordering::SeqCst));
    }
}

#[test]
fn failed_family_keeps_recent_address_but_expires_stale_address() {
    let probe = PublicIpProbe::default();
    {
        let mut state = probe.state.lock().unwrap();
        state
            .v4
            .observe(Some("203.0.113.7".parse().unwrap()), Instant::now());
        state.v6.observe(
            Some("2001:db8::7".parse().unwrap()),
            Instant::now() - PUBLIC_IP_STALE_AFTER - Duration::from_secs(1),
        );
    }
    probe.refresh_with(|_, _| None);
    wait_until(|| !probe.probing.load(Ordering::SeqCst));
    assert_eq!(probe.v4().as_deref(), Some("203.0.113.7"));
    assert!(probe.v6().is_none());
}
