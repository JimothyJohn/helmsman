//! The published playground (docs/playground/, served by GitHub Pages
//! from master) is a page that opens in playground mode, loads only what
//! is beside it, and may talk only to its own site and one other origin:
//! the relay's demo function when it is built for the free chat, else the
//! Claude API for a visitor's own key.

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

/// The model API, for a page that takes the visitor's own key.
const MODEL_API: &str = "https://api.anthropic.com";

/// Whether `origin` is a function URL of the relay's demo (the free chat):
/// `https://<label>.lambda-url.<region>.on.aws`, nothing after it.
fn is_free_chat(origin: &str) -> bool {
    origin
        .strip_prefix("https://")
        .and_then(|host| host.split_once(".lambda-url."))
        .is_some_and(|(label, rest)| {
            let plain = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
            plain(label) && rest.strip_suffix(".on.aws").is_some_and(plain)
        })
}

/// The page may connect to itself and to exactly one other origin, which
/// `other` accepts.
fn check(html: &str, other: impl Fn(&str) -> bool) {
    assert!(html.contains("<canvas id=\"canvas\" data-mode=\"playground\">"));
    let csp = csp(html);
    for must in [
        "default-src 'self'",
        "base-uri 'none'",
        "form-action 'none'",
        "object-src 'none'",
    ] {
        assert!(csp.contains(must), "CSP lacks {must}: {csp}");
    }
    let connect: Vec<&str> = csp
        .split(';')
        .map(str::trim)
        .filter(|d| d.starts_with("connect-src"))
        .collect();
    assert_eq!(connect.len(), 1, "one connect-src: {csp}");
    let sources: Vec<&str> = connect[0].split_whitespace().skip(1).collect();
    assert_eq!(sources.len(), 2, "itself and one origin: {csp}");
    assert_eq!(sources[0], "'self'", "{csp}");
    assert!(
        other(sources[1]),
        "connect-src may not name {}: {csp}",
        sources[1]
    );
    assert!(!csp.contains('*') && !csp.contains("http:"), "{csp}");
}

#[test]
fn only_a_demo_function_url_passes_for_the_free_chat() {
    for ok in [
        "https://iup63z6bv5rz2fwtwexyw3pvoi0spenq.lambda-url.us-east-1.on.aws",
        "https://abc.lambda-url.eu-west-1.on.aws",
    ] {
        assert!(is_free_chat(ok), "{ok}");
    }
    for bad in [
        "",
        "*",
        "https://*",
        "https://*.lambda-url.us-east-1.on.aws",
        "http://abc.lambda-url.us-east-1.on.aws",
        "https://abc.lambda-url.us-east-1.on.aws/",
        "https://abc.lambda-url.us-east-1.on.aws.evil.example",
        "https://evil.example/abc.lambda-url.us-east-1.on.aws",
        "https://abc.lambda-url..on.aws",
        "https://.lambda-url.us-east-1.on.aws",
        "https://a.b.lambda-url.us-east-1.on.aws",
        "https://evil.example",
        MODEL_API,
        "'self'",
    ] {
        assert!(!is_free_chat(bad), "{bad}");
    }
}

/// A page's source brings in the loading screen, before the loader.
fn check_loading_source(html: &str) {
    let script = html
        .find("href=\"loading/loading.js\"")
        .expect("the screen's script");
    let loader = html.find("rel=\"rust\"").expect("the loader");
    assert!(script < loader, "the script must hear the loader fail");
    assert!(html.contains("data-initializer=\"loading/helmsman-canvas-initializer.js\""));
    assert!(html.contains("href=\"loading/loading.html\""));
    assert!(html.contains("<noscript>"));
}

/// The screen made it into a built page, and the bar's total is the real
/// size of the file: trunk's own number is from before wasm-opt.
fn check_built(html: &str, dir: &Path) {
    assert!(!html.contains("data-trunk"), "trunk left a link unprocessed");
    for must in [
        "id=\"loading-bar\"",
        "id=\"loading-failed\"",
        "#FFC20E",
        "helmsmanLoading",
    ] {
        assert!(html.contains(must), "the built page lacks {must}");
    }
    let screen = html.find("window.helmsmanLoading =").unwrap();
    let loader = html
        .find("__trunkInitializer(init,")
        .expect("the initializer call");
    assert!(screen < loader, "the screen runs first");

    let call = &html[loader..];
    let at = call.find("_bg.wasm', ").expect("the size") + "_bg.wasm', ".len();
    let size: u64 = call[at..at + call[at..].find(',').unwrap()].parse().unwrap();
    let wasm = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.to_string_lossy().ends_with("_bg.wasm"))
        .unwrap();
    assert_eq!(
        size,
        std::fs::metadata(wasm).unwrap().len(),
        "run patch-wasm-size.sh"
    );
}

fn loading_file(name: &str) -> String {
    read(&Path::new(env!("CARGO_MANIFEST_DIR")).join("loading").join(name))
}

#[test]
fn both_pages_load_with_the_screen() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    check_loading_source(&read(&root.join("playground.html")));
    check_loading_source(&read(&root.join("index.html")));
}

#[test]
fn the_screen_knows_every_failure_and_says_it_plainly() {
    let js = loading_file("loading.js");
    for must in [
        "addEventListener('error'",
        "addEventListener('unhandledrejection'",
        "addEventListener('helmsman-started'",
        "addEventListener('helmsman-start-failed'",
        "typeof WebAssembly",
        "load:",
        "start:",
        "unsupported:",
        "stalled:",
    ] {
        assert!(js.contains(must), "loading.js lacks {must}");
    }
    // The box finds inline scripts by these strings to hash them into its CSP.
    let html = loading_file("loading.html");
    for text in [&js, &html] {
        let lower = text.to_lowercase();
        assert!(!lower.contains("<script") && !lower.contains("</script"));
    }
    // The reader is a plant engineer.
    let words = format!("{js}{html}").to_lowercase();
    for never in ["rust", "crate", "raspberry", "anthropic", "panic"] {
        assert!(!words.contains(never), "{never} in the loading screen");
    }
}

#[test]
fn the_app_tells_the_screen_how_starting_went() {
    let main = read(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"));
    let js = loading_file("loading.js");
    for event in ["helmsman-started", "helmsman-start-failed"] {
        assert!(
            main.contains(&format!("\"{event}\"")),
            "main.rs does not send {event}"
        );
        assert!(
            js.contains(&format!("'{event}'")),
            "loading.js does not hear {event}"
        );
    }
}

#[test]
fn the_source_page_is_locked_to_the_api() {
    // The source names the model API; a build for the free chat replaces
    // it with the relay's origin (scripts/build-playground.sh, which stops
    // if this line is not the one it knows).
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    check(&read(&root.join("playground.html")), |o| o == MODEL_API);
}

#[test]
fn the_published_page_is_built_and_complete() {
    const PUBLIC_URL: &str = "/playground/";
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/playground");
    let html = read(&dir.join("index.html"));
    // Published on the free chat, or on the visitor's own key: one of the
    // two, never both, and nothing else.
    check(&html, |o| o == MODEL_API || is_free_chat(o));
    // trunk's loader, from the Pages path, and the files it names are there.
    assert!(
        !html.contains("/helmsman/playground/"),
        "the site is at the root of helmsman.advin.io"
    );
    let mut named = Vec::new();
    for (at, _) in html.match_indices(PUBLIC_URL) {
        let rest = &html[at + PUBLIC_URL.len()..];
        let file = &rest[..rest.find(['\'', '"']).expect("closing quote")];
        assert!(dir.join(file).is_file(), "{file} is missing from docs/playground");
        named.push(file);
    }
    for end in [".js", "_bg.wasm", "-helmsman-canvas-initializer.js"] {
        assert!(named.iter().any(|f| f.ends_with(end)), "no {end} in {named:?}");
    }
    check_built(&html, &dir);
    // Nothing else that could hold a key or a build of something else.
    for e in std::fs::read_dir(&dir).unwrap() {
        let name = e.unwrap().file_name().into_string().unwrap();
        assert!(
            name == "index.html"
                || name == "helmsman-mark-16-yellow.svg"
                || name.ends_with("-helmsman-canvas-initializer.js")
                || (name.starts_with("helmsman-canvas-")
                    && (Path::new(&name).extension().is_some_and(|x| x == "js")
                        || name.ends_with("_bg.wasm"))),
            "unexpected file {name}"
        );
    }
}
