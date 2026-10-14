//! Outbound HTTP shared by the widget fetch proxy and add-ins: agents whose resolver refuses this
//! machine unless the request explicitly names it.
//!
//! Permission checks compare URL host *names*. A name can still lead to this machine: `0.0.0.0`,
//! numeric forms such as `2130706433`, or a public DNS name that resolves to `127.0.0.1`. So the
//! final check is made on the resolved addresses, at connection time (which also covers DNS
//! answers that change after the name was checked): [`agent`] with `allow_local = false` drops
//! loopback and unspecified addresses and fails if none are left.

use std::io;
use std::net::IpAddr;

use ureq::config::Config;
use ureq::http::Uri;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

/// Whether a URL host is written as this machine (`localhost`, `127.*`, `::1`). Permissions must
/// name such hosts explicitly, with a port.
#[must_use]
pub fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']);
    host.eq_ignore_ascii_case("localhost") || host.starts_with("127.") || host == "::1"
}

/// Whether connecting to `ip` reaches this machine: loopback, or the unspecified address (which
/// connects to the local host), in IPv4, IPv6 or IPv4-mapped IPv6 form.
#[must_use]
pub fn is_local_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_unspecified(),
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6
                    .to_ipv4_mapped()
                    .is_some_and(|v4| v4.is_loopback() || v4.is_unspecified())
        }
    }
}

/// Resolves normally, then drops addresses that would reach this machine.
#[derive(Debug, Default)]
struct PublicResolver(DefaultResolver);

impl Resolver for PublicResolver {
    fn resolve(
        &self,
        uri: &Uri,
        config: &Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let all = self.0.resolve(uri, config, timeout)?;
        let mut public = self.empty();
        for addr in all.iter().filter(|a| !is_local_address(a.ip())) {
            public.push(*addr);
        }
        if public.is_empty() {
            let host = uri.host().unwrap_or("?");
            return Err(ureq::Error::Io(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "{host} resolves to this machine; to reach a local service, declare localhost or 127.0.0.1 with its port"
                ),
            )));
        }
        Ok(public)
    }
}

/// An agent with `config`. Unless `allow_local`, it refuses to connect to this machine however the
/// URL spells the host. Pass `allow_local = true` only for a URL whose host
/// [`is_loopback_host`] and that a permission names explicitly.
#[must_use]
pub fn agent(config: Config, allow_local: bool) -> ureq::Agent {
    if allow_local {
        ureq::Agent::with_parts(
            config,
            DefaultConnector::default(),
            DefaultResolver::default(),
        )
    } else {
        ureq::Agent::with_parts(
            config,
            DefaultConnector::default(),
            PublicResolver::default(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr};

    use super::*;

    #[test]
    fn local_addresses() {
        let local = [
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V4(Ipv4Addr::new(127, 9, 9, 9)),
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            IpAddr::V6(Ipv4Addr::LOCALHOST.to_ipv6_mapped()),
        ];
        assert!(local.into_iter().all(is_local_address));
        let public = [
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)),
            IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
            IpAddr::V6("2606:4700::1111".parse().unwrap()),
        ];
        assert!(!public.into_iter().any(is_local_address));
    }

    #[test]
    fn loopback_host_names() {
        for h in [
            "localhost",
            "LOCALHOST",
            "127.0.0.1",
            "127.1",
            "::1",
            "[::1]",
        ] {
            assert!(is_loopback_host(h), "{h}");
        }
        for h in ["0.0.0.0", "localtest.me", "2130706433", "example.com"] {
            assert!(!is_loopback_host(h), "{h}");
        }
    }

    /// Serve one HTTP 200 on 127.0.0.1 and return the port.
    fn stub() -> u16 {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(4) {
                let mut s = stream.unwrap();
                let _ = s.read(&mut [0_u8; 2048]);
                let _ = s.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                );
            }
        });
        port
    }

    fn get(allow_local: bool, url: &str) -> Result<String, String> {
        let config = ureq::Agent::config_builder().max_redirects(0).build();
        agent(config, allow_local)
            .get(url)
            .call()
            .map_err(|e| e.to_string())?
            .body_mut()
            .read_to_string()
            .map_err(|e| e.to_string())
    }

    #[test]
    fn public_agent_refuses_this_machine_in_any_spelling() {
        let port = stub();
        for host in ["127.0.0.1", "0.0.0.0", "2130706433", "localhost"] {
            let err = get(false, &format!("http://{host}:{port}/")).unwrap_err();
            assert!(err.contains("resolves to this machine"), "{host}: {err}");
        }
        assert_eq!(
            get(true, &format!("http://127.0.0.1:{port}/")),
            Ok("ok".into())
        );
    }

    /// A public DNS name that resolves to 127.0.0.1 (`cargo test -- --ignored`).
    #[test]
    #[ignore = "needs DNS"]
    fn public_agent_refuses_names_that_resolve_here() {
        let port = stub();
        let err = get(false, &format!("http://localtest.me:{port}/")).unwrap_err();
        assert!(err.contains("resolves to this machine"), "{err}");
    }
}
