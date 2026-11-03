//! Fetching from a marketplace index over HTTP: the index and its
//! signature, then one bundle file, each checked before the next step
//! ([`super::index`]). Used by `helmsman-plc bundle fetch` on a laptop and
//! by the box API (`POST /api/market/list`, `POST /api/market/install`).
//!
//! The box fetches a URL a client gave it, so the host is checked first:
//! loopback, private, link-local, shared, multicast and unspecified
//! addresses are refused unless the caller allows local hosts (a test, a
//! LAN mirror behind `--market-allow-local`). The check is on the resolved
//! addresses at fetch time; a name that resolves differently a moment
//! later is not caught (DNS rebinding), which is why the flag is off by
//! default on a box. Every body is capped before it is read.

use std::net::{IpAddr, ToSocketAddrs};
use std::time::Duration;

use helmsman_agent::http::{self, Timeouts, Url};

use super::index::{self, Entry, Index, MAX_INDEX, MAX_SIG};
use super::{MAX_BUNDLE, TrustStore};

/// How long one fetch may take.
const TIMEOUTS: Timeouts = Timeouts {
    connect: Duration::from_secs(10),
    idle: Duration::from_secs(15),
    total: Duration::from_secs(60),
};

/// A verified index and where it came from.
#[derive(Debug, Clone)]
pub struct Listing {
    pub url: Url,
    /// The publisher the box's trust store names for the signing key.
    pub publisher: String,
    pub index: Index,
}

/// Whether every address `url` resolves to is a public one.
///
/// # Errors
/// The first address that is not, or a host that does not resolve.
pub fn host_is_public(url: &Url) -> Result<(), String> {
    let addrs = (url.host.as_str(), url.port)
        .to_socket_addrs()
        .map_err(|e| format!("{}: cannot resolve: {}", url.host, e.kind()))?;
    let mut any = false;
    for a in addrs {
        any = true;
        let ip = a.ip();
        let local = match ip {
            IpAddr::V4(v4) => {
                v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.is_unspecified()
                    || v4.is_multicast()
                    || v4.is_broadcast()
                    || v4.is_documentation()
                    // 100.64.0.0/10, carrier-grade NAT
                    || (v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1]))
                    // 0.0.0.0/8
                    || v4.octets()[0] == 0
            }
            IpAddr::V6(v6) => {
                v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_multicast()
                    // fc00::/7 unique local, fe80::/10 link local
                    || (v6.segments()[0] & 0xfe00) == 0xfc00
                    || (v6.segments()[0] & 0xffc0) == 0xfe80
                    || v6.to_ipv4_mapped().is_some_and(|m| {
                        m.is_loopback() || m.is_private() || m.is_link_local() || m.is_unspecified()
                    })
            }
        };
        if local {
            return Err(format!(
                "{} resolves to {ip}, a local address; a marketplace is a public site (--market-allow-local lifts this on a bench)",
                url.host
            ));
        }
    }
    if !any {
        return Err(format!("{} resolves to no address", url.host));
    }
    Ok(())
}

fn get(url: &Url, cap: usize, what: &str) -> Result<Vec<u8>, String> {
    let req = http::Request {
        method: "GET",
        path: if url.prefix.is_empty() { "/" } else { &url.prefix },
        headers: &[("Accept", "*/*")],
        json: None,
    };
    let r = http::send(url, &req, TIMEOUTS).map_err(|e| format!("{what}: {url}: {e:?}"))?;
    if r.status != 200 {
        return Err(format!("{what}: {url} answered {}", r.status));
    }
    r.read_body(cap)
        .map_err(|e| format!("{what}: {url}: {e:?} (at most {cap} bytes)"))
}

/// The URL of `file` beside the index at `index_url`.
///
/// # Errors
/// An index URL with no file part.
pub fn file_url(index_url: &Url, file: &str) -> Result<Url, String> {
    if !index::file_name_ok(file) {
        return Err(format!("{file}: not a bundle file name"));
    }
    let dir = match index_url.prefix.rsplit_once('/') {
        Some((dir, _)) => dir,
        None => "",
    };
    Ok(Url {
        https: index_url.https,
        host: index_url.host.clone(),
        port: index_url.port,
        prefix: format!("{dir}/{file}"),
    })
}

/// Fetches and verifies the index at `url` (`index.json`; its signature at
/// `index.json.sig`).
///
/// # Errors
/// A host that is not public (unless `allow_local`), a fetch that fails or
/// overruns its cap, an index that does not decode, a signature that does
/// not verify or whose key the trust store lacks.
pub fn list(url: &str, trust: &TrustStore, allow_local: bool) -> Result<Listing, String> {
    let url = Url::parse(url)?;
    if url.prefix.is_empty() || url.prefix.ends_with('/') {
        return Err("the index URL names the index file, like https://example.com/market/index.json".into());
    }
    if !allow_local {
        host_is_public(&url)?;
    }
    let bytes = get(&url, MAX_INDEX, "index")?;
    let sig_url = Url {
        prefix: format!("{}.sig", url.prefix),
        ..url.clone()
    };
    let sig = get(&sig_url, MAX_SIG, "index signature")?;
    let sig = String::from_utf8(sig).map_err(|_| "index signature is not text".to_string())?;
    let publisher = index::verify_signature(&bytes, &sig, trust).map_err(|e| e.0)?;
    let index = Index::decode(&bytes).map_err(|e| e.0)?;
    if index.publisher != publisher {
        return Err(format!(
            "the index says it is {}'s, the signing key is {publisher}'s",
            index.publisher
        ));
    }
    Ok(Listing {
        url,
        publisher,
        index,
    })
}

/// Fetches the bundle `entry` names from the listing's site and checks it
/// against the entry (size, SHA-256). The bytes still have to pass
/// [`super::verify`] on the box: this only proves they are the file the
/// signed index named.
///
/// # Errors
/// A fetch that fails or overruns, or a file that is not the one named.
pub fn fetch(listing: &Listing, entry: &Entry, allow_local: bool) -> Result<Vec<u8>, String> {
    let url = file_url(&listing.url, &entry.file)?;
    if !allow_local {
        host_is_public(&url)?;
    }
    let cap = usize::try_from(entry.size).unwrap_or(MAX_BUNDLE).min(MAX_BUNDLE);
    let bytes = get(&url, cap, "bundle")?;
    entry.check_file(&bytes).map_err(|e| e.0)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_hosts_are_refused_by_default() {
        for u in [
            "http://127.0.0.1:8000/index.json",
            "http://localhost:8000/index.json",
            "http://10.0.0.5/index.json",
            "http://192.168.1.2/index.json",
            "http://169.254.169.254/latest/meta-data",
            "http://100.64.0.1/index.json",
            "http://0.0.0.0/index.json",
            "http://[::1]:8000/index.json",
            "http://[fe80::1]:8000/index.json",
            "http://[::ffff:127.0.0.1]:8000/index.json",
        ] {
            let url = Url::parse(u).unwrap();
            assert!(host_is_public(&url).is_err(), "{u}");
        }
    }

    #[test]
    fn file_urls_sit_beside_the_index() {
        let idx = Url::parse("https://example.com/market/index.json").unwrap();
        assert_eq!(
            file_url(&idx, "a-1.0.hbundle").unwrap().to_string(),
            "https://example.com/market/a-1.0.hbundle"
        );
        let root = Url::parse("https://example.com/index.json").unwrap();
        assert_eq!(
            file_url(&root, "a-1.0.hbundle").unwrap().to_string(),
            "https://example.com/a-1.0.hbundle"
        );
        assert!(file_url(&idx, "../a.hbundle").is_err());
        assert!(file_url(&idx, "a.zip").is_err());
    }

    #[test]
    fn an_index_url_names_a_file() {
        let t = TrustStore::default();
        assert!(list("https://example.com/", &t, true).is_err());
        assert!(list("https://example.com", &t, true).is_err());
        assert!(list("ftp://example.com/index.json", &t, true).is_err());
    }
}
