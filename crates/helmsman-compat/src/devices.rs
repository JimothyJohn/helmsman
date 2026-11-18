//! helmsman ↔ helmsman-devices. A box takes the library from
//! `index.json` at the root of the devices repository (the raw file on
//! its `main` branch, `helmsman_project::devlib::DEFAULT_URL`, so a
//! devices merge to `main` is live at once) and fetches each entry's map,
//! profile and record from beside it. Here a box at one ref is pointed
//! (`--devices-index`) at a devices checkout at another, served on
//! loopback, and asked for every entry the way the Devices tab asks:
//!
//! - by text (`?q=vendor model`): the entry is listed, and its map lines,
//!   profile and record each read on this box (the box answers
//!   `lines_unavailable`, `profile_unavailable` or `device_unavailable`,
//!   in words, for one it cannot);
//! - by EtherNet/IP identity (`?vendor=&product=`) when the entry has one:
//!   what a scan finds is matched to it;
//! - by `?wired=` for a record (a sensor a map line names).
//!
//! An index the box cannot read at all is one failure (`unavailable`).

use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::Path;

use serde_json::Value as Json;

use crate::boxp::BoxProc;
use crate::client;
use crate::report::Finding;
use crate::web;

/// Runs the contract: the box `bin` (of checkout `helmsman`) against the
/// devices checkout `devices`.
///
/// # Errors
/// The index cannot be read here, or the box or the server cannot start.
pub fn check(bin: &Path, helmsman: &Path, devices: &Path, work: &Path) -> Result<Vec<Finding>, String> {
    let index_text = std::fs::read_to_string(devices.join("index.json"))
        .map_err(|e| format!("devices index.json: {e}"))?;
    let index: Json = serde_json::from_str(&index_text).map_err(|e| format!("devices index.json: {e}"))?;
    let entries = index["devices"].as_array().cloned().unwrap_or_default();
    let server = web::serve_dir(devices.to_path_buf()).map_err(|e| e.to_string())?;
    let extra: Vec<OsString> = vec!["--devices-index".into(), server.url("/index.json").into()];
    let b = BoxProc::start(bin, helmsman, work, &extra)?;
    let ask = |query: &str| -> Result<Json, String> {
        let (status, body) = client::get(&b.url, &format!("/api/devices/library?{query}"), Some(&b.token))?;
        if status == 200 {
            Ok(body)
        } else {
            Err(format!("{status} {body}"))
        }
    };

    let mut out = Vec::new();
    if entries.is_empty() {
        out.push(Finding::fail("devices/index", "the index lists no devices"));
        return Ok(out);
    }
    for e in &entries {
        let folder = e["folder"].as_str().unwrap_or("?");
        let answer = ask(&format!("q={}", encode(&search_text(e))));
        let answer = match answer {
            Ok(a) => a,
            Err(why) => {
                out.push(Finding::fail(folder, format!("library by text: {why}")));
                continue;
            }
        };
        if let Some(why) = answer["unavailable"].as_str() {
            out.push(Finding::fail(
                "devices/index",
                format!("the box cannot read the library's index: {why}"),
            ));
            return Ok(out);
        }
        out.extend(judge_entry(e, &answer));
        if let Some((v, p)) = enip_identity(e) {
            out.push(match ask(&format!("vendor={v}&product={p}")) {
                Ok(a) if find(&a, folder).is_some() => Finding::pass(
                    format!("{folder} identity"),
                    format!("vendor {v} product {p} finds it"),
                ),
                Ok(a) => Finding::fail(
                    format!("{folder} identity"),
                    format!("vendor {v} product {p} does not find it: {}", short(&a)),
                ),
                Err(why) => Finding::fail(format!("{folder} identity"), why),
            });
        }
        if e.get("record").is_some_and(|r| !r.is_null()) {
            let wired = folder.trim_start_matches("devices/");
            out.push(match ask(&format!("wired={}", encode(wired))) {
                Ok(a) => match find(&a, folder) {
                    Some(m) if m.get("device").is_some() => Finding::pass(
                        format!("{folder} wired"),
                        format!("wired={wired} reads its record"),
                    ),
                    Some(m) => Finding::fail(
                        format!("{folder} wired"),
                        format!(
                            "wired={wired}: {}",
                            m["device_unavailable"]
                                .as_str()
                                .unwrap_or("no record in the answer")
                        ),
                    ),
                    None => Finding::fail(
                        format!("{folder} wired"),
                        format!("wired={wired} does not find it: {}", short(&a)),
                    ),
                },
                Err(why) => Finding::fail(format!("{folder} wired"), why),
            });
        }
    }
    drop(b);
    Ok(out)
}

/// What the Devices tab would type for an entry: its vendor and the first
/// word of its model (at most 120 characters reach the box's search).
#[must_use]
pub fn search_text(e: &Json) -> String {
    let vendor = e["vendor"].as_str().unwrap_or_default();
    let model = e["model"].as_str().unwrap_or_default();
    let first = model
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_end_matches(',');
    format!("{vendor} {first}").trim().to_string()
}

/// The entry's EtherNet/IP vendor id and product code, if it has them.
#[must_use]
pub fn enip_identity(e: &Json) -> Option<(u64, u64)> {
    let id = e.get("identity")?.get("enip")?;
    Some((id.get("vendor_id")?.as_u64()?, id.get("product_code")?.as_u64()?))
}

/// The match for `folder` in a library answer.
#[must_use]
pub fn find<'a>(answer: &'a Json, folder: &str) -> Option<&'a Json> {
    answer["matches"]
        .as_array()?
        .iter()
        .find(|m| m["folder"] == folder)
}

/// One entry's text search answer, held to what the index promises: the
/// entry is there, and each file it names reads on the box.
#[must_use]
pub fn judge_entry(e: &Json, answer: &Json) -> Vec<Finding> {
    let folder = e["folder"].as_str().unwrap_or("?");
    let Some(m) = find(answer, folder) else {
        return vec![Finding::fail(
            folder,
            format!(
                "the box does not list it for {:?}: {}",
                search_text(e),
                short(answer)
            ),
        )];
    };
    let mut out = Vec::new();
    let mut fine = Vec::new();
    for (key, what) in [
        ("lines_unavailable", "map"),
        ("profile_unavailable", "profile"),
        ("device_unavailable", "record"),
    ] {
        if let Some(why) = m[key].as_str() {
            out.push(Finding::fail(format!("{folder} {what}"), why.to_string()));
        }
    }
    if !m["map"].is_null() && m["lines"].as_str().is_none() && m["lines_unavailable"].is_null() {
        out.push(Finding::fail(
            format!("{folder} map"),
            "the box names a map and serves no lines",
        ));
    } else if m["lines"].is_string() {
        fine.push("map");
    }
    if e.get("profile").is_some_and(Json::is_string) && m["profile_text"].as_str().is_none() && out.is_empty()
    {
        out.push(Finding::fail(
            format!("{folder} profile"),
            "the index names a profile the box does not serve",
        ));
    } else if m["profile_text"].is_string() {
        fine.push("profile");
    }
    if out.is_empty() {
        let what = if fine.is_empty() {
            "listed".to_string()
        } else {
            format!("listed, {} read", fine.join(" and "))
        };
        out.push(Finding::pass(folder, what));
    }
    out
}

fn short(j: &Json) -> String {
    let s = j.to_string();
    if s.len() > 200 {
        format!("{}…", s.chars().take(200).collect::<String>())
    } else {
        s
    }
}

/// Percent-encodes a query value.
#[must_use]
pub fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry() -> Json {
        json!({ "folder": "devices/moxa/iologik-e1213", "vendor": "Moxa", "model": "ioLogik E1213",
                "profile": "moxa-iologik-e1213.device",
                "identity": { "enip": { "vendor_id": 991, "product_code": 515 } } })
    }

    #[test]
    fn search_text_is_vendor_and_first_model_word() {
        assert_eq!(search_text(&entry()), "Moxa ioLogik");
        let ifm = json!({ "vendor": "ifm", "model": "IF6123, IF6124, IG6615" });
        assert_eq!(search_text(&ifm), "ifm IF6123");
        assert_eq!(enip_identity(&entry()), Some((991, 515)));
        assert_eq!(enip_identity(&ifm), None);
    }

    #[test]
    fn an_entry_the_box_does_not_list_fails() {
        let f = judge_entry(
            &entry(),
            &json!({ "matches": [ { "folder": "devices/other/x" } ] }),
        );
        assert_eq!(f.len(), 1);
        assert!(!f[0].ok && f[0].detail.contains("does not list it"), "{f:?}");
    }

    #[test]
    fn each_unreadable_file_is_its_own_failure() {
        let answer = json!({ "matches": [ { "folder": "devices/moxa/iologik-e1213", "map": "enip.map",
            "lines_unavailable": "404", "profile_unavailable": "does not read as a profile: line 3" } ] });
        let f = judge_entry(&entry(), &answer);
        let failed: Vec<&str> = f.iter().filter(|f| !f.ok).map(|f| f.check.as_str()).collect();
        assert_eq!(
            failed,
            [
                "devices/moxa/iologik-e1213 map",
                "devices/moxa/iologik-e1213 profile"
            ],
            "{f:?}"
        );
    }

    #[test]
    fn a_profile_silently_missing_from_the_answer_fails() {
        let answer = json!({ "matches": [ { "folder": "devices/moxa/iologik-e1213" } ] });
        let f = judge_entry(&entry(), &answer);
        assert!(f.iter().any(|f| !f.ok && f.check.ends_with("profile")), "{f:?}");
    }

    #[test]
    fn a_map_named_without_lines_fails() {
        let e = json!({ "folder": "devices/a/b", "vendor": "a", "model": "b" });
        let answer = json!({ "matches": [ { "folder": "devices/a/b", "map": "enip.map" } ] });
        assert!(!judge_entry(&e, &answer)[0].ok);
    }

    #[test]
    fn a_whole_answer_passes_once() {
        let answer = json!({ "matches": [ { "folder": "devices/moxa/iologik-e1213", "map": "enip.map",
            "lines": "dev x\n", "profile_text": "helmsman-device 1\n" } ] });
        let f = judge_entry(&entry(), &answer);
        assert_eq!(
            f,
            [Finding::pass(
                "devices/moxa/iologik-e1213",
                "listed, map and profile read"
            )]
        );
    }

    #[test]
    fn encode_escapes_what_a_query_cannot_carry() {
        assert_eq!(encode("Moxa ioLogik&x=1"), "Moxa%20ioLogik%26x%3D1");
        assert_eq!(encode("ifm/if6123"), "ifm/if6123");
    }
}
