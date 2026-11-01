//! The published playground (docs/playground/, served by GitHub Pages
//! from master) is a page that opens in playground mode, loads only what
//! is beside it, and may talk only to its own site and the Claude API.

use std::path::Path;

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn csp(html: &str) -> &str {
    let at = html
        .find("http-equiv=\"Content-Security-Policy\"")
        .expect("a CSP meta tag");
    let rest = &html[at..];
    let start = rest.find("content=\"").expect("content") + 9;
    let end = rest[start..].find('"').expect("closing quote");
    &rest[start..start + end]
}

fn check(html: &str) {
    assert!(html.contains("<canvas id=\"canvas\" data-mode=\"playground\">"));
    let csp = csp(html);
    for must in [
        "default-src 'self'",
        "connect-src 'self' https://api.anthropic.com;",
        "base-uri 'none'",
        "form-action 'none'",
        "object-src 'none'",
    ] {
        assert!(csp.contains(must), "CSP lacks {must}: {csp}");
    }
    assert!(!csp.contains("connect-src *") && !csp.contains("http:"), "{csp}");
}

#[test]
fn the_source_page_is_locked_to_the_api() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    check(&read(&root.join("playground.html")));
}

#[test]
fn the_published_page_is_built_and_complete() {
    const PUBLIC_URL: &str = "/playground/";
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/playground");
    let html = read(&dir.join("index.html"));
    check(&html);
    // trunk's loader, from the Pages path, and the files it names are there.
    assert!(
        !html.contains("/helmsman/playground/"),
        "the site is at the root of helmsman.advin.io"
    );
    let mut named = 0;
    for ext in [".js", "_bg.wasm"] {
        let at = html.find(ext).unwrap_or_else(|| panic!("no {ext} in the page"));
        let start = html[..at].rfind(PUBLIC_URL).expect("Pages public URL") + PUBLIC_URL.len();
        let file = &html[start..at + ext.len()];
        assert!(dir.join(file).is_file(), "{file} is missing from docs/playground");
        named += 1;
    }
    assert_eq!(named, 2);
    // Nothing else that could hold a key or a build of something else.
    for e in std::fs::read_dir(&dir).unwrap() {
        let name = e.unwrap().file_name().into_string().unwrap();
        assert!(
            name == "index.html"
                || name == "helmsman-mark-16-yellow.svg"
                || (name.starts_with("helmsman-canvas-")
                    && (Path::new(&name).extension().is_some_and(|x| x == "js")
                        || name.ends_with("_bg.wasm"))),
            "unexpected file {name}"
        );
    }
}
