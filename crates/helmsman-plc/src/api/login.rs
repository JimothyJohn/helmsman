//! `POST /api/login`, `POST /api/logout` and `GET /api/session`: login
//! sessions ([`crate::policy::account`]), for a person (a box with an
//! accounts file: `{user, password}`) or for a role (a box without one:
//! `{role, pin}`, checked by the PIN gate with its lockout, so a PIN
//! guessed through a login costs what one guessed through an apply does).
//!
//! A login takes no token (it is its own credential) and a JSON body
//! only (`Content-Type: application/json`, which a cross-site form cannot
//! send). The session reaches the caller one of two ways:
//!
//! - **A browser** (the request carries [`BROWSER_HEADER`] and passes
//!   [`same_origin`]): as the [`SESSION_COOKIE`] cookie, `HttpOnly;
//!   SameSite=Strict; Path=/api; Max-Age=<session life>`, and never in
//!   the body, so no script on the page can read it. A login over a
//!   cookie the browser already has ends that session first (the session
//!   is rotated, never reused). Over the box's HTTPS it is also `Secure`;
//!   plain HTTP is served only from the box's own host then (an SSH
//!   tunnel, a local browser), where it is not. The same-origin proof
//!   compares the Origin with the scheme the request came over.
//! - **Anything else** (scripts, the CLI): as `token` in the body, sent
//!   back as `Authorization: Bearer`, as before.
//!
//! A cookie is only ever a session: the box token is never a cookie, and
//! `/mcp` (outside the cookie's path, and refused there anyway) takes
//! the box token only. When a request carries both an `Authorization`
//! header and the cookie, the header is the credential and the cookie is
//! not read: a cookie never rescues a wrong token, and a token is never
//! held to the cookie's same-origin rule (no other site can make a
//! browser send it).
//!
//! **A request authenticated by the cookie** (anything but a `GET`) must
//! prove it comes from the box's own page ([`same_origin`]): the
//! [`BROWSER_HEADER`] (a cross-site form cannot send a custom header, and
//! a cross-site `fetch` with one needs a preflight the box never
//! answers), and an `Origin` that is this box (`http://` + its `Host`),
//! or, with no `Origin`, a `Referer` on this box. Otherwise 403, and the
//! session is not touched.
//!
//! Passwords and PINs are only ever in the request body: never logged,
//! audited or echoed (a malformed body gets a fixed message, not
//! serde's), and a user name is logged or audited only when it names an
//! account, so a password typed into the user field never lands in the
//! log. Session tokens are never logged or audited either way.
//!
//! Audited: every login that worked, every wrong password and every wrong
//! PIN, with the account or the role. Only logged: refusals while locked,
//! malformed or cross-site logins, and logins of the wrong kind for the
//! box, so an unauthenticated flood cannot fill the audit log (which
//! refuses rather than forgets).

use serde::Deserialize;
use serde_json::json;

use super::http::SESSION_COOKIE;
use super::{Ctx, Note, Request, Response, append_as, audit_full, clean, is_json, logins, record};
use crate::policy::Role;
use crate::policy::account::{
    Issued, LoginRefused, Logins, Mode, Principal, Proof, SessionRefused, valid_user,
};
use crate::policy::pin::Refused;

/// The header the canvas sends on every call it makes with the cookie.
pub const BROWSER_HEADER: &str = "x-helmsman-request";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    user: Option<String>,
    password: Option<String>,
    role: Option<String>,
    pin: Option<String>,
}

/// What a login presents.
enum Credential {
    Account { user: String, password: String },
    Pin { role: String, pin: String },
}

const SHAPE: &str = "expected JSON {\"user\": ..., \"password\": ...} or {\"role\": ..., \"pin\": ...}";

fn credential(req: &Request) -> Option<Credential> {
    // serde's message can quote the body, password or PIN included:
    // never echoed.
    match serde_json::from_slice::<Body>(&req.body).ok()? {
        Body {
            user: Some(user),
            password: Some(password),
            role: None,
            pin: None,
        } => Some(Credential::Account { user, password }),
        Body {
            user: None,
            password: None,
            role: Some(role),
            pin: Some(pin),
        } => Some(Credential::Pin { role, pin }),
        _ => None,
    }
}

/// The value of a header sent exactly once.
fn one<'a>(req: &'a Request, name: &'a str) -> Option<&'a str> {
    let mut v = req.header(name);
    let first = v.next();
    if v.next().is_some() {
        return None;
    }
    first
}

/// Whether the caller says it is the canvas (it sent [`BROWSER_HEADER`]).
fn browser(req: &Request) -> bool {
    req.header(BROWSER_HEADER).next().is_some()
}

/// Whether `req` provably comes from this box's own page: exactly one
/// `X-Helmsman-Request: 1`, and an `Origin` of `http://` + its `Host` (or,
/// without an `Origin`, a `Referer` on it). What is wrong, if not.
///
/// # Errors
/// Why the request is refused, in the words the 403 carries.
pub(super) fn same_origin(req: &Request) -> Result<(), &'static str> {
    if one(req, BROWSER_HEADER) != Some("1") {
        return Err("a browser session needs the X-Helmsman-Request: 1 header (the box's own page sends it)");
    }
    let Some(host) = one(req, "host").filter(|h| !h.is_empty()) else {
        return Err("a browser session needs exactly one Host header");
    };
    let me = format!("{}://{host}", if req.tls { "https" } else { "http" });
    let origins: Vec<&str> = req.header("origin").collect();
    match origins.as_slice() {
        [o] if o.eq_ignore_ascii_case(&me) => Ok(()),
        [_] => Err("refused: the request came from another site (its Origin is not this box)"),
        [] => match one(req, "referer") {
            Some(r)
                if r.len() > me.len()
                    && r.as_bytes()[me.len()] == b'/'
                    && r[..me.len()].eq_ignore_ascii_case(&me) =>
            {
                Ok(())
            }
            Some(_) => Err("refused: the request came from another site (its Referer is not this box)"),
            None => Err("refused: no Origin or Referer shows the request came from this box's page"),
        },
        _ => Err("refused: more than one Origin header"),
    }
}

/// `Set-Cookie` for a new session, living as long as it may.
#[must_use]
pub fn set_cookie(token: &str, max_age_s: u64) -> String {
    format!("{SESSION_COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/api; Max-Age={max_age_s}")
}

/// `cookie` with `Secure` when `req` came over the box's HTTPS, so the
/// browser never sends it back in clear (`box-https`). Over plain HTTP
/// (this host only, on a box with HTTPS) it stays as it was.
fn secure(req: &Request, cookie: String) -> String {
    if req.tls { cookie + "; Secure" } else { cookie }
}

/// `Set-Cookie` that makes the browser drop the session cookie.
#[must_use]
pub fn clear_cookie() -> String {
    format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Strict; Path=/api; Max-Age=0")
}

/// Authenticates `req` by its session cookie (no `Authorization` header
/// was sent): the same-origin proof first, so a forged request neither
/// uses nor refreshes the session.
///
/// # Errors
/// The refusal: 403 without the proof, 401 (and the cookie cleared) when
/// the cookie is no live session.
pub(super) fn by_cookie(req: &mut Request, ctx: &Ctx) -> Result<(), Response> {
    let log = |why: &str| {
        (ctx.cfg.log)(&format!(
            "api: refused {} {} (session cookie: {why})",
            clean(&req.method),
            clean(&req.path)
        ));
    };
    if let Err(why) = same_origin(req) {
        log(why);
        return Err(Response::error(403, why));
    }
    let gone = |why: &str| Response::error(401, why).with("Set-Cookie", secure(req, clear_cookie()));
    let [token] = req.cookies.as_slice() else {
        log("more than one");
        return Err(gone("more than one session cookie; sign in again"));
    };
    let Some(l) = logins(ctx) else {
        log("no sessions on this box");
        return Err(gone("this box holds no login sessions"));
    };
    match l.session(token) {
        Ok(who) => {
            req.session = Some(who);
            req.by_cookie = true;
            Ok(())
        }
        Err(e) => {
            log("not a live session");
            Err(gone(refused_why(e)))
        }
    }
}

/// What a 401 says for a session that was not taken.
const fn refused_why(e: SessionRefused) -> &'static str {
    match e {
        SessionRefused::Expired => "session expired; log in again",
        SessionRefused::Revoked => "session ended: the account was disabled or changed; log in again",
        SessionRefused::Unknown => "not logged in, or the session ended; log in again",
    }
}

/// `POST /api/login`.
pub(super) fn login(req: &Request, ctx: &Ctx) -> Response {
    let log = |what: &str| (ctx.cfg.log)(&format!("api: POST {}: {what}", clean(&req.path)));
    let Some(l) = logins(ctx) else {
        log("refused: this box holds no login sessions");
        return Response::error(409, "this box has no approval path, so nothing to log in to");
    };
    if !is_json(req) {
        log("refused: not JSON");
        return Response::error(415, "a login is JSON: Content-Type: application/json");
    }
    let browser = browser(req);
    if browser && let Err(why) = same_origin(req) {
        log(&format!("refused: {why}"));
        return Response::error(403, why);
    }
    let Some(cred) = credential(req) else {
        log("refused: malformed login");
        return Response::error(400, SHAPE);
    };
    let issued = match cred {
        Credential::Account { user, password } => account(req, ctx, l, &user, password),
        Credential::Pin { role, pin } => role_pin(req, ctx, l, &role, pin),
    };
    let issued = match issued {
        Ok(i) => i,
        Err(r) => return r,
    };
    let who = &issued.who;
    let detail = format!("login: {}", who.label());
    if append_as(
        ctx,
        req,
        200,
        &detail,
        (None, None),
        who.person().map(str::to_string),
        Some(who.role),
    )
    .is_none()
    {
        l.logout(&issued.token);
        return audit_full();
    }
    let mut body = json!({
        "user": who.person(),
        "role": who.role.as_str(),
        "credential": credential_name(who),
        "expires_in_s": issued.ttl.as_secs(),
        "idle_s": issued.idle.as_secs(),
    });
    if !browser {
        body["token"] = json!(issued.token);
        return Response::json(200, &body).with("Cache-Control", "no-store".into());
    }
    // Rotation: whatever session this browser had ends here.
    for old in &req.cookies {
        l.logout(old);
    }
    body["cookie"] = json!(true);
    Response::json(200, &body)
        .with("Cache-Control", "no-store".into())
        .with(
            "Set-Cookie",
            secure(req, set_cookie(&issued.token, issued.ttl.as_secs())),
        )
}

const fn credential_name(who: &Principal) -> &'static str {
    match who.proof {
        Proof::Password => "account",
        Proof::Pin => "pin",
    }
}

/// `{user, password}`: a box with accounts.
fn account(req: &Request, ctx: &Ctx, l: &Logins, user: &str, password: String) -> Result<Issued, Response> {
    let log = |what: &str| (ctx.cfg.log)(&format!("api: POST {}: {what}", clean(&req.path)));
    let known = valid_user(user) && matches!(l.mode(), Mode::Accounts(a) if a.get(user).is_some());
    let named = if known {
        user.to_string()
    } else {
        "an unknown user".to_string()
    };
    let outcome = l.login(user, &password);
    drop(password);
    match outcome {
        Ok(issued) => Ok(issued),
        Err(LoginRefused::Wrong) => {
            let user = known.then(|| named.clone());
            Err(
                match append_as(
                    ctx,
                    req,
                    403,
                    &format!("login refused: wrong user or password ({named})"),
                    (None, None),
                    user,
                    None,
                ) {
                    Some(_) => Response::error(403, "wrong user or password"),
                    None => audit_full(),
                },
            )
        }
        Err(LoginRefused::Locked(wait)) => {
            let secs = wait.as_secs().max(1);
            log(&format!("refused ({named}): logins locked for {secs} s"));
            Err(Response::json(
                429,
                &json!({ "error": "too many wrong passwords; logins are locked", "retry_after_s": secs }),
            )
            .with("Retry-After", secs.to_string()))
        }
        Err(LoginRefused::NoAccounts) => {
            log("refused: this box has no accounts");
            Err(Response::error(
                409,
                "this box has no accounts: log in with a role and its PIN",
            ))
        }
        Err(LoginRefused::Unavailable(why)) => Err(unavailable(&log, &why)),
        Err(LoginRefused::Full) => {
            log(&format!("refused ({named}): too many sessions"));
            Err(full())
        }
    }
}

fn unavailable(log: &dyn Fn(&str), why: &str) -> Response {
    log(&format!("refused: {}", clean(why)));
    Response::error(
        503,
        "the box's accounts cannot be used now; nobody can log in (see its journal)",
    )
}

fn full() -> Response {
    Response::error(503, "too many sessions; try again later")
}

/// `{role, pin}`: a box without accounts. The PIN goes through the gate
/// apply uses: its lockout, its 1 KiB cap, its constant-time check.
fn role_pin(req: &Request, ctx: &Ctx, l: &Logins, role: &str, pin: String) -> Result<Issued, Response> {
    let log = |what: &str| (ctx.cfg.log)(&format!("api: POST {}: {what}", clean(&req.path)));
    match l.mode() {
        Mode::Pins => {}
        Mode::Accounts(_) => {
            log("refused: a role PIN on a box with accounts");
            return Err(Response::error(
                403,
                "this box has accounts: role PINs are off; log in with your user name and password",
            ));
        }
        Mode::Broken(why) => return Err(unavailable(&log, &why)),
    }
    let Some(role) = Role::parse(role) else {
        log("refused: no such role");
        return Err(Response::error(
            400,
            "role must be operator, engineer or integrator",
        ));
    };
    let Some(gate) = ctx.approval.as_ref().map(|a| &a.gate) else {
        return Err(Response::error(
            409,
            "this box has no approval path, so nothing to log in to",
        ));
    };
    let checked = gate.check(role, &pin);
    drop(pin);
    match checked {
        Ok(()) => l.issue_role(role).map_err(|e| match e {
            LoginRefused::Full => {
                log(&format!("refused ({role}): too many sessions"));
                full()
            }
            LoginRefused::Unavailable(why) => unavailable(&log, &why),
            _ => Response::error(503, "no session could be opened"),
        }),
        Err(Refused::Wrong) => Err(
            match append_as(
                ctx,
                req,
                403,
                &format!("login refused: wrong PIN ({role})"),
                (None, None),
                None,
                Some(role),
            ) {
                Some(_) => Response::error(403, "wrong PIN"),
                None => audit_full(),
            },
        ),
        Err(Refused::Locked(wait)) => {
            let secs = wait.as_secs().max(1);
            log(&format!("refused ({role}): PIN entry locked for {secs} s"));
            Err(Response::json(
                429,
                &json!({ "error": "too many wrong PINs; PIN entry is locked", "retry_after_s": secs }),
            )
            .with("Retry-After", secs.to_string()))
        }
    }
}

/// `POST /api/logout`: ends the session it was sent with (the bearer, or
/// the cookie, which it also clears).
pub(super) fn logout(req: &Request, ctx: &Ctx) -> Response {
    let token = if req.by_cookie {
        req.cookies.first().map(String::as_str)
    } else {
        req.bearer.as_deref()
    };
    let ended = match (&req.session, token, logins(ctx)) {
        (Some(_), Some(t), Some(l)) => l.logout(t),
        _ => None,
    };
    let (status, body, detail) = match ended {
        Some(who) => (
            200,
            json!({ "logged_out": true }),
            format!("logout: {}", who.label()),
        ),
        None => (
            400,
            json!({ "error": "not a login session; nothing to log out" }),
            "logout: not a session".to_string(),
        ),
    };
    let r = match record(ctx, req, status, &detail, Note::default()) {
        Some(_) => Response::json(status, &body),
        None => audit_full(),
    };
    // That session's forces end with it.
    super::forces::sweep(ctx);
    if req.by_cookie {
        r.with("Set-Cookie", secure(req, clear_cookie()))
    } else {
        r
    }
}

/// `GET /api/session`: who the session sent (the cookie, or a bearer
/// session) is, or 401. Using it counts as using the session.
pub(super) fn whoami(req: &Request, ctx: &Ctx) -> Response {
    let cookie = !req.authorization;
    let token = if cookie {
        match req.cookies.as_slice() {
            [t] => Some(t.as_str()),
            _ => None,
        }
    } else {
        req.bearer.as_deref()
    };
    let refused = |why: &str| {
        let r = Response::error(401, why);
        if cookie && !req.cookies.is_empty() {
            r.with("Set-Cookie", secure(req, clear_cookie()))
        } else {
            r
        }
    };
    let (Some(l), Some(t)) = (logins(ctx), token) else {
        return refused("not logged in");
    };
    match l.session(t) {
        Ok(who) => Response::json(
            200,
            &json!({
                "user": who.person(),
                "role": who.role.as_str(),
                "credential": credential_name(&who),
                "via": if cookie { "cookie" } else { "bearer" },
                "idle_s": l.limits().idle.as_secs(),
            }),
        ),
        Err(e) => refused(refused_why(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Limits;

    fn req(origin: &str, tls: bool) -> Request {
        let head = format!(
            "POST /api/login HTTP/1.1\r\nHost: box:8080\r\nX-Helmsman-Request: 1\r\nOrigin: {origin}"
        );
        let mut r = super::super::http::parse_head(head.as_bytes(), &Limits::default())
            .unwrap()
            .0;
        r.tls = tls;
        r
    }

    #[test]
    fn the_same_origin_proof_takes_the_scheme_the_request_came_over() {
        assert!(same_origin(&req("https://box:8080", true)).is_ok());
        assert!(same_origin(&req("http://box:8080", false)).is_ok());
        // A page over plain HTTP cannot pass for the HTTPS one, nor back.
        assert!(same_origin(&req("http://box:8080", true)).is_err());
        assert!(same_origin(&req("https://box:8080", false)).is_err());
    }

    #[test]
    fn a_cookie_over_tls_is_secure_and_over_plain_http_as_before() {
        let c = set_cookie("t", 60);
        assert_eq!(
            secure(&req("https://box:8080", true), c.clone()),
            format!("{c}; Secure")
        );
        assert_eq!(secure(&req("http://box:8080", false), c.clone()), c);
        assert!(secure(&req("https://box:8080", true), clear_cookie()).ends_with("Max-Age=0; Secure"));
    }
}
